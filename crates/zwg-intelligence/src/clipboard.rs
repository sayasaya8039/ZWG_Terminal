//! Smart clipboard with classification-based grouping.
//!
//! Clipboard entries are classified using the rule-based classifier (Phase 2).
//! Entries are grouped by LineKind, and ranked by recency + context relevance.
//! When embedding tokenizer becomes available, cosine similarity ranking
//! will replace the rule-based approach.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::Instant;

use crate::classifier::{LineClassification, LineKind, classify_line};

/// Maximum clipboard history entries.
const MAX_HISTORY: usize = 100;

/// A clipboard entry with classification metadata.
#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    pub text: String,
    pub classification: LineClassification,
    pub timestamp: Instant,
    pub paste_count: u32,
}

/// A group of clipboard entries sharing the same LineKind.
#[derive(Debug, Clone)]
pub struct ClipboardGroup {
    pub kind: LineKind,
    pub entries: Vec<usize>, // indices into SmartClipboard::history
}

/// Smart clipboard that classifies and groups copied text.
pub struct SmartClipboard {
    history: Mutex<Vec<ClipboardEntry>>,
}

impl SmartClipboard {
    pub fn new() -> Self {
        Self {
            history: Mutex::new(Vec::new()),
        }
    }

    /// Record a new clipboard copy. Classifies the first line for grouping.
    pub fn push(&self, text: String) {
        if text.trim().is_empty() {
            return;
        }

        let first_line = text.lines().next().unwrap_or(&text);
        let classification = classify_line(first_line);

        let mut history = self.history.lock();

        // Deduplicate: if the same text was copied recently, update timestamp
        if let Some(existing) = history.iter_mut().find(|e| e.text == text) {
            existing.timestamp = Instant::now();
            return;
        }

        history.push(ClipboardEntry {
            text,
            classification,
            timestamp: Instant::now(),
            paste_count: 0,
        });

        // Evict oldest
        while history.len() > MAX_HISTORY {
            // Remove least recently used (oldest timestamp, lowest paste_count)
            let min_idx = history
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    a.paste_count
                        .cmp(&b.paste_count)
                        .then_with(|| a.timestamp.cmp(&b.timestamp))
                })
                .map(|(i, _)| i)
                .unwrap_or(0);
            history.remove(min_idx);
        }
    }

    /// Record that an entry was pasted (for ranking).
    pub fn record_paste(&self, text: &str) {
        let mut history = self.history.lock();
        if let Some(entry) = history.iter_mut().find(|e| e.text == text) {
            entry.paste_count += 1;
            entry.timestamp = Instant::now();
        }
    }

    /// Get all entries grouped by LineKind, most recent first within each group.
    pub fn groups(&self) -> Vec<ClipboardGroup> {
        let history = self.history.lock();
        let mut groups: HashMap<LineKind, Vec<usize>> = HashMap::new();

        for (idx, entry) in history.iter().enumerate() {
            groups
                .entry(entry.classification.kind)
                .or_default()
                .push(idx);
        }

        let mut result: Vec<ClipboardGroup> = groups
            .into_iter()
            .map(|(kind, mut entries)| {
                // Sort by recency (newest first)
                entries.sort_by(|&a, &b| {
                    history[b].timestamp.cmp(&history[a].timestamp)
                });
                ClipboardGroup { kind, entries }
            })
            .collect();

        // Sort groups: Error/Warning first, then by entry count descending
        result.sort_by(|a, b| {
            let a_priority = group_priority(a.kind);
            let b_priority = group_priority(b.kind);
            a_priority
                .cmp(&b_priority)
                .then_with(|| b.entries.len().cmp(&a.entries.len()))
        });

        result
    }

    /// Get the top-K most relevant entries for the current context.
    /// Context is a string (e.g., current terminal line or command).
    /// Ranking: same LineKind > recent > frequently pasted.
    pub fn suggest(&self, context: &str, top_k: usize) -> Vec<ClipboardEntry> {
        let context_kind = classify_line(context).kind;
        let history = self.history.lock();

        let mut scored: Vec<(usize, f32)> = history
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let mut score = 0.0f32;

                // Same kind bonus
                if entry.classification.kind == context_kind {
                    score += 0.5;
                }

                // Recency bonus (decay over time)
                let age_secs = entry.timestamp.elapsed().as_secs_f32();
                score += 1.0 / (1.0 + age_secs / 60.0); // half-life ~1 min

                // Paste frequency bonus
                score += (entry.paste_count as f32).min(5.0) * 0.1;

                (idx, score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scored
            .into_iter()
            .take(top_k)
            .map(|(idx, _)| history[idx].clone())
            .collect()
    }

    /// Get recent entries (most recent first).
    pub fn recent(&self, limit: usize) -> Vec<ClipboardEntry> {
        let history = self.history.lock();
        let mut entries: Vec<ClipboardEntry> = history.iter().cloned().collect();
        entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        entries.truncate(limit);
        entries
    }

    /// Number of entries in history.
    pub fn len(&self) -> usize {
        self.history.lock().len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.history.lock().is_empty()
    }

    /// Clear all history.
    pub fn clear(&self) {
        self.history.lock().clear();
    }
}

