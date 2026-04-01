//! Context Manager — smart scrollback compression inspired by Ollama's context shifting.
//!
//! Key concepts ported from Ollama:
//! - Context shifting: discard old tokens while keeping important prefix
//! - Priority-based retention: errors/warnings/commands are kept longer
//! - Summarization-based compression: replace verbose output with summaries

use std::collections::HashMap;

use crate::classifier::LineKind;
use crate::summarizer;

/// Importance score for each line kind.
/// Higher scores mean the line is retained longer during eviction.
fn default_importance() -> HashMap<LineKind, f32> {
    let mut m = HashMap::new();
    m.insert(LineKind::Error, 1.0);
    m.insert(LineKind::StackTrace, 0.9);
    m.insert(LineKind::Warning, 0.8);
    m.insert(LineKind::Command, 0.7);
    m.insert(LineKind::FilePath, 0.6);
    m.insert(LineKind::Diff, 0.5);
    m.insert(LineKind::Url, 0.4);
    m.insert(LineKind::Json, 0.3);
    m.insert(LineKind::Table, 0.3);
    m.insert(LineKind::Markdown, 0.2);
    m.insert(LineKind::Text, 0.1);
    m
}

/// Metadata for a single indexed line.
#[derive(Debug, Clone)]
pub struct LineMeta {
    /// Global line number in scrollback.
    pub line_id: usize,
    /// Classification of the line.
    pub kind: LineKind,
    /// Timestamp (monotonic counter or epoch millis).
    pub timestamp: u64,
}

/// Context manager that implements importance-based eviction for scrollback.
///
/// Analogous to Ollama's context shifting — instead of blindly discarding the
/// oldest tokens, we keep important lines (errors, commands) longer and evict
/// low-importance lines (plain text) first.
pub struct ContextManager {
    /// Maximum number of indexed lines before eviction triggers.
    max_lines: usize,
    /// Number of "important" lines at the head to always keep (Ollama's numKeep).
    num_keep: usize,
    /// Importance scores by line kind.
    importance: HashMap<LineKind, f32>,
}

impl ContextManager {
    /// Create a new ContextManager.
    ///
    /// - `max_lines`: capacity limit before eviction starts.
    /// - `num_keep`: number of head lines that are never evicted (like Ollama's numKeep).
    pub fn new(max_lines: usize, num_keep: usize) -> Self {
        Self {
            max_lines,
            num_keep,
            importance: default_importance(),
        }
    }

    /// Return the importance score for a given line kind.
    pub fn classify_importance(&self, kind: LineKind) -> f32 {
        self.importance.get(&kind).copied().unwrap_or(0.1)
    }

    /// Maximum capacity before eviction triggers.
    pub fn max_lines(&self) -> usize {
        self.max_lines
    }

    /// Number of head lines protected from eviction.
    pub fn num_keep(&self) -> usize {
        self.num_keep
    }

