//! Speculative Decoding — draft-then-verify pipeline for faster inference.
//!
//! Ollama's 2026 roadmap includes speculative decoding integration. The core
//! idea: run a small "draft" model to generate K candidate tokens, then verify
//! them in a single forward pass of the full model. Accepted tokens are free
//! (amortized cost of one large-model call for K tokens).
//!
//! This module implements:
//! 1. **Draft generation** — fast local n-gram / trie-based prediction
//! 2. **Parallel verification** — batch-verify candidates via Ollama API
//! 3. **Acceptance sampling** — accept prefix of matching tokens
//! 4. **Adaptive draft length** — tune K based on acceptance rate
//!
//! For terminal command prediction, the "draft model" is our KvPrefixCache
//! (zero-cost trie lookup), and the "verifier" is the Ollama LLM.

use std::time::{Duration, Instant};

/// Configuration for speculative decoding.
#[derive(Debug, Clone)]
pub struct SpeculativeConfig {
    /// Maximum number of draft tokens to generate per step.
    pub max_draft_tokens: usize,
    /// Minimum acceptance rate before reducing draft length.
    pub min_acceptance_rate: f32,
    /// Maximum acceptance rate before increasing draft length.
    pub max_acceptance_rate: f32,
    /// Target tokens per second for adaptive tuning.
    pub target_tokens_per_sec: f32,
}

impl Default for SpeculativeConfig {
    fn default() -> Self {
        Self {
            max_draft_tokens: 8,
            min_acceptance_rate: 0.3,
            max_acceptance_rate: 0.8,
            target_tokens_per_sec: 50.0,
        }
    }
}

/// A single speculative decoding step result.
#[derive(Debug, Clone)]
pub struct SpeculativeResult {
    /// Tokens accepted from the draft.
    pub accepted_tokens: Vec<String>,
    /// Number of draft tokens that were proposed.
    pub draft_count: usize,
    /// Number of tokens accepted by the verifier.
    pub accepted_count: usize,
    /// Time spent on draft generation.
    pub draft_time: Duration,
    /// Time spent on verification.
    pub verify_time: Duration,
    /// Current acceptance rate (exponential moving average).
    pub acceptance_rate: f32,
}

/// Draft generator trait — any fast local predictor can serve as a draft model.
pub trait DraftGenerator {
    /// Generate up to `max_tokens` draft continuations for the given context.
    /// Returns a list of candidate token strings.
    fn generate_draft(&self, context: &[String], max_tokens: usize) -> Vec<String>;
}

/// N-gram based draft generator — uses command history patterns.
pub struct NgramDraftGenerator {
    /// N-gram table: (context_suffix) → Vec<(next_token, frequency)>
    bigrams: std::collections::HashMap<String, Vec<(String, u32)>>,
    trigrams: std::collections::HashMap<(String, String), Vec<(String, u32)>>,
}

impl NgramDraftGenerator {
    pub fn new() -> Self {
        Self {
            bigrams: std::collections::HashMap::new(),
            trigrams: std::collections::HashMap::new(),
        }
    }

    /// Train on a sequence of command tokens.
    pub fn train(&mut self, tokens: &[String]) {
        // Bigrams
        for window in tokens.windows(2) {
            let entry = self.bigrams.entry(window[0].clone()).or_default();
            if let Some(item) = entry.iter_mut().find(|(t, _)| t == &window[1]) {
                item.1 += 1;
            } else {
                entry.push((window[1].clone(), 1));
            }
        }

        // Trigrams
        for window in tokens.windows(3) {
            let key = (window[0].clone(), window[1].clone());
            let entry = self.trigrams.entry(key).or_default();
            if let Some(item) = entry.iter_mut().find(|(t, _)| t == &window[2]) {
                item.1 += 1;
            } else {
                entry.push((window[2].clone(), 1));
            }
        }
    }
}

impl DraftGenerator for NgramDraftGenerator {
    fn generate_draft(&self, context: &[String], max_tokens: usize) -> Vec<String> {
        let mut draft = Vec::with_capacity(max_tokens);
        let mut ctx: Vec<String> = context.to_vec();

        for _ in 0..max_tokens {
            // Try trigram first
            let next = if ctx.len() >= 2 {
                let key = (
                    ctx[ctx.len() - 2].clone(),
                    ctx[ctx.len() - 1].clone(),
                );
                self.trigrams
                    .get(&key)
                    .and_then(|candidates| candidates.iter().max_by_key(|(_, f)| *f))
                    .map(|(t, _)| t.clone())
            } else {
                None
            };

            // Fallback to bigram
            let next = next.or_else(|| {
                ctx.last()
                    .and_then(|last| self.bigrams.get(last))
                    .and_then(|candidates| candidates.iter().max_by_key(|(_, f)| *f))
                    .map(|(t, _)| t.clone())
            });

            match next {
                Some(token) => {
                    ctx.push(token.clone());
                    draft.push(token);
                }
                None => break,
            }
        }

        draft
    }
}

/// Speculative decoder — orchestrates draft → verify → accept pipeline.
pub struct SpeculativeDecoder {
    config: SpeculativeConfig,
    /// Exponential moving average of acceptance rate.
    ema_acceptance: f32,
    /// Current adaptive draft length.
    current_draft_len: usize,
    /// Total tokens generated.
    total_generated: u64,
    /// Total tokens accepted from drafts.
    total_accepted: u64,
    /// Total time saved vs sequential decoding.
    total_time_saved: Duration,
}