/// Priority order for group display (lower = shown first).
fn group_priority(kind: LineKind) -> u8 {
    match kind {
        LineKind::Error => 0,
        LineKind::Warning => 1,
        LineKind::Command => 2,
        LineKind::FilePath => 3,
        LineKind::Url => 4,
        LineKind::StackTrace => 5,
        LineKind::Json => 6,
        LineKind::Diff => 7,
        LineKind::Table => 8,
        LineKind::Markdown => 9,
        LineKind::Text => 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_retrieve() {
        let cb = SmartClipboard::new();
        cb.push("https://example.com".to_string());
        cb.push("error: something failed".to_string());
        cb.push("$ cargo build".to_string());
        assert_eq!(cb.len(), 3);
    }

    #[test]
    fn deduplication() {
        let cb = SmartClipboard::new();
        cb.push("hello".to_string());
        cb.push("hello".to_string());
        assert_eq!(cb.len(), 1);
    }

    #[test]
    fn groups_by_kind() {
        let cb = SmartClipboard::new();
        cb.push("https://a.com".to_string());
        cb.push("https://b.com".to_string());
        cb.push("error: fail".to_string());
        cb.push("plain text".to_string());

        let groups = cb.groups();
        assert!(groups.len() >= 2);
        // Error group should come first
        assert_eq!(groups[0].kind, LineKind::Error);
    }

    #[test]
    fn suggest_context_relevance() {
        let cb = SmartClipboard::new();
        cb.push("https://docs.rs".to_string());
        cb.push("error: type mismatch".to_string());
        cb.push("$ npm install".to_string());

        let suggestions = cb.suggest("error: another error", 2);
        assert!(!suggestions.is_empty());
        // Error context should rank error entries higher
        assert_eq!(suggestions[0].classification.kind, LineKind::Error);
    }

    #[test]
    fn recent_order() {
        let cb = SmartClipboard::new();
        cb.push("first".to_string());
        cb.push("second".to_string());
        cb.push("third".to_string());

        let recent = cb.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].text, "third");
        assert_eq!(recent[1].text, "second");
    }

    #[test]
    fn empty_text_ignored() {
        let cb = SmartClipboard::new();
        cb.push("".to_string());
        cb.push("   ".to_string());
        assert_eq!(cb.len(), 0);
    }

    #[test]
    fn eviction_at_max() {
        let cb = SmartClipboard::new();
        for i in 0..150 {
            cb.push(format!("entry {i}"));
        }
        assert!(cb.len() <= MAX_HISTORY);
    }

    #[test]
    fn record_paste_boosts() {
        let cb = SmartClipboard::new();
        cb.push("important".to_string());
        cb.push("not important".to_string());
        cb.record_paste("important");
        cb.record_paste("important");

        let suggestions = cb.suggest("anything", 1);
        assert_eq!(suggestions[0].text, "important");
    }
}
