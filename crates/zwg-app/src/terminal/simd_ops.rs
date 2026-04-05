//! SIMD-accelerated terminal hot paths (x86_64 AVX2)
//!
//! Four vectorized operations:
//! 1. VT parser: fast escape/control byte scanning
//! 2. Grid update: dirty cell detection via bitmap
//! 3. UTF-8 → glyph: ASCII fast-path cell-to-char index
//! 4. Color packing: batch RGB→RGBA alpha insertion

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

// ── 1. VT Parser: Escape byte scanning ──────────────────────────
// Scan buffer for first ESC (0x1B) byte. Returns index or data.len().
// Used to partition VT output into "normal text" and "escape sequences".

/// Find the first ESC (0x1B) byte in the buffer using SIMD.
/// Returns `data.len()` if no ESC found.
#[cfg(target_arch = "x86_64")]
pub fn find_escape(data: &[u8]) -> usize {
    if data.len() >= 32 && is_x86_feature_detected!("avx2") {
        unsafe { find_escape_avx2(data) }
    } else {
        find_escape_scalar(data)
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn find_escape(data: &[u8]) -> usize {
    find_escape_scalar(data)
}

fn find_escape_scalar(data: &[u8]) -> usize {
    memchr_single(data, 0x1B)
}

/// Find first occurrence of a byte (branchless scalar with unrolling).
#[inline]
fn memchr_single(data: &[u8], needle: u8) -> usize {
    for (i, &b) in data.iter().enumerate() {
        if b == needle {
            return i;
        }
    }
    data.len()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn find_escape_avx2(data: &[u8]) -> usize {
    let len = data.len();
    let ptr = data.as_ptr();
    let mut pos = 0usize;
    let needle = _mm256_set1_epi8(0x1Bu8 as i8);

    while pos + 32 <= len {
        let chunk = _mm256_loadu_si256(ptr.add(pos) as *const __m256i);
        let eq = _mm256_cmpeq_epi8(chunk, needle);
        let mask = _mm256_movemask_epi8(eq) as u32;
        if mask != 0 {
            return pos + mask.trailing_zeros() as usize;
        }
        pos += 32;
    }
    // Scalar tail
    while pos < len {
        if *ptr.add(pos) == 0x1B {
            return pos;
        }
        pos += 1;
    }
    len
}

/// Scan for first control character (0x00-0x1F) OR high byte (≥0x80).
/// Everything else is printable ASCII that can be batch-output.
/// Returns length of the printable ASCII prefix.
#[cfg(target_arch = "x86_64")]
pub fn scan_printable_ascii(data: &[u8]) -> usize {
    if data.len() >= 32 && is_x86_feature_detected!("avx2") {
        unsafe { scan_printable_ascii_avx2(data) }
    } else {
        scan_printable_ascii_scalar(data)
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn scan_printable_ascii(data: &[u8]) -> usize {
    scan_printable_ascii_scalar(data)
}

fn scan_printable_ascii_scalar(data: &[u8]) -> usize {
    data.iter()
        .position(|&b| b < 0x20 || b > 0x7E)
        .unwrap_or(data.len())
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn scan_printable_ascii_avx2(data: &[u8]) -> usize {
    let len = data.len();
    let ptr = data.as_ptr();
    let mut pos = 0usize;

    // Technique: subtract bias to convert unsigned range check to signed compare.
    // printable range: 0x20..=0x7E → biased: -96..=-2 (all negative in i8)
    let bias = _mm256_set1_epi8(-128i8);
    let low = _mm256_set1_epi8((0x20u8 as i8).wrapping_sub(-128i8)); // -96
    let high = _mm256_set1_epi8((0x7Eu8 as i8).wrapping_sub(-128i8)); // -2

    while pos + 32 <= len {
        let chunk = _mm256_loadu_si256(ptr.add(pos) as *const __m256i);
        let biased = _mm256_add_epi8(chunk, bias);
        // biased >= low  →  cmpgt(biased, low-1)
        let ge_low = _mm256_cmpgt_epi8(biased, _mm256_sub_epi8(low, _mm256_set1_epi8(1)));
        // biased <= high →  cmpgt(high+1, biased)
        let le_high = _mm256_cmpgt_epi8(_mm256_add_epi8(high, _mm256_set1_epi8(1)), biased);
        let in_range = _mm256_and_si256(ge_low, le_high);
        let mask = _mm256_movemask_epi8(in_range) as u32;
        if mask != 0xFFFF_FFFF {
            return pos + (!mask).trailing_zeros() as usize;
        }
        pos += 32;
    }
    // Scalar tail
    while pos < len {
        let b = *ptr.add(pos);
        if b < 0x20 || b > 0x7E {
            break;
        }
        pos += 1;
    }
    pos
}

// ── 2. Grid dirty cell detection: bitmap approach ───────────────
// Replaces sort+dedup with a bitmap scan. For typical 200×50 grids,
// the bitmap fits in L1 cache (~1.5 KB) and produces already-sorted output.

#[cfg(feature = "ghostty_vt")]
pub fn dirty_cells_from_bitmap(
    rects: &[ghostty_vt::GpuDamageRect],
    term_cols: u16,
    max_row: u32,
) -> Vec<ghostty_vt::GpuDirtyCell> {
    if rects.is_empty() || term_cols == 0 || max_row == 0 {
        return Vec::new();
    }
    let cols_u32 = term_cols as u32;
    let total = max_row as usize * term_cols as usize;

    // Bitmap: 1 bit per cell. 64 cells per u64 word.
    let bitmap_words = (total + 63) / 64;
    let mut bitmap = vec![0u64; bitmap_words];

    for rect in rects {
        if rect.col_count == 0 || rect.row_count == 0 || rect.start_col >= cols_u32 {
            continue;
        }
        let col_end = (rect.start_col + rect.col_count).min(cols_u32);
        for row in rect.row_start..rect.row_start.saturating_add(rect.row_count) {
            if row >= max_row {
                break;
            }
            let row_base = row * cols_u32;
            // Set bits for dirty columns in this row
            for col in rect.start_col..col_end {
                let idx = (row_base + col) as usize;
                if idx < total {
                    bitmap[idx / 64] |= 1u64 << (idx % 64);
                }
            }
        }
    }

    // Extract set bits — output is already sorted by construction
    let mut cells = Vec::new();
    for (word_idx, &word) in bitmap.iter().enumerate() {
        if word == 0 {
            continue;
        }
        let base = (word_idx * 64) as u32;
        let mut bits = word;
        while bits != 0 {
            let bit_pos = bits.trailing_zeros();
            cells.push(ghostty_vt::GpuDirtyCell {
                instance_index: base + bit_pos,
            });
            bits &= bits - 1; // clear lowest set bit
        }
    }
    cells
}

/// SIMD-accelerated row byte comparison.
/// Compares raw cell data between old and new row to detect changes.
#[cfg(target_arch = "x86_64")]
pub fn rows_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    if a.len() >= 32 && is_x86_feature_detected!("avx2") {
        unsafe { rows_bytes_eq_avx2(a, b) }
    } else {
        a == b
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn rows_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn rows_bytes_eq_avx2(a: &[u8], b: &[u8]) -> bool {
    let len = a.len();
    let pa = a.as_ptr();
    let pb = b.as_ptr();
    let mut pos = 0usize;

    while pos + 32 <= len {
        let ca = _mm256_loadu_si256(pa.add(pos) as *const __m256i);
        let cb = _mm256_loadu_si256(pb.add(pos) as *const __m256i);
        let eq = _mm256_cmpeq_epi8(ca, cb);
        if _mm256_movemask_epi8(eq) as u32 != 0xFFFF_FFFF {
            return false;
        }
        pos += 32;
    }
    // Scalar tail
    a[pos..] == b[pos..]
}

// ── 3. UTF-8 → glyph index: ASCII fast path ────────────────────
// Terminal text is ~90% ASCII. For ASCII, byte_idx == char_idx == cell_col.
// SIMD validates ASCII-ness of the prefix, enabling O(1) cell-to-char lookup.

/// Count leading ASCII bytes (< 0x80) using SIMD.
/// The movemask trick: bit 7 of each byte becomes a bitmask bit.
/// All-ASCII chunk → mask == 0.
#[cfg(target_arch = "x86_64")]
pub fn ascii_prefix_len(data: &[u8]) -> usize {
    if data.len() >= 32 && is_x86_feature_detected!("avx2") {
        unsafe { ascii_prefix_len_avx2(data) }
    } else {
        ascii_prefix_len_scalar(data)
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn ascii_prefix_len(data: &[u8]) -> usize {
    ascii_prefix_len_scalar(data)
}

fn ascii_prefix_len_scalar(data: &[u8]) -> usize {
    data.iter()
        .position(|&b| b >= 0x80)
        .unwrap_or(data.len())
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn ascii_prefix_len_avx2(data: &[u8]) -> usize {
    let len = data.len();
    let ptr = data.as_ptr();
    let mut pos = 0usize;

    // movemask_epi8 extracts bit 7 of each byte → non-zero means non-ASCII
    while pos + 32 <= len {
        let chunk = _mm256_loadu_si256(ptr.add(pos) as *const __m256i);
        let mask = _mm256_movemask_epi8(chunk) as u32;
        if mask != 0 {
            return pos + mask.trailing_zeros() as usize;
        }
        pos += 32;
    }
    while pos < len {
        if *ptr.add(pos) >= 0x80 {
            return pos;
        }
        pos += 1;
    }
    len
}

/// SIMD-accelerated cell-column to character-index conversion.
///
/// Fast path: if the text's ASCII prefix covers `cell_col`, returns O(1).
/// ASCII printable chars always have display width 1, so byte = char = cell.
/// Falls back to scalar unicode-width iteration only for non-ASCII text.
pub fn fast_cell_to_char_index(text: &str, cell_col: usize) -> usize {
    let bytes = text.as_bytes();
    let ascii_len = ascii_prefix_len(bytes);

    // Fast path 1: entire text is ASCII → O(1) lookup
    if ascii_len >= bytes.len() {
        return cell_col;
    }

    // Fast path 2: ASCII prefix covers the target column
    if cell_col < ascii_len {
        return cell_col;
    }

    // Scalar path: continue from where ASCII prefix ends
    let mut col = ascii_len;
    let suffix = &text[ascii_len..];
    for (char_offset, ch) in suffix.chars().enumerate() {
        if col >= cell_col {
            return ascii_len + char_offset;
        }
        col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
    }

    // Past end of text
    let total_chars = ascii_len + suffix.chars().count();
    total_chars + cell_col.saturating_sub(col)
}

// ── 4. Color packing: batch RGB→RGBA alpha insertion ────────────
// GPU cell colors stored as 0x00RRGGBB need alpha → 0xFFRRGGBB.
// AVX2 processes 8 u32s (32 bytes) per iteration with a single OR.

/// Batch-OR `0xFF00_0000` alpha mask into each element.
/// Processes 8 u32s per AVX2 cycle.
#[cfg(target_arch = "x86_64")]
pub fn batch_or_alpha(values: &mut [u32]) {
    if values.len() >= 8 && is_x86_feature_detected!("avx2") {
        unsafe { batch_or_alpha_avx2(values) }
    } else {
        for v in values.iter_mut() {
            *v |= 0xFF00_0000;
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn batch_or_alpha(values: &mut [u32]) {
    for v in values.iter_mut() {
        *v |= 0xFF00_0000;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn batch_or_alpha_avx2(values: &mut [u32]) {
    let len = values.len();
    let ptr = values.as_mut_ptr();
    let mut pos = 0usize;
    let alpha = _mm256_set1_epi32(0xFF00_0000u32 as i32);

    while pos + 8 <= len {
        let chunk = _mm256_loadu_si256(ptr.add(pos) as *const __m256i);
        let result = _mm256_or_si256(chunk, alpha);
        _mm256_storeu_si256(ptr.add(pos) as *mut __m256i, result);
        pos += 8;
    }
    // Scalar tail
    while pos < len {
        *ptr.add(pos) |= 0xFF00_0000;
        pos += 1;
    }
}

/// Bulk-initialize a row of GpuCellData with default colors.
/// Avoids per-cell push loop: pre-fills all columns, then patches col indices.
#[cfg(feature = "ghostty_vt")]
pub fn init_default_row(
    cells: &mut Vec<ghostty_vt::GpuCellData>,
    row_idx: u16,
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
) {
    let n = term_cols as usize;
    cells.clear();
    cells.reserve(n);

    let template = ghostty_vt::GpuCellData {
        col: 0,
        row: row_idx,
        codepoint: 0,
        fg_rgba: 0xFF00_0000 | default_fg,
        bg_rgba: 0xFF00_0000 | default_bg,
        flags: 0,
        _pad: 0,
    };

    // Fill with template
    cells.resize(n, template);

    // Fixup col indices: 0, 1, 2, ...
    // For typical term_cols (80-300), scalar is fine — the memset above is the big win.
    for (i, cell) in cells.iter_mut().enumerate() {
        cell.col = i as u16;
    }
}

/// Batch apply style run colors to a pre-initialized cell row.
/// Applies fg_rgba and bg_rgba with alpha for the columns covered by each run.
#[cfg(feature = "ghostty_vt")]
pub fn apply_style_run_colors(
    cells: &mut [ghostty_vt::GpuCellData],
    runs: &[ghostty_vt::StyleRun],
) {
    // Collect fg/bg values that need alpha OR
    for run in runs {
        let start = run.start_col.saturating_sub(1) as usize; // 1-based → 0-based
        let end = (run.end_col as usize).min(cells.len());
        if start >= end {
            continue;
        }
        let fg = ((run.fg.r as u32) << 16) | ((run.fg.g as u32) << 8) | (run.fg.b as u32);
        let bg = ((run.bg.r as u32) << 16) | ((run.bg.g as u32) << 8) | (run.bg.b as u32);
        let fg_rgba = 0xFF00_0000 | fg;
        let bg_rgba = 0xFF00_0000 | bg;
        let flags = run.flags as u16;

        for cell in &mut cells[start..end] {
            cell.fg_rgba = fg_rgba;
            cell.bg_rgba = bg_rgba;
            cell.flags = flags;
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_escape_empty() {
        assert_eq!(find_escape(b""), 0);
    }

    #[test]
    fn find_escape_no_esc() {
        assert_eq!(find_escape(b"Hello, world!"), 13);
    }

    #[test]
    fn find_escape_at_start() {
        assert_eq!(find_escape(b"\x1b[31m"), 0);
    }

    #[test]
    fn find_escape_middle() {
        let data = b"Hello\x1b[0mWorld";
        assert_eq!(find_escape(data), 5);
    }

    #[test]
    fn find_escape_large_buffer() {
        let mut data = vec![b'A'; 128];
        data[100] = 0x1B;
        assert_eq!(find_escape(&data), 100);
    }

    #[test]
    fn scan_printable_ascii_basic() {
        assert_eq!(scan_printable_ascii(b"Hello"), 5);
        assert_eq!(scan_printable_ascii(b"Hello\n"), 5);
        assert_eq!(scan_printable_ascii(b"\x1b[31m"), 0);
        assert_eq!(scan_printable_ascii(b""), 0);
    }

    #[test]
    fn scan_printable_ascii_large() {
        let data: Vec<u8> = (0..256).map(|_| b'X').collect();
        assert_eq!(scan_printable_ascii(&data), 256);
    }

    #[test]
    fn ascii_prefix_len_all_ascii() {
        assert_eq!(ascii_prefix_len(b"Hello World"), 11);
    }

    #[test]
    fn ascii_prefix_len_utf8() {
        let s = "Helloあいう";
        assert_eq!(ascii_prefix_len(s.as_bytes()), 5);
    }

    #[test]
    fn ascii_prefix_len_all_utf8() {
        let s = "あいう";
        assert_eq!(ascii_prefix_len(s.as_bytes()), 0);
    }

    #[test]
    fn fast_cell_to_char_index_ascii() {
        assert_eq!(fast_cell_to_char_index("Hello World", 5), 5);
        assert_eq!(fast_cell_to_char_index("Hello World", 0), 0);
        assert_eq!(fast_cell_to_char_index("Hello World", 11), 11);
        // Past end
        assert_eq!(fast_cell_to_char_index("Hello", 10), 10);
    }

    #[test]
    fn fast_cell_to_char_index_cjk() {
        // 'あ' is width 2, 'B' is width 1
        // text: "あBCD" → cells: [あ][_][B][C][D]
        // cell 0 → char 0 (あ)
        // cell 2 → char 1 (B)
        // cell 3 → char 2 (C)
        assert_eq!(fast_cell_to_char_index("あBCD", 0), 0);
        assert_eq!(fast_cell_to_char_index("あBCD", 2), 1);
        assert_eq!(fast_cell_to_char_index("あBCD", 3), 2);
    }

    #[test]
    fn fast_cell_to_char_index_mixed() {
        // "ABあCD" → cells: [A][B][あ][_][C][D]
        // cell 0 → char 0 (A)
        // cell 1 → char 1 (B)
        // cell 2 → char 2 (あ)  — ASCII prefix is 2, cell_col 2 >= 2
        // cell 4 → char 3 (C)
        assert_eq!(fast_cell_to_char_index("ABあCD", 0), 0);
        assert_eq!(fast_cell_to_char_index("ABあCD", 1), 1);
        assert_eq!(fast_cell_to_char_index("ABあCD", 2), 2);
        assert_eq!(fast_cell_to_char_index("ABあCD", 4), 3);
    }

    #[test]
    fn batch_or_alpha_basic() {
        let mut values = vec![0x00FF0000, 0x0000FF00, 0x000000FF];
        batch_or_alpha(&mut values);
        assert_eq!(values, vec![0xFFFF0000, 0xFF00FF00, 0xFF0000FF]);
    }

    #[test]
    fn batch_or_alpha_large() {
        let mut values: Vec<u32> = (0..64).map(|i| i * 0x010101).collect();
        batch_or_alpha(&mut values);
        for (i, &v) in values.iter().enumerate() {
            assert_eq!(v, 0xFF00_0000 | (i as u32 * 0x010101));
        }
    }

    #[test]
    fn rows_bytes_eq_identical() {
        let a = vec![1u8; 128];
        let b = vec![1u8; 128];
        assert!(rows_bytes_eq(&a, &b));
    }

    #[test]
    fn rows_bytes_eq_different() {
        let a = vec![1u8; 128];
        let mut b = vec![1u8; 128];
        b[100] = 2;
        assert!(!rows_bytes_eq(&a, &b));
    }

    #[test]
    fn rows_bytes_eq_different_len() {
        assert!(!rows_bytes_eq(&[1, 2, 3], &[1, 2]));
    }
}
