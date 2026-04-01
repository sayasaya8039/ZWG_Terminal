//! Parallel Decoding — generate multiple candidate sequences simultaneously.
//!
//! Ollama supports parallel model scheduling: up to 6 models can coexist
//! in VRAM. This module extends that concept to parallel decoding within
//! a single inference session:
//!
//! 1. **Beam search** — maintain top-K candidate sequences scored by log-prob
//! 2. **Best-of-N sampling** — generate N candidates, pick best by scoring
//! 3. **Parallel prefix** — shared prompt processing, divergent generation
//! 4. **Chunked prefill** — process long prompts in fixed-size chunks to
//!    reduce time-to-first-token and enable progress reporting

use std::collections::BinaryHeap;
use std::cmp::Ordering;

/// A candidate sequence in beam search.
#[derive(Debug, Clone)]
pub struct BeamCandidate {
    /// Token IDs generated so far.
    pub tokens: Vec<u32>,
    /// Cumulative log-probability.
    pub log_prob: f64,
    /// Whether this candidate has emitted an end token.
    pub finished: bool,
}

impl PartialEq for BeamCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.log_prob == other.log_prob
    }
}

impl Eq for BeamCandidate {}

impl PartialOrd for BeamCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BeamCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher log_prob = better, so reverse comparison
        self.log_prob
            .partial_cmp(&other.log_prob)
            .unwrap_or(Ordering::Equal)
    }
}

/// Beam search state.
pub struct BeamSearch {
    /// Active beams.
    beams: Vec<BeamCandidate>,
    /// Finished beams (sorted by score).
    finished: BinaryHeap<BeamCandidate>,
    /// Beam width (K).
    beam_width: usize,
    /// Maximum sequence length.
    max_length: usize,
    /// Length penalty alpha (0 = no penalty, 1 = linear).
    length_penalty: f64,
}

impl BeamSearch {
    pub fn new(beam_width: usize, max_length: usize) -> Self {
        Self {
            beams: vec![BeamCandidate {
                tokens: Vec::new(),
                log_prob: 0.0,
                finished: false,
            }],
            finished: BinaryHeap::new(),
            beam_width,
            max_length,
            length_penalty: 0.6,
        }
    }

    /// Expand beams with new token candidates.
    ///
    /// `expand_fn` takes a beam's current tokens and returns
    /// Vec<(token_id, log_prob)> candidates for the next position.
    pub fn step<F>(&mut self, expand_fn: F)
    where
        F: Fn(&[u32]) -> Vec<(u32, f64)>,
    {
        let mut all_candidates: Vec<BeamCandidate> = Vec::new();

        for beam in &self.beams {
            if beam.finished || beam.tokens.len() >= self.max_length {
                self.finished.push(beam.clone());
                continue;
            }

            let next_tokens = expand_fn(&beam.tokens);

            for (token_id, token_log_prob) in next_tokens {
                let mut new_tokens = beam.tokens.clone();
                new_tokens.push(token_id);
                all_candidates.push(BeamCandidate {
                    tokens: new_tokens,
                    log_prob: beam.log_prob + token_log_prob,
                    finished: false,
                });
            }
        }

        // Keep top-K by normalized score
        all_candidates.sort_by(|a, b| {
            let score_a = self.normalized_score(a);
            let score_b = self.normalized_score(b);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(Ordering::Equal)
        });
        all_candidates.truncate(self.beam_width);
        self.beams = all_candidates;
    }

    /// Mark beams containing a specific end token as finished.
    pub fn mark_finished(&mut self, end_token: u32) {
        for beam in &mut self.beams {
            if beam.tokens.last() == Some(&end_token) {
                beam.finished = true;
            }
        }
        let (done, active): (Vec<_>, Vec<_>) =
            self.beams.drain(..).partition(|b| b.finished);
        for b in done {
            self.finished.push(b);
        }
        self.beams = active;
    }

    /// Get the best finished sequence (or best active if none finished).
    pub fn best(&self) -> Option<&BeamCandidate> {
        self.finished
            .peek()
            .or_else(|| self.beams.first())
    }

    /// Check if search is complete (all beams finished or max length reached).
    pub fn is_done(&self) -> bool {
        self.beams.is_empty()
    }

    /// Normalized score with length penalty.
    fn normalized_score(&self, candidate: &BeamCandidate) -> f64 {
        let len = candidate.tokens.len().max(1) as f64;
        candidate.log_prob / len.powf(self.length_penalty)
    }

    /// Get all active beams.
    pub fn active_beams(&self) -> &[BeamCandidate] {
        &self.beams
    }
}

// ── Chunked Prefill ─────────────────────────────────────────────────────