impl SpeculativeDecoder {
    pub fn new(config: SpeculativeConfig) -> Self {
        let initial_len = config.max_draft_tokens / 2;
        Self {
            config,
            ema_acceptance: 0.5,
            current_draft_len: initial_len.max(1),
            total_generated: 0,
            total_accepted: 0,
            total_time_saved: Duration::ZERO,
        }
    }

    /// Run one speculative step: draft → verify → accept.
    ///
    /// The `verify_fn` takes a context + draft tokens and returns how many
    /// of the draft tokens the verifier model accepts (prefix match).
    pub fn step<D, V>(
        &mut self,
        draft_gen: &D,
        context: &[String],
        verify_fn: V,
    ) -> SpeculativeResult
    where
        D: DraftGenerator,
        V: FnOnce(&[String], &[String]) -> usize,
    {
        // 1. Draft generation (fast)
        let draft_start = Instant::now();
        let draft = draft_gen.generate_draft(context, self.current_draft_len);
        let draft_time = draft_start.elapsed();
        let draft_count = draft.len();

        if draft_count == 0 {
            return SpeculativeResult {
                accepted_tokens: Vec::new(),
                draft_count: 0,
                accepted_count: 0,
                draft_time,
                verify_time: Duration::ZERO,
                acceptance_rate: self.ema_acceptance,
            };
        }

        // 2. Verification (one batch call to large model)
        let verify_start = Instant::now();
        let accepted_count = verify_fn(context, &draft);
        let verify_time = verify_start.elapsed();

        let accepted_tokens: Vec<String> = draft[..accepted_count].to_vec();

        // 3. Update statistics
        self.total_generated += draft_count as u64;
        self.total_accepted += accepted_count as u64;

        let rate = if draft_count > 0 {
            accepted_count as f32 / draft_count as f32
        } else {
            0.0
        };

        // EMA update (alpha = 0.3 for responsiveness)
        self.ema_acceptance = 0.7 * self.ema_acceptance + 0.3 * rate;

        // 4. Adaptive draft length tuning
        if self.ema_acceptance < self.config.min_acceptance_rate && self.current_draft_len > 1 {
            self.current_draft_len -= 1;
        } else if self.ema_acceptance > self.config.max_acceptance_rate
            && self.current_draft_len < self.config.max_draft_tokens
        {
            self.current_draft_len += 1;
        }

        // Estimate time saved: accepted_count tokens that didn't need
        // individual large-model calls
        if accepted_count > 1 {
            let estimated_per_token = verify_time.as_secs_f64() / draft_count as f64;
            let saved =
                Duration::from_secs_f64(estimated_per_token * (accepted_count - 1) as f64);
            self.total_time_saved += saved;
        }

        SpeculativeResult {
            accepted_tokens,
            draft_count,
            accepted_count,
            draft_time,
            verify_time,
            acceptance_rate: self.ema_acceptance,
        }
    }

    /// Get cumulative statistics.
    pub fn stats(&self) -> SpeculativeStats {
        SpeculativeStats {
            total_generated: self.total_generated,
            total_accepted: self.total_accepted,
            overall_acceptance_rate: if self.total_generated > 0 {
                self.total_accepted as f32 / self.total_generated as f32
            } else {
                0.0
            },
            current_draft_len: self.current_draft_len,
            ema_acceptance: self.ema_acceptance,
            total_time_saved: self.total_time_saved,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpeculativeStats {
    pub total_generated: u64,
    pub total_accepted: u64,
    pub overall_acceptance_rate: f32,
    pub current_draft_len: usize,
    pub ema_acceptance: f32,
    pub total_time_saved: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ngram_draft_from_training_data() {
        let mut ngram = NgramDraftGenerator::new();
        let tokens: Vec<String> = vec!["git", "add", ".", "&&", "git", "commit"]
            .into_iter()
            .map(String::from)
            .collect();
        ngram.train(&tokens);

        let ctx = vec!["git".to_string()];
        let draft = ngram.generate_draft(&ctx, 3);
        assert!(!draft.is_empty());
        assert_eq!(draft[0], "add");
    }

    #[test]
    fn speculative_decoder_adapts_draft_length() {
        let config = SpeculativeConfig {
            max_draft_tokens: 6,
            min_acceptance_rate: 0.3,
            max_acceptance_rate: 0.8,
            ..Default::default()
        };
        let mut decoder = SpeculativeDecoder::new(config);
        let draft = NgramDraftGenerator::new();

        // All rejected → should decrease draft length
        for _ in 0..10 {
            decoder.step(&draft, &["ls".into()], |_, _| 0);
        }
        assert_eq!(decoder.current_draft_len, 1);
    }

    #[test]
    fn speculative_step_accepts_prefix() {
        let mut draft_gen = NgramDraftGenerator::new();
        draft_gen.train(
            &["cargo", "build", "--release"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );

        let config = SpeculativeConfig::default();
        let mut decoder = SpeculativeDecoder::new(config);

        let ctx = vec!["cargo".to_string()];
        let result = decoder.step(&draft_gen, &ctx, |_, draft| {
            // Verifier accepts first 2 tokens
            draft.len().min(2)
        });
        assert!(result.accepted_count <= result.draft_count);
    }
}
