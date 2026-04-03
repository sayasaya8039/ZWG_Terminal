//! PagedAttention — virtual-memory-inspired KV cache management.
//!
//! Ported from vLLM's PagedAttention concept. Instead of allocating a
//! contiguous KV cache per sequence, KV data is stored in fixed-size
//! "pages" (blocks). This eliminates memory fragmentation and enables:
//!
//! 1. **Near-zero waste** — only allocate pages actually used
//! 2. **Shared prefixes** — multiple sequences can reference the same page
//!    (copy-on-write), e.g., system prompts
//! 3. **Dynamic growth** — sequences grow by appending pages, no reallocation
//! 4. **Efficient eviction** — free individual pages, not entire sequences
//!
//! Ref: Kwon et al., "Efficient Memory Management for Large Language Model
//! Serving with PagedAttention" (SOSP 2023)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

// ─── SIMD ヘルパー（AVX2 対応環境で自動適用） ───

/// ページデータの SIMD ゼロ初期化。
/// ページサイズが head_dim の倍数（=32byte アラインメント保証）の場合、
/// AVX2 の 256bit 一括ゼロ書き込みを使用する。
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_zero_fill_f32(ptr: *mut f32, count: usize) {
    #[cfg(target_feature = "avx2")]
    {
        use std::arch::x86_64::{_mm256_setzero_ps, _mm256_store_ps};
        let zero = _mm256_setzero_ps();
        let chunks = count / 8;
        for i in 0..chunks {
            _mm256_store_ps(ptr.add(i * 8), zero);
        }
        // 端数はスカラー処理
        for i in (chunks * 8)..count {
            *ptr.add(i) = 0.0;
        }
        return;
    }
    // AVX2 非対応フォールバック
    #[allow(unreachable_code)]
    {
        std::ptr::write_bytes(ptr, 0, count);
    }
}

/// KV データの SIMD コピー。
/// アラインメント保証が可能な場合 AVX2 を使用し、
/// そうでなければ ptr::copy_nonoverlapping にフォールバック。
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_copy_f32(dst: *mut f32, src: *const f32, count: usize) {
    #[cfg(target_feature = "avx2")]
    {
        use std::arch::x86_64::{_mm256_loadu_ps, _mm256_storeu_ps};
        let chunks = count / 8;
        for i in 0..chunks {
            let vec = _mm256_loadu_ps(src.add(i * 8));
            _mm256_storeu_ps(dst.add(i * 8), vec);
        }
        // 端数
        let remainder_start = chunks * 8;
        if remainder_start < count {
            std::ptr::copy_nonoverlapping(
                src.add(remainder_start),
                dst.add(remainder_start),
                count - remainder_start,
            );
        }
        return;
    }
    // AVX2 非対応フォールバック
    #[allow(unreachable_code)]
    {
        std::ptr::copy_nonoverlapping(src, dst, count);
    }
}

