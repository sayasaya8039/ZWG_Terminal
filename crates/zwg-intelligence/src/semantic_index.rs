use anyhow::Result;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::classifier::{LineKind, classify_line};
use crate::context_manager::{ContextManager, LineMeta};
use crate::embeddings::{EmbeddingEngine, cosine_similarity};

/// Newtype wrapper for f32 that implements Ord for use in BinaryHeap.
/// NaN is treated as less than all other values.
#[derive(Clone, Copy, PartialEq)]
struct OrderedF32(f32);

impl Eq for OrderedF32 {}

impl PartialOrd for OrderedF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(std::cmp::Ordering::Less)
    }
}

/// A single indexed line with its embedding and classification.
#[derive(Clone)]
struct IndexedLine {
    line_number: usize,
    text: String,
    embedding: Vec<f32>,
    kind: LineKind,
    timestamp: u64,
}

/// Result of a semantic search.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub line_number: usize,
    pub text: String,
    pub score: f32,
}

/// Semantic index over terminal scrollback lines.
///
/// Lines are embedded in batches for efficiency.
/// Search returns top-K results by cosine similarity.
/// Eviction uses importance-based strategy via `ContextManager`.
pub struct SemanticIndex {
    engine: Arc<EmbeddingEngine>,
    lines: Mutex<VecDeque<IndexedLine>>,
    pending: Mutex<Vec<(usize, String)>>,
    max_lines: usize,
    context_mgr: ContextManager,
    /// Monotonic counter for line timestamps.
    ts_counter: Mutex<u64>,
}

impl SemanticIndex {
    pub fn new(engine: Arc<EmbeddingEngine>, max_lines: usize) -> Self {
        // Keep first 10% of lines as "important prefix" (like Ollama's numKeep)
        let num_keep = (max_lines / 10).max(1);
        Self {
            engine,
            lines: Mutex::new(VecDeque::new()),
            pending: Mutex::new(Vec::new()),
            max_lines,
            context_mgr: ContextManager::new(max_lines, num_keep),
            ts_counter: Mutex::new(0),
        }
    }

    /// Allocate a monotonically increasing timestamp.
    fn next_ts(&self) -> u64 {
        let mut ts = self.ts_counter.lock();
        let val = *ts;
        *ts = val + 1;
        val
    }

    /// Queue a line for background embedding.
    pub fn add_line(&self, line_number: usize, text: String) {
        if text.trim().len() < 3 {
            return;
        }
        self.pending.lock().push((line_number, text));
    }

    /// Queue multiple lines at once.
    pub fn add_lines(&self, lines: impl IntoIterator<Item = (usize, String)>) {
        let mut pending = self.pending.lock();
        for (num, text) in lines {
            if text.trim().len() >= 3 {
                pending.push((num, text));
            }
        }
    }

    /// Process pending lines: embed them and add to the index.
    /// Call this periodically from a background thread.
    pub fn flush_pending(&self) -> Result<usize> {
        let batch: Vec<(usize, String)> = {
            let mut pending = self.pending.lock();
            std::mem::take(&mut *pending)
        };

        if batch.is_empty() {
            return Ok(0);
        }

        let chunk_size = 32;
        let mut total_processed = 0;

        for chunk in batch.chunks(chunk_size) {
            let texts: Vec<&str> = chunk.iter().map(|(_, t)| t.as_str()).collect();
            let embeddings = self.engine.embed_batch(&texts)?;

            let mut lines = self.lines.lock();
            for ((line_num, text), embedding) in chunk.iter().zip(embeddings) {
                let kind = classify_line(text).kind;
                let timestamp = self.next_ts();
                lines.push_back(IndexedLine {
                    line_number: *line_num,
                    text: text.clone(),
                    embedding,
                    kind,
                    timestamp,
                });
            }

            // Importance-based eviction (replaces simple FIFO pop_front)
            if lines.len() > self.max_lines {
                let need_free = lines.len() - self.max_lines;
                let metas: Vec<LineMeta> = lines
                    .iter()
                    .enumerate()
                    .map(|(_, line)| LineMeta {
                        line_id: line.line_number,
                        kind: line.kind,
                        timestamp: line.timestamp,
                    })
                    .collect();
                let victims = self.context_mgr.select_eviction_candidates(&metas, need_free);
                // Remove in reverse order to preserve indices
                for &idx in victims.iter().rev() {
                    lines.remove(idx);
                }
            }

            total_processed += chunk.len();
        }

        log::debug!("Indexed {} scrollback lines", total_processed);
        Ok(total_processed)
    }

    /// Search the index for lines semantically similar to the query.
    /// Snapshot under lock, then score without holding the lock.
    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<SearchResult>> {
        let query_text = format!("query: {}", query);
        let query_embedding = self.engine.embed_one(&query_text)?;

        // Snapshot lines under lock, release immediately
        let snapshot: Vec<IndexedLine> = {
            let lines = self.lines.lock();
            lines.iter().cloned().collect()
        };

        if snapshot.is_empty() {
            return Ok(Vec::new());
        }

        // Score without holding lock — use min-heap for O(n log k) top-K extraction
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        let k = top_k.min(snapshot.len());
        let mut heap: BinaryHeap<Reverse<(OrderedF32, usize)>> = BinaryHeap::with_capacity(k + 1);

        for (idx, line) in snapshot.iter().enumerate() {
            let score = cosine_similarity(&query_embedding, &line.embedding);
            let entry = Reverse((OrderedF32(score), idx));

            if heap.len() < k {
                heap.push(entry);
            } else if let Some(&Reverse((OrderedF32(min_score), _))) = heap.peek() {
                if score > min_score {
                    heap.pop();
                    heap.push(entry);
                }
            }
        }

        let mut results: Vec<SearchResult> = heap
            .into_sorted_vec()
            .into_iter()
            .map(|Reverse((OrderedF32(score), idx))| {
                let line = &snapshot[idx];
                SearchResult {
                    line_number: line.line_number,
                    text: line.text.clone(),
                    score,
                }
            })
            .collect();

        // into_sorted_vec returns ascending; we want descending (highest score first)
        results.reverse();

        Ok(results)
    }

    pub fn indexed_count(&self) -> usize {
        self.lines.lock().len()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }

    pub fn clear(&self) {
        self.lines.lock().clear();
        self.pending.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_result_fields() {
        let result = SearchResult {
            line_number: 42,
            text: "hello world".to_string(),
            score: 0.95,
        };
        assert_eq!(result.line_number, 42);
        assert_eq!(result.text, "hello world");
        assert!((result.score - 0.95).abs() < 1e-6);
    }
}