/// Configuration for chunked prompt processing.
#[derive(Debug, Clone)]
pub struct ChunkedPrefillConfig {
    /// Chunk size in tokens. Smaller = more responsive TTFT reporting,
    /// larger = better throughput.
    pub chunk_size: usize,
    /// Whether to yield between chunks (for progress callbacks).
    pub yield_between_chunks: bool,
}

impl Default for ChunkedPrefillConfig {
    fn default() -> Self {
        Self {
            chunk_size: 512,
            yield_between_chunks: true,
        }
    }
}

/// Progress report during chunked prefill.
#[derive(Debug, Clone)]
pub struct PrefillProgress {
    /// Tokens processed so far.
    pub processed: usize,
    /// Total tokens in the prompt.
    pub total: usize,
    /// Estimated time to completion (based on current rate).
    pub eta_ms: u64,
}

/// Process a prompt in chunks, calling `process_chunk` for each chunk
/// and `on_progress` between chunks.
///
/// Returns the total processing time.
pub fn chunked_prefill<F, P>(
    prompt_tokens: &[u32],
    config: &ChunkedPrefillConfig,
    mut process_chunk: F,
    mut on_progress: P,
) -> std::time::Duration
where
    F: FnMut(&[u32]),
    P: FnMut(PrefillProgress),
{
    let total = prompt_tokens.len();
    let start = std::time::Instant::now();

    for (i, chunk) in prompt_tokens.chunks(config.chunk_size).enumerate() {
        process_chunk(chunk);

        let processed = ((i + 1) * config.chunk_size).min(total);
        let elapsed = start.elapsed();
        let rate = if elapsed.as_millis() > 0 {
            processed as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };
        let remaining = total.saturating_sub(processed);
        let eta_ms = if rate > 0.0 {
            (remaining as f64 / rate * 1000.0) as u64
        } else {
            0
        };

        on_progress(PrefillProgress {
            processed,
            total,
            eta_ms,
        });
    }

    start.elapsed()
}

// ── Best-of-N Sampling ──────────────────────────────────────────────────

/// Generate N candidates and select the best one by a scoring function.
pub fn best_of_n<G, S>(
    n: usize,
    mut generate_fn: G,
    score_fn: S,
) -> Option<Vec<u32>>
where
    G: FnMut() -> Vec<u32>,
    S: Fn(&[u32]) -> f64,
{
    let mut best_tokens: Option<Vec<u32>> = None;
    let mut best_score = f64::NEG_INFINITY;

    for _ in 0..n {
        let candidate = generate_fn();
        let score = score_fn(&candidate);
        if score > best_score {
            best_score = score;
            best_tokens = Some(candidate);
        }
    }

    best_tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beam_search_finds_best_path() {
        let mut beam = BeamSearch::new(3, 5);

        // Step 1: expand with 3 candidates
        beam.step(|_tokens| {
            vec![(1, -0.5), (2, -1.0), (3, -2.0)]
        });
        assert_eq!(beam.active_beams().len(), 3);

        // Step 2: best beam (token 1) expands further
        beam.step(|tokens| {
            if tokens.last() == Some(&1) {
                vec![(10, -0.1), (11, -0.3)]
            } else {
                vec![(20, -1.0)]
            }
        });

        let best = beam.best().unwrap();
        assert_eq!(best.tokens[0], 1, "best beam should start with token 1");
    }

    #[test]
    fn beam_search_finishes_on_end_token() {
        let mut beam = BeamSearch::new(2, 10);
        beam.step(|_| vec![(1, -0.5), (99, -0.1)]); // 99 = end token
        beam.mark_finished(99);

        assert!(!beam.is_done()); // still have active beams
        assert_eq!(beam.finished.len(), 1);
    }

    #[test]
    fn chunked_prefill_reports_progress() {
        let tokens: Vec<u32> = (0..100).collect();
        let config = ChunkedPrefillConfig {
            chunk_size: 30,
            yield_between_chunks: true,
        };

        let mut progress_reports = Vec::new();
        let mut chunks_processed = 0usize;

        chunked_prefill(
            &tokens,
            &config,
            |_chunk| { chunks_processed += 1; },
            |progress| { progress_reports.push(progress); },
        );

        assert_eq!(chunks_processed, 4); // ceil(100/30) = 4
        assert_eq!(progress_reports.len(), 4);
        assert_eq!(progress_reports.last().unwrap().processed, 100);
    }

    #[test]
    fn best_of_n_selects_highest_score() {
        let mut counter = 0u32;
        let best = best_of_n(
            5,
            || {
                counter += 1;
                vec![counter]
            },
            |tokens| tokens[0] as f64, // higher token = better score
        );
        assert_eq!(best.unwrap(), vec![5]);
    }
}
