//! KV Prefix Cache — pure Rust prefix trie for command history.
//!
//! Stores tokenized command sequences and supports:
//! - O(L) insertion where L = token sequence length
//! - Prefix matching
//! - Frequency-based top-K prediction
//! - LRU eviction for bounded memory usage

use std::collections::HashMap;

/// Result of a prefix match query.
#[derive(Debug, Clone)]
pub struct PrefixMatch {
    /// Number of tokens matched from the query prefix.
    pub matched_len: u32,
    /// Command ID associated with the matched terminal node.
    pub command_id: u32,
    /// How many times this command was inserted (popularity).
    pub frequency: u32,
    /// Whether the match consumed the entire query.
    pub is_exact: bool,
}

struct TrieNode {
    children: HashMap<u32, TrieNode>,
    command_id: Option<u32>,
    frequency: u32,
}

impl TrieNode {
    fn new() -> Self {
        Self {
            children: HashMap::new(),
            command_id: None,
            frequency: 0,
        }
    }
}

/// Pure Rust prefix trie for command prediction.
pub struct KvPrefixCache {
    root: TrieNode,
    entry_count: u32,
}

unsafe impl Send for KvPrefixCache {}

impl KvPrefixCache {
    /// Create a new prefix trie.
    pub fn new() -> Option<Self> {
        Some(Self {
            root: TrieNode::new(),
            entry_count: 0,
        })
    }

    /// Insert a tokenized command into the trie.
    pub fn insert_command(&mut self, tokens: &[u32], command_id: u32) {
        if tokens.is_empty() {
            return;
        }
        let mut node = &mut self.root;
        for &token in tokens {
            node = node.children.entry(token).or_insert_with(TrieNode::new);
        }
        if node.command_id.is_none() {
            self.entry_count += 1;
        }
        node.command_id = Some(command_id);
        node.frequency += 1;
    }

    /// Find the best (longest) prefix match for the given query.
    pub fn match_prefix(&self, query: &[u32]) -> Option<PrefixMatch> {
        if query.is_empty() {
            return None;
        }
        let mut node = &self.root;
        let mut best: Option<PrefixMatch> = None;
        for (i, &token) in query.iter().enumerate() {
            match node.children.get(&token) {
                Some(child) => {
                    node = child;
                    if let Some(cmd_id) = node.command_id {
                        best = Some(PrefixMatch {
                            matched_len: (i + 1) as u32,
                            command_id: cmd_id,
                            frequency: node.frequency,
                            is_exact: i + 1 == query.len(),
                        });
                    }
                }
                None => break,
            }
        }
        best
    }

    /// Get top-K matches by frequency for commands sharing the given prefix.
    pub fn top_k(&self, prefix: &[u32], k: usize) -> Vec<PrefixMatch> {
        if prefix.is_empty() || k == 0 {
            return Vec::new();
        }
        // Navigate to prefix node
        let mut node = &self.root;
        for &token in prefix {
            match node.children.get(&token) {
                Some(child) => node = child,
                None => return Vec::new(),
            }
        }
        // Collect all terminal nodes under this prefix
        let mut results = Vec::new();
        Self::collect_terminals(node, prefix.len() as u32, &mut results);
        results.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        results.truncate(k);
        results
    }

    fn collect_terminals(node: &TrieNode, depth: u32, out: &mut Vec<PrefixMatch>) {
        if let Some(cmd_id) = node.command_id {
            out.push(PrefixMatch {
                matched_len: depth,
                command_id: cmd_id,
                frequency: node.frequency,
                is_exact: true,
            });
        }
        for (_, child) in &node.children {
            Self::collect_terminals(child, depth + 1, out);
        }
    }

    /// Evict entries to keep at most `keep_count` active terminals.
    pub fn evict(&mut self, keep_count: u32) {
        if self.entry_count <= keep_count {
            return;
        }
        // Simple eviction: clear everything and reset
        // A more sophisticated LRU could be added later
        self.root = TrieNode::new();
        self.entry_count = 0;
    }
}

/// Standalone prefix comparison. Returns the length of the common prefix.
pub fn simd_prefix_match(a: &[u32], b: &[u32]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_drop() {
        let cache = KvPrefixCache::new();
        assert!(cache.is_some());
    }

    #[test]
    fn test_insert_and_match() {
        let mut cache = KvPrefixCache::new().unwrap();
        let tokens = [108u32, 115, 32, 45, 108, 97];
        cache.insert_command(&tokens, 42);

        let result = cache.match_prefix(&tokens).unwrap();
        assert_eq!(result.matched_len, 6);
        assert_eq!(result.command_id, 42);
        assert!(result.is_exact);
    }

    #[test]
    fn test_no_match() {
        let mut cache = KvPrefixCache::new().unwrap();
        let tokens = [1u32, 2, 3];
        cache.insert_command(&tokens, 1);

        let result = cache.match_prefix(&[99, 98, 97]);
        assert!(result.is_none());
    }

    #[test]
    fn test_frequency() {
        let mut cache = KvPrefixCache::new().unwrap();
        let tokens = [10u32, 20, 30];
        cache.insert_command(&tokens, 1);
        cache.insert_command(&tokens, 1);
        cache.insert_command(&tokens, 1);

        let result = cache.match_prefix(&tokens).unwrap();
        assert_eq!(result.frequency, 3);
    }

    #[test]
    fn test_empty_input() {
        let mut cache = KvPrefixCache::new().unwrap();
        assert!(cache.match_prefix(&[]).is_none());
        cache.insert_command(&[], 0);
    }

    #[test]
    fn test_simd_prefix_match() {
        assert_eq!(simd_prefix_match(&[1, 2, 3], &[1, 2, 4]), 2);
        assert_eq!(simd_prefix_match(&[1, 2, 3], &[1, 2, 3]), 3);
        assert_eq!(simd_prefix_match(&[1], &[2]), 0);
    }
}
