//! Command predictor using n-gram frequency model with optional LLM fallback.
//!
//! Learns from shell history and terminal output to suggest completions.
//! Pure Rust n-gram prediction in <100µs; optional Ollama LLM fallback
//! when n-gram confidence is low.

use parking_lot::Mutex;
use std::collections::HashMap;

#[cfg(feature = "ollama")]
use crate::ollama_client::OllamaClient;

/// Maximum history entries to retain.
const MAX_HISTORY: usize = 5000;

/// Maximum n-gram context length.
const MAX_NGRAM: usize = 3;

/// Minimum n-gram score below which LLM fallback is triggered.
#[cfg(feature = "ollama")]
const LLM_FALLBACK_THRESHOLD: f32 = 0.5;

/// A predicted command completion.
#[derive(Debug, Clone)]
pub struct Prediction {
    /// The full predicted command text.
    pub text: String,
    /// Confidence score (0.0-1.0).
    pub score: f32,
    /// How many times this command appeared in history.
    pub frequency: u32,
}

/// Context for hybrid (n-gram + LLM) prediction.
pub struct PredictionContext {
    /// Current working directory.
    pub cwd: String,
    /// Last ~10 commands.
    pub recent_commands: Vec<String>,
    /// Last ~5 lines of terminal output (optional).
    pub recent_output: Option<String>,
}

/// N-gram entry tracking frequency and recency.
#[derive(Debug, Clone)]
struct NgramEntry {
    text: String,
    count: u32,
    last_seen: u64,
}

/// Command predictor that learns from shell history.
pub struct CommandPredictor {
    state: Mutex<PredictorState>,
    #[cfg(feature = "ollama")]
    ollama: Option<OllamaClient>,
}

struct PredictorState {
    /// Full command history (most recent last).
    history: Vec<String>,
    /// Prefix → candidates map for fast lookup.
    prefix_map: HashMap<String, Vec<NgramEntry>>,
    /// Monotonic counter for recency tracking.
    tick: u64,
}

