//! Sliding Window Attention — bounded context for long terminal sessions.
//!
//! For decoder-only models, sliding window attention limits the KV cache
//! to the most recent W tokens. This enables constant memory usage
//! regardless of session length, critical for terminal applications
//! where sessions can run for hours.
//!
//! Combined with Ollama's context shifting, important tokens (errors,
//! commands) are preserved in a pinned "anchor" region at the start,
//! while the sliding window covers the most recent output.
//!
//! Layout: [anchor_tokens (pinned)] ... [sliding_window (most recent W tokens)]

/// Configuration for sliding window attention.
#[derive(Debug, Clone)]
pub struct SlidingWindowConfig {
    /// Maximum window size in tokens.
    pub window_size: usize,
    /// Number of anchor tokens pinned at the start (never evicted).
    pub anchor_size: usize,
    /// Overlap between consecutive windows (for context continuity).
    pub overlap: usize,
}

impl Default for SlidingWindowConfig {
    fn default() -> Self {
        Self {
            window_size: 4096,
            anchor_size: 256,
            overlap: 128,
        }
    }
}

/// A token with its position and metadata for the sliding window.
#[derive(Debug, Clone)]
pub struct WindowToken {
    /// Global position in the full sequence.
    pub global_pos: usize,
    /// The token ID.
    pub token_id: u32,
    /// Whether this token is pinned (anchor region).
    pub pinned: bool,
    /// Importance score (higher = retained longer during eviction).
    pub importance: f32,
}

/// Sliding window context manager.
pub struct SlidingWindowContext {
    config: SlidingWindowConfig,
    /// Anchor tokens (pinned, never evicted).
    anchor: Vec<WindowToken>,
    /// Active window tokens (most recent).
    window: Vec<WindowToken>,
    /// Total tokens seen (including evicted).
    total_seen: usize,
    /// Number of context shifts performed.
    shifts: usize,
}

impl SlidingWindowContext {
    pub fn new(config: SlidingWindowConfig) -> Self {
        Self {
            config,
            anchor: Vec::new(),
            window: Vec::new(),
            total_seen: 0,
            shifts: 0,
        }
    }

    /// Add tokens to the context. Triggers a window shift if capacity is exceeded.
    pub fn push_tokens(&mut self, tokens: &[(u32, f32)]) {
        for &(token_id, importance) in tokens {
            let token = WindowToken {
                global_pos: self.total_seen,
                token_id,
                pinned: self.total_seen < self.config.anchor_size,
                importance,
            };

            if token.pinned {
                self.anchor.push(token);
            } else {
                self.window.push(token);
            }

            self.total_seen += 1;
        }

        // Shift window if it exceeds capacity
        let effective_window = self.config.window_size - self.anchor.len();
        if self.window.len() > effective_window {
            self.shift_window(effective_window);
        }
    }

    /// Shift the window: keep overlap + high-importance tokens, evict the rest.
    fn shift_window(&mut self, target_size: usize) {
        self.shifts += 1;

        if self.window.len() <= target_size {
            return;
        }

        let to_evict = self.window.len() - target_size;

        // Strategy: evict from the front (oldest), but rescue high-importance tokens
        // by moving them to the end of the eviction zone.
        // Sort the eviction candidates by importance (ascending = low importance first).
        let (evict_zone, keep_zone) = self.window.split_at(to_evict + self.config.overlap);

        // From evict_zone, rescue tokens above importance threshold
        let threshold = 0.7;
        let mut rescued: Vec<WindowToken> = evict_zone
            .iter()
            .filter(|t| t.importance >= threshold)
            .cloned()
            .collect();

        // Build new window: rescued + overlap + keep
        let overlap_start = if evict_zone.len() > self.config.overlap {
            evict_zone.len() - self.config.overlap
        } else {
            0
        };
        let overlap_tokens = &evict_zone[overlap_start..];

        let mut new_window = Vec::with_capacity(target_size);
        new_window.extend(rescued.drain(..));
        new_window.extend_from_slice(overlap_tokens);
        new_window.extend_from_slice(keep_zone);

        // Truncate if still over budget
        if new_window.len() > target_size {
            let excess = new_window.len() - target_size;
            new_window.drain(..excess);
        }

        self.window = new_window;
    }

    /// Get all active token IDs (anchor + window) in order.
    pub fn active_tokens(&self) -> Vec<u32> {
        let mut tokens = Vec::with_capacity(self.anchor.len() + self.window.len());
        tokens.extend(self.anchor.iter().map(|t| t.token_id));
        tokens.extend(self.window.iter().map(|t| t.token_id));
        tokens
    }

    /// Get the effective context length.
    pub fn context_length(&self) -> usize {
        self.anchor.len() + self.window.len()
    }