    /// Select which lines to evict when the index exceeds capacity.
    ///
    /// Returns indices into `lines` that should be removed, sorted ascending.
    /// The first `num_keep` entries are never evicted.
    /// Among the remaining lines, the lowest-importance (then oldest) are chosen first.
    pub fn select_eviction_candidates(
        &self,
        lines: &[LineMeta],
        need_free: usize,
    ) -> Vec<usize> {
        if need_free == 0 || lines.len() <= self.num_keep {
            return Vec::new();
        }

        // Build (index, score) for eviction-eligible lines (skip first num_keep)
        let start = self.num_keep.min(lines.len());
        let mut candidates: Vec<(usize, f32, u64)> = lines[start..]
            .iter()
            .enumerate()
            .map(|(i, meta)| {
                let idx = start + i;
                let score = self.classify_importance(meta.kind);
                (idx, score, meta.timestamp)
            })
            .collect();

        // Sort by importance ascending, then by timestamp ascending (oldest first)
        candidates.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.2.cmp(&b.2))
        });

        let count = need_free.min(candidates.len());
        let mut result: Vec<usize> = candidates[..count].iter().map(|(idx, _, _)| *idx).collect();
        result.sort_unstable();
        result
    }

    /// Compress a contiguous region of plain-text lines into a summary.
    ///
    /// Uses the rule-based summarizer to produce a one-line summary.
    /// Example: 100 lines of build output → "Build output: 47/47 tests passed"
    pub fn compress_region(&self, lines: &[String]) -> String {
        if lines.is_empty() {
            return String::new();
        }

        if lines.len() == 1 {
            return lines[0].clone();
        }

        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let summary = summarizer::summarize(&refs);

        if summary.text.is_empty() {
            // Fallback: line count
            format!("[{} lines compressed]", lines.len())
        } else {
            format!("[{}] ({}L)", summary.text, lines.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_importance_scores() {
        let cm = ContextManager::new(1000, 10);
        assert!((cm.classify_importance(LineKind::Error) - 1.0).abs() < f32::EPSILON);
        assert!((cm.classify_importance(LineKind::StackTrace) - 0.9).abs() < f32::EPSILON);
        assert!((cm.classify_importance(LineKind::Warning) - 0.8).abs() < f32::EPSILON);
        assert!((cm.classify_importance(LineKind::Command) - 0.7).abs() < f32::EPSILON);
        assert!((cm.classify_importance(LineKind::FilePath) - 0.6).abs() < f32::EPSILON);
        assert!((cm.classify_importance(LineKind::Text) - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn eviction_respects_num_keep() {
        let cm = ContextManager::new(5, 3);
        let lines = vec![
            LineMeta { line_id: 0, kind: LineKind::Text, timestamp: 0 },
            LineMeta { line_id: 1, kind: LineKind::Text, timestamp: 1 },
            LineMeta { line_id: 2, kind: LineKind::Text, timestamp: 2 },
            LineMeta { line_id: 3, kind: LineKind::Text, timestamp: 3 },
            LineMeta { line_id: 4, kind: LineKind::Text, timestamp: 4 },
            LineMeta { line_id: 5, kind: LineKind::Text, timestamp: 5 },
            LineMeta { line_id: 6, kind: LineKind::Text, timestamp: 6 },
        ];

        let victims = cm.select_eviction_candidates(&lines, 2);
        assert_eq!(victims.len(), 2);
        // First 3 (num_keep) should never be evicted
        assert!(victims.iter().all(|&idx| idx >= 3));
    }

    #[test]
    fn eviction_prefers_low_importance() {
        let cm = ContextManager::new(5, 0);
        let lines = vec![
            LineMeta { line_id: 0, kind: LineKind::Error, timestamp: 0 },
            LineMeta { line_id: 1, kind: LineKind::Text, timestamp: 1 },
            LineMeta { line_id: 2, kind: LineKind::Warning, timestamp: 2 },
            LineMeta { line_id: 3, kind: LineKind::Text, timestamp: 3 },
            LineMeta { line_id: 4, kind: LineKind::Command, timestamp: 4 },
        ];

        let victims = cm.select_eviction_candidates(&lines, 2);
        assert_eq!(victims.len(), 2);
        // Text lines (importance 0.1) should be evicted first
        assert!(victims.contains(&1));
        assert!(victims.contains(&3));
    }

    #[test]
    fn eviction_oldest_first_when_same_importance() {
        let cm = ContextManager::new(5, 0);
        let lines = vec![
            LineMeta { line_id: 0, kind: LineKind::Text, timestamp: 10 },
            LineMeta { line_id: 1, kind: LineKind::Text, timestamp: 5 },
            LineMeta { line_id: 2, kind: LineKind::Text, timestamp: 20 },
            LineMeta { line_id: 3, kind: LineKind::Text, timestamp: 1 },
        ];

        let victims = cm.select_eviction_candidates(&lines, 2);
        assert_eq!(victims.len(), 2);
        // Oldest timestamps (1, 5) → indices 3 and 1
        assert!(victims.contains(&3));
        assert!(victims.contains(&1));
    }

    #[test]
    fn eviction_zero_need_free() {
        let cm = ContextManager::new(100, 10);
        let lines = vec![
            LineMeta { line_id: 0, kind: LineKind::Text, timestamp: 0 },
        ];
        let victims = cm.select_eviction_candidates(&lines, 0);
        assert!(victims.is_empty());
    }

    #[test]
    fn compress_empty() {
        let cm = ContextManager::new(100, 10);
        assert_eq!(cm.compress_region(&[]), "");
    }

    #[test]
    fn compress_single_line() {
        let cm = ContextManager::new(100, 10);
        let lines = vec!["hello world".to_string()];
        assert_eq!(cm.compress_region(&lines), "hello world");
    }

    #[test]
    fn compress_build_output() {
        let cm = ContextManager::new(100, 10);
        let lines = vec![
            "   Compiling smux v0.8.19".to_string(),
            "   Compiling smux-core v0.8.19".to_string(),
            "    Finished `release` profile [optimized] target(s) in 20.10s".to_string(),
        ];
        let result = cm.compress_region(&lines);
        assert!(!result.is_empty());
        assert!(result.contains("3L") || result.contains("Finished"));
    }

    #[test]
    fn compress_test_output() {
        let cm = ContextManager::new(100, 10);
        let lines = vec![
            "running 47 tests".to_string(),
            "test result: ok. 47 passed; 0 failed; 0 ignored".to_string(),
        ];
        let result = cm.compress_region(&lines);
        assert!(result.contains("47") || result.contains("2L"));
    }

    #[test]
    fn compress_plain_text_fallback() {
        let cm = ContextManager::new(100, 10);
        let lines: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let result = cm.compress_region(&lines);
        // Should either summarize or show "[10 lines compressed]"
        assert!(result.contains("10") || result.contains("line"));
    }
}