impl CommandPredictor {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(PredictorState {
                history: Vec::new(),
                prefix_map: HashMap::new(),
                tick: 0,
            }),
            #[cfg(feature = "ollama")]
            ollama: None,
        }
    }

    /// Create a predictor with Ollama LLM fallback enabled.
    #[cfg(feature = "ollama")]
    pub fn with_ollama(ollama: OllamaClient) -> Self {
        Self {
            state: Mutex::new(PredictorState {
                history: Vec::new(),
                prefix_map: HashMap::new(),
                tick: 0,
            }),
            ollama: Some(ollama),
        }
    }

    /// Record a completed command (e.g., when the user presses Enter).
    pub fn learn(&self, command: &str) {
        let command = command.trim();
        if command.is_empty() || command.len() < 2 {
            return;
        }

        let mut state = self.state.lock();
        state.tick += 1;
        let tick = state.tick;

        // Add to history
        state.history.push(command.to_string());
        if state.history.len() > MAX_HISTORY {
            state.history.remove(0);
        }

        // Build n-gram prefixes
        let words: Vec<&str> = command.split_whitespace().collect();
        for n in 1..=MAX_NGRAM.min(words.len()) {
            let prefix = words[..n - 1].join(" ");
            let key = if prefix.is_empty() {
                "".to_string()
            } else {
                prefix
            };

            let entries = state.prefix_map.entry(key).or_default();
            if let Some(entry) = entries.iter_mut().find(|e| e.text == command) {
                entry.count += 1;
                entry.last_seen = tick;
            } else {
                entries.push(NgramEntry {
                    text: command.to_string(),
                    count: 1,
                    last_seen: tick,
                });
            }
        }

        // Also index by character prefixes for inline completion
        for len in 1..=command.len().min(20) {
            if command.is_char_boundary(len) {
                let char_prefix = &command[..len];
                let entries = state
                    .prefix_map
                    .entry(char_prefix.to_string())
                    .or_default();
                if let Some(entry) = entries.iter_mut().find(|e| e.text == command) {
                    entry.count += 1;
                    entry.last_seen = tick;
                } else {
                    entries.push(NgramEntry {
                        text: command.to_string(),
                        count: 1,
                        last_seen: tick,
                    });
                }
            }
        }
    }

    /// Predict completions for the current input.
    /// Returns up to `top_k` predictions sorted by score.
    pub fn predict(&self, input: &str, top_k: usize) -> Vec<Prediction> {
        let input = input.trim();
        if input.is_empty() {
            return Vec::new();
        }

        let state = self.state.lock();
        let tick = state.tick;

        // Collect candidates from prefix map
        let mut candidates: HashMap<String, (u32, u64)> = HashMap::new();

        // Exact prefix match
        if let Some(entries) = state.prefix_map.get(input) {
            for entry in entries {
                if entry.text.starts_with(input) && entry.text != input {
                    let e = candidates.entry(entry.text.clone()).or_insert((0, 0));
                    e.0 += entry.count;
                    e.1 = e.1.max(entry.last_seen);
                }
            }
        }

        // Also try word-based prefix
        let words: Vec<&str> = input.split_whitespace().collect();
        if !words.is_empty() {
            let word_prefix = words[..words.len().saturating_sub(1)].join(" ");
            if let Some(entries) = state.prefix_map.get(&word_prefix) {
                for entry in entries {
                    if entry.text.starts_with(input) && entry.text != input {
                        let e = candidates.entry(entry.text.clone()).or_insert((0, 0));
                        e.0 += entry.count;
                        e.1 = e.1.max(entry.last_seen);
                    }
                }
            }
        }

        if candidates.is_empty() {
            // Fallback: scan history for prefix matches
            for cmd in state.history.iter().rev().take(500) {
                if cmd.starts_with(input) && cmd != input {
                    let e = candidates.entry(cmd.clone()).or_insert((0, 0));
                    e.0 += 1;
                    e.1 = e.1.max(tick);
                }
            }
        }

        // Score: frequency * recency_decay
        let mut scored: Vec<Prediction> = candidates
            .into_iter()
            .map(|(text, (count, last_seen))| {
                let recency = 1.0 / (1.0 + (tick.saturating_sub(last_seen)) as f32 / 100.0);
                let freq_score = (count as f32).ln().max(0.0) + 1.0;
                let score = freq_score * recency;
                Prediction {
                    text,
                    score,
                    frequency: count,
                }
            })
            .collect();

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }

    /// Get the single best ghost-text suggestion.
    /// Returns only the completion suffix (part after the input).
    pub fn ghost_text(&self, input: &str) -> Option<String> {
        let predictions = self.predict(input, 1);
        predictions.into_iter().next().and_then(|p| {
            p.text.strip_prefix(input).map(|s| s.to_string())
        })
    }

    /// Number of unique commands learned.
    pub fn history_len(&self) -> usize {
        self.state.lock().history.len()
    }

    /// Clear all learned data.
    pub fn clear(&self) {
        let mut state = self.state.lock();
        state.history.clear();
        state.prefix_map.clear();
        state.tick = 0;
    }

    /// Hybrid prediction: n-gram first, LLM fallback when confidence is low.
    ///
    /// Returns n-gram predictions if the top score exceeds the threshold.
    /// Otherwise, queries the local Ollama LLM for a prediction and prepends it.
    #[cfg(feature = "ollama")]
    pub fn predict_hybrid(&self, input: &str, context: &PredictionContext) -> Vec<Prediction> {
        let mut results = self.predict(input, 5);

        // If n-gram confidence is high enough, skip LLM.
        let top_score = results.first().map(|p| p.score).unwrap_or(0.0);
        if top_score >= LLM_FALLBACK_THRESHOLD {
            return results;
        }

        // Attempt LLM fallback.
        let ollama = match &self.ollama {
            Some(o) => o,
            None => return results,
        };

        if !ollama.is_available() {
            return results;
        }

        let history_refs: Vec<&str> = context
            .recent_commands
            .iter()
            .map(|s| s.as_str())
            .collect();

        match ollama.predict_command(input, &history_refs, &context.cwd) {
            Ok(Some(predicted)) => {
                // Prepend LLM prediction with a synthetic high score.
                let already_present = results.iter().any(|p| p.text == predicted);
                if !already_present {
                    results.insert(
                        0,
                        Prediction {
                            text: predicted,
                            score: 1.0,
                            frequency: 0,
                        },
                    );
                }
                results
            }
            _ => results,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learn_and_predict() {
        let p = CommandPredictor::new();
        p.learn("cargo build --release");
        p.learn("cargo test");
        p.learn("cargo build --release");
        p.learn("cargo fmt");

        let predictions = p.predict("cargo b", 3);
        assert!(!predictions.is_empty());
        assert_eq!(predictions[0].text, "cargo build --release");
        assert!(predictions[0].frequency >= 2);
    }

    #[test]
    fn ghost_text_completion() {
        let p = CommandPredictor::new();
        p.learn("git push origin main");
        p.learn("git pull origin main");
        p.learn("git push origin main");

        let ghost = p.ghost_text("git pu");
        assert!(ghost.is_some());
        // Should complete to "push origin main" (most frequent)
        assert_eq!(ghost.unwrap(), "sh origin main");
    }

    #[test]
    fn no_prediction_for_empty() {
        let p = CommandPredictor::new();
        p.learn("ls -la");
        assert!(p.predict("", 3).is_empty());
    }

    #[test]
    fn no_exact_match_in_predictions() {
        let p = CommandPredictor::new();
        p.learn("cargo build");
        let predictions = p.predict("cargo build", 3);
        assert!(predictions.is_empty());
    }

    #[test]
    fn recency_boost() {
        let p = CommandPredictor::new();
        // Learn old command many times
        for _ in 0..10 {
            p.learn("npm install");
        }
        // Learn new command once
        p.learn("npm init");

        let predictions = p.predict("npm i", 2);
        assert_eq!(predictions.len(), 2);
        // "npm install" should still rank first due to high frequency
        assert_eq!(predictions[0].text, "npm install");
    }

    #[test]
    fn history_limit() {
        let p = CommandPredictor::new();
        for i in 0..6000 {
            p.learn(&format!("cmd_{i}"));
        }
        assert!(p.history_len() <= MAX_HISTORY);
    }

    #[test]
    fn clear_resets() {
        let p = CommandPredictor::new();
        p.learn("test");
        assert!(p.history_len() > 0);
        p.clear();
        assert_eq!(p.history_len(), 0);
    }
}