// x86_64 以外のアーキテクチャ向けフォールバック
#[cfg(not(target_arch = "x86_64"))]
#[inline]
unsafe fn simd_zero_fill_f32(ptr: *mut f32, count: usize) {
    std::ptr::write_bytes(ptr, 0, count);
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
unsafe fn simd_copy_f32(dst: *mut f32, src: *const f32, count: usize) {
    std::ptr::copy_nonoverlapping(src, dst, count);
}

/// Number of KV entries per page. Typical: 16 tokens × head_dim values.
const DEFAULT_PAGE_SIZE: usize = 16;

/// Global page ID counter.
static NEXT_PAGE_ID: AtomicU32 = AtomicU32::new(0);

/// A single page in the KV cache — fixed-size block of key-value data.
#[derive(Debug, Clone)]
pub struct KvPage {
    pub id: u32,
    /// Number of valid tokens in this page (0..page_size).
    pub used: usize,
    /// Key data: [used × head_dim] flattened f32.
    pub keys: Vec<f32>,
    /// Value data: [used × head_dim] flattened f32.
    pub values: Vec<f32>,
    /// Reference count for copy-on-write sharing.
    pub ref_count: u32,
}

impl KvPage {
    fn new(head_dim: usize, page_size: usize) -> Self {
        Self {
            id: NEXT_PAGE_ID.fetch_add(1, Ordering::Relaxed),
            used: 0,
            keys: vec![0.0; page_size * head_dim],
            values: vec![0.0; page_size * head_dim],
            ref_count: 1,
        }
    }

    fn is_full(&self, page_size: usize) -> bool {
        self.used >= page_size
    }
}

/// A sequence's KV cache — list of page references.
#[derive(Debug, Clone)]
pub struct SequencePages {
    /// Ordered list of page IDs.
    pub page_ids: Vec<u32>,
    /// Total tokens stored across all pages.
    pub total_tokens: usize,
}

/// PagedAttention KV cache manager.
pub struct PagedKvCache {
    /// All allocated pages.
    pages: HashMap<u32, KvPage>,
    /// Per-sequence page tables.
    sequences: HashMap<u64, SequencePages>,
    /// Free page pool.
    free_pages: Vec<u32>,
    /// Configuration.
    page_size: usize,
    head_dim: usize,
    /// Maximum total pages (memory budget).
    max_pages: usize,
}

impl PagedKvCache {
    /// Create a new paged KV cache.
    ///
    /// `max_pages` × `page_size` × `head_dim` × 2 (K+V) × 4 (f32) = total memory budget.
    pub fn new(head_dim: usize, page_size: usize, max_pages: usize) -> Self {
        Self {
            pages: HashMap::new(),
            sequences: HashMap::new(),
            free_pages: Vec::new(),
            page_size,
            head_dim,
            max_pages,
        }
    }

    /// Create with default page size (16 tokens).
    pub fn with_defaults(head_dim: usize, max_pages: usize) -> Self {
        Self::new(head_dim, DEFAULT_PAGE_SIZE, max_pages)
    }

    /// Allocate a fresh page (or recycle from free pool).
    fn alloc_page(&mut self) -> Option<u32> {
        if let Some(page_id) = self.free_pages.pop() {
            // Recycle: SIMD ゼロ初期化でページをリセット
            if let Some(page) = self.pages.get_mut(&page_id) {
                page.used = 0;
                page.ref_count = 1;
                let key_len = page.keys.len();
                let val_len = page.values.len();
                unsafe {
                    simd_zero_fill_f32(page.keys.as_mut_ptr(), key_len);
                    simd_zero_fill_f32(page.values.as_mut_ptr(), val_len);
                }
            }
            return Some(page_id);
        }

        if self.pages.len() >= self.max_pages {
            return None; // out of memory
        }

        let page = KvPage::new(self.head_dim, self.page_size);
        let id = page.id;
        self.pages.insert(id, page);
        Some(id)
    }

    /// Free a page (decrement ref_count, return to pool if zero).
    fn free_page(&mut self, page_id: u32) {
        if let Some(page) = self.pages.get_mut(&page_id) {
            page.ref_count = page.ref_count.saturating_sub(1);
            if page.ref_count == 0 {
                self.free_pages.push(page_id);
            }
        }
    }

    /// Create a new sequence.
    pub fn create_sequence(&mut self, seq_id: u64) {
        self.sequences.insert(
            seq_id,
            SequencePages {
                page_ids: Vec::new(),
                total_tokens: 0,
            },
        );
    }

    /// Append KV data for new tokens to a sequence.
    /// Returns the number of tokens successfully appended.
    pub fn append(
        &mut self,
        seq_id: u64,
        keys: &[f32],   // [num_tokens × head_dim]
        values: &[f32], // [num_tokens × head_dim]
    ) -> usize {
        let num_tokens = keys.len() / self.head_dim;
        let mut appended = 0;

        for t in 0..num_tokens {
            let k_slice = &keys[t * self.head_dim..(t + 1) * self.head_dim];
            let v_slice = &values[t * self.head_dim..(t + 1) * self.head_dim];

            // Get or allocate current page
            let page_id = {
                let seq = self.sequences.get(&seq_id);
                let last_page = seq.and_then(|s| s.page_ids.last().copied());
                let need_new = last_page
                    .and_then(|id| self.pages.get(&id))
                    .map(|p| p.is_full(self.page_size))
                    .unwrap_or(true);

                if need_new {
                    match self.alloc_page() {
                        Some(new_id) => {
                            if let Some(seq) = self.sequences.get_mut(&seq_id) {
                                seq.page_ids.push(new_id);
                            }
                            new_id
                        }
                        None => break, // OOM
                    }
                } else {
                    last_page.unwrap()
                }
            };

            // Write KV data to the page（SIMD memcpy 最適化）
            if let Some(page) = self.pages.get_mut(&page_id) {
                let offset = page.used * self.head_dim;
                let hd = self.head_dim;
                unsafe {
                    simd_copy_f32(
                        page.keys.as_mut_ptr().add(offset),
                        k_slice.as_ptr(),
                        hd,
                    );
                    simd_copy_f32(
                        page.values.as_mut_ptr().add(offset),
                        v_slice.as_ptr(),
                        hd,
                    );
                }
                page.used += 1;
            }

            if let Some(seq) = self.sequences.get_mut(&seq_id) {
                seq.total_tokens += 1;
            }
            appended += 1;
        }

        appended
    }

    /// Share a prefix of pages from one sequence to another (copy-on-write).
    /// Returns number of shared tokens.
    pub fn share_prefix(&mut self, src_seq: u64, dst_seq: u64, num_pages: usize) -> usize {
        let src_page_ids: Vec<u32> = self
            .sequences
            .get(&src_seq)
            .map(|s| s.page_ids[..num_pages.min(s.page_ids.len())].to_vec())
            .unwrap_or_default();

        let mut shared_tokens = 0;
        for &page_id in &src_page_ids {
            if let Some(page) = self.pages.get_mut(&page_id) {
                page.ref_count += 1;
                shared_tokens += page.used;
            }
        }

        if let Some(dst) = self.sequences.get_mut(&dst_seq) {
            dst.page_ids.extend_from_slice(&src_page_ids);
            dst.total_tokens += shared_tokens;
        }

        shared_tokens
    }

    /// Delete a sequence, freeing its pages.
    pub fn delete_sequence(&mut self, seq_id: u64) {
        if let Some(seq) = self.sequences.remove(&seq_id) {
            for page_id in seq.page_ids {
                self.free_page(page_id);
            }
        }
    }

    /// Read all KV data for a sequence (concatenated).
    pub fn read_kv(&self, seq_id: u64) -> Option<(Vec<f32>, Vec<f32>)> {
        let seq = self.sequences.get(&seq_id)?;
        let total = seq.total_tokens;
        let mut keys = Vec::with_capacity(total * self.head_dim);
        let mut values = Vec::with_capacity(total * self.head_dim);

        // SIMD バルクコピーで KV データを結合
        let mut k_offset = 0usize;
        let mut v_offset = 0usize;
        // 事前に正確なサイズを確保（resize で初期化済み領域を作る）
        keys.resize(total * self.head_dim, 0.0);
        values.resize(total * self.head_dim, 0.0);

        for &page_id in &seq.page_ids {
            if let Some(page) = self.pages.get(&page_id) {
                let count = page.used * self.head_dim;
                unsafe {
                    simd_copy_f32(
                        keys.as_mut_ptr().add(k_offset),
                        page.keys.as_ptr(),
                        count,
                    );
                    simd_copy_f32(
                        values.as_mut_ptr().add(v_offset),
                        page.values.as_ptr(),
                        count,
                    );
                }
                k_offset += count;
                v_offset += count;
            }
        }

        Some((keys, values))
    }

    /// Total memory used (bytes).
    pub fn memory_bytes(&self) -> usize {
        self.pages.len() * self.page_size * self.head_dim * 2 * 4
    }

    /// Memory utilization (fraction of allocated pages that are used).
    pub fn utilization(&self) -> f32 {
        let total_slots: usize = self.pages.len() * self.page_size;
        let used_slots: usize = self.pages.values().map(|p| p.used).sum();
        if total_slots == 0 {
            return 0.0;
        }
        used_slots as f32 / total_slots as f32
    }

    /// Number of free pages available.
    pub fn free_page_count(&self) -> usize {
        self.free_pages.len() + (self.max_pages - self.pages.len())
    }

    /// Stats snapshot.
    pub fn stats(&self) -> PagedCacheStats {
        PagedCacheStats {
            total_pages: self.pages.len(),
            free_pages: self.free_page_count(),
            max_pages: self.max_pages,
            sequences: self.sequences.len(),
            total_tokens: self.sequences.values().map(|s| s.total_tokens).sum(),
            memory_bytes: self.memory_bytes(),
            utilization: self.utilization(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PagedCacheStats {
    pub total_pages: usize,
    pub free_pages: usize,
    pub max_pages: usize,
    pub sequences: usize,
    pub total_tokens: usize,
    pub memory_bytes: usize,
    pub utilization: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_append() {
        let mut cache = PagedKvCache::with_defaults(4, 100);
        cache.create_sequence(1);
        let keys = vec![1.0; 4 * 5]; // 5 tokens, head_dim=4
        let values = vec![2.0; 4 * 5];
        let appended = cache.append(1, &keys, &values);
        assert_eq!(appended, 5);

        let (k, v) = cache.read_kv(1).unwrap();
        assert_eq!(k.len(), 20);
        assert_eq!(v.len(), 20);
        assert!((k[0] - 1.0).abs() < 1e-6);
        assert!((v[0] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn page_overflow_allocates_new_page() {
        let mut cache = PagedKvCache::new(2, 4, 100); // page_size=4
        cache.create_sequence(1);
        // Append 6 tokens (requires 2 pages of size 4)
        let keys = vec![1.0; 2 * 6];
        let values = vec![2.0; 2 * 6];
        cache.append(1, &keys, &values);

        let seq = cache.sequences.get(&1).unwrap();
        assert_eq!(seq.page_ids.len(), 2);
        assert_eq!(seq.total_tokens, 6);
    }

    #[test]
    fn share_prefix_increments_refcount() {
        let mut cache = PagedKvCache::new(2, 4, 100);
        cache.create_sequence(1);
        cache.append(1, &vec![1.0; 2 * 4], &vec![2.0; 2 * 4]);

        cache.create_sequence(2);
        let shared = cache.share_prefix(1, 2, 1); // share 1 page
        assert_eq!(shared, 4);

        let page_id = cache.sequences.get(&1).unwrap().page_ids[0];
        assert_eq!(cache.pages.get(&page_id).unwrap().ref_count, 2);
    }

    #[test]
    fn delete_sequence_frees_pages() {
        let mut cache = PagedKvCache::with_defaults(4, 100);
        cache.create_sequence(1);
        cache.append(1, &vec![1.0; 4 * 20], &vec![2.0; 4 * 20]);
        let pages_before = cache.pages.len();
        cache.delete_sequence(1);
        assert_eq!(cache.free_pages.len(), pages_before);
    }

    #[test]
    fn oom_stops_appending() {
        let mut cache = PagedKvCache::new(2, 4, 1); // only 1 page allowed
        cache.create_sequence(1);
        let appended = cache.append(1, &vec![1.0; 2 * 8], &vec![2.0; 2 * 8]);
        assert_eq!(appended, 4); // only 1 page worth
    }
}