    /// Get statistics.
    pub fn stats(&self) -> SlidingWindowStats {
        SlidingWindowStats {
            anchor_tokens: self.anchor.len(),
            window_tokens: self.window.len(),
            total_seen: self.total_seen,
            total_evicted: self.total_seen - self.context_length(),
            shifts: self.shifts,
            utilization: self.context_length() as f32 / self.config.window_size as f32,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SlidingWindowStats {
    pub anchor_tokens: usize,
    pub window_tokens: usize,
    pub total_seen: usize,
    pub total_evicted: usize,
    pub shifts: usize,
    pub utilization: f32,
}

/// Model keep-alive manager — tracks which models should stay warm in memory.
///
/// Ollama's OLLAMA_KEEP_ALIVE keeps models loaded in GPU/system memory
/// between requests. This manager tracks usage patterns to decide which
/// models to keep warm.
pub struct ModelKeepAlive {
    /// Model name → last used timestamp (monotonic ms).
    models: std::collections::HashMap<String, KeepAliveEntry>,
    /// Keep-alive duration (models idle longer are unloaded).
    keep_alive: std::time::Duration,
}

#[derive(Debug, Clone)]
struct KeepAliveEntry {
    last_used: std::time::Instant,
    load_count: u64,
    total_tokens: u64,
}

impl ModelKeepAlive {
    /// Create with a keep-alive duration. Use Duration::MAX for permanent.
    pub fn new(keep_alive: std::time::Duration) -> Self {
        Self {
            models: std::collections::HashMap::new(),
            keep_alive,
        }
    }

    /// Record model usage.
    pub fn touch(&mut self, model: &str, tokens: u64) {
        let entry = self
            .models
            .entry(model.to_string())
            .or_insert(KeepAliveEntry {
                last_used: std::time::Instant::now(),
                load_count: 0,
                total_tokens: 0,
            });
        entry.last_used = std::time::Instant::now();
        entry.load_count += 1;
        entry.total_tokens += tokens;
    }

    /// Check if a model should still be kept alive.
    pub fn is_warm(&self, model: &str) -> bool {
        self.models
            .get(model)
            .map(|e| e.last_used.elapsed() < self.keep_alive)
            .unwrap_or(false)
    }

    /// Get list of models that should be unloaded (expired keep-alive).
    pub fn expired_models(&self) -> Vec<String> {
        self.models
            .iter()
            .filter(|(_, e)| e.last_used.elapsed() >= self.keep_alive)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Evict expired models.
    pub fn evict_expired(&mut self) -> Vec<String> {
        let expired = self.expired_models();
        for name in &expired {
            self.models.remove(name);
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_window_basic() {
        let config = SlidingWindowConfig {
            window_size: 10,
            anchor_size: 2,
            overlap: 1,
        };
        let mut ctx = SlidingWindowContext::new(config);

        // Add 15 tokens (2 anchor + 8 window capacity)
        let tokens: Vec<(u32, f32)> = (0..15).map(|i| (i as u32, 0.1)).collect();
        ctx.push_tokens(&tokens);

        assert_eq!(ctx.anchor.len(), 2);
        assert!(ctx.window.len() <= 8);
        assert!(ctx.shifts > 0);
    }

    #[test]
    fn anchor_tokens_never_evicted() {
        let config = SlidingWindowConfig {
            window_size: 8,
            anchor_size: 3,
            overlap: 1,
        };
        let mut ctx = SlidingWindowContext::new(config);

        let tokens: Vec<(u32, f32)> = (0..20).map(|i| (i as u32, 0.1)).collect();
        ctx.push_tokens(&tokens);

        // Anchor should always have first 3 tokens
        assert_eq!(ctx.anchor.len(), 3);
        assert_eq!(ctx.anchor[0].token_id, 0);
        assert_eq!(ctx.anchor[1].token_id, 1);
        assert_eq!(ctx.anchor[2].token_id, 2);
    }

    #[test]
    fn high_importance_tokens_rescued() {
        let config = SlidingWindowConfig {
            window_size: 6,
            anchor_size: 0,
            overlap: 0,
        };
        let mut ctx = SlidingWindowContext::new(config);

        // Token 2 has high importance
        let tokens: Vec<(u32, f32)> = vec![
            (0, 0.1), (1, 0.1), (2, 0.9), (3, 0.1),
            (4, 0.1), (5, 0.1), (6, 0.1), (7, 0.1),
            (8, 0.1), (9, 0.1),
        ];
        ctx.push_tokens(&tokens);

        let active = ctx.active_tokens();
        // Token 2 should be rescued due to high importance
        assert!(active.contains(&2), "high-importance token should be rescued");
    }

    #[test]
    fn model_keep_alive_warm() {
        let mut ka = ModelKeepAlive::new(std::time::Duration::from_secs(60));
        ka.touch("llama3.2:3b", 100);
        assert!(ka.is_warm("llama3.2:3b"));
        assert!(!ka.is_warm("nonexistent"));
    }
}
