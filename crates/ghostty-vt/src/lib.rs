//! ghostty-vt — Safe Rust wrapper around Ghostty's VT terminal

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;

#[derive(Debug)]
pub enum Error {
    CreateFailed,
    FeedFailed(i32),
    ScrollFailed(i32),
    DumpFailed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::CreateFailed => write!(f, "terminal create failed"),
            Error::FeedFailed(code) => write!(f, "terminal feed failed: {code}"),
            Error::ScrollFailed(code) => write!(f, "terminal scroll failed: {code}"),
            Error::DumpFailed => write!(f, "terminal dump failed"),
        }
    }
}

impl std::error::Error for Error {}

pub struct Terminal {
    ptr: NonNull<c_void>,
}

unsafe impl Send for Terminal {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: Rgb,
    pub bg: Rgb,
    pub flags: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleRun {
    pub start_col: u16,
    pub end_col: u16,
    pub fg: Rgb,
    pub bg: Rgb,
    pub flags: u8,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ContentKind {
    #[default]
    Unknown = 0,
    PlainText = 1,
    Json = 2,
    Markdown = 3,
}

impl From<u8> for ContentKind {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::PlainText,
            2 => Self::Json,
            3 => Self::Markdown,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ContentStats {
    pub kind: ContentKind,
    pub flags: u8,
    pub json_structural_count: u32,
    pub markdown_marker_count: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl KeyModifiers {
    fn bits(self) -> u16 {
        let mut bits = 0u16;
        if self.shift {
            bits |= 0x0001;
        }
        if self.control {
            bits |= 0x0002;
        }
        if self.alt {
            bits |= 0x0004;
        }
        if self.super_key {
            bits |= 0x0008;
        }
        bits
    }
}

pub fn encode_key_named(name: &str, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    if name.is_empty() {
        return None;
    }

    let bytes = unsafe {
        ghostty_vt_sys::ghostty_vt_encode_key_named(name.as_ptr(), name.len(), modifiers.bits())
    };
    if bytes.ptr.is_null() || bytes.len == 0 {
        return None;
    }

    let slice = unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) };
    let out = slice.to_vec();
    unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
    Some(out)
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Result<Self, Error> {
        let ptr = unsafe { ghostty_vt_sys::ghostty_vt_terminal_new(cols, rows) };
        let ptr = NonNull::new(ptr).ok_or(Error::CreateFailed)?;
        Ok(Self { ptr })
    }

    pub fn new_with_scrollback(cols: u16, rows: u16, max_scrollback: usize) -> Result<Self, Error> {
        let ptr = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_new_with_scrollback(cols, rows, max_scrollback)
        };
        let ptr = NonNull::new(ptr).ok_or(Error::CreateFailed)?;
        Ok(Self { ptr })
    }

    pub fn set_default_colors(&mut self, fg: Rgb, bg: Rgb) {
        unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_set_default_colors(
                self.ptr.as_ptr(),
                fg.r,
                fg.g,
                fg.b,
                bg.r,
                bg.g,
                bg.b,
            )
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let rc = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_feed(self.ptr.as_ptr(), bytes.as_ptr(), bytes.len())
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(Error::FeedFailed(rc))
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), Error> {
        let rc =
            unsafe { ghostty_vt_sys::ghostty_vt_terminal_resize(self.ptr.as_ptr(), cols, rows) };
        if rc == 0 {
            Ok(())
        } else {
            Err(Error::ScrollFailed(rc))
        }
    }

    pub fn dump_viewport(&self) -> Result<String, Error> {
        let bytes = unsafe { ghostty_vt_sys::ghostty_vt_terminal_dump_viewport(self.ptr.as_ptr()) };
        if bytes.ptr.is_null() {
            return Err(Error::DumpFailed);
        }
        let slice = unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) };
        let s = String::from_utf8_lossy(slice).into_owned();
        unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
        Ok(s)
    }

    pub fn dump_viewport_row(&self, row: u16) -> Result<String, Error> {
        let bytes = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_dump_viewport_row(self.ptr.as_ptr(), row)
        };
        if bytes.ptr.is_null() {
            return Err(Error::DumpFailed);
        }
        let slice = unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) };
        let s = String::from_utf8_lossy(slice).into_owned();
        unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
        Ok(s)
    }

    pub fn dump_viewport_row_cell_styles(&self, row: u16) -> Result<Vec<CellStyle>, Error> {
        let bytes = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_dump_viewport_row_cell_styles(
                self.ptr.as_ptr(),
                row,
            )
        };
        if bytes.ptr.is_null() {
            return Err(Error::DumpFailed);
        }
        if bytes.len == 0 {
            unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
            return Ok(Vec::new());
        }
        if bytes.len % 8 != 0 {
            unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
            return Err(Error::DumpFailed);
        }

        let slice = unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) };
        let mut out = Vec::with_capacity(bytes.len / 8);
        for chunk in slice.chunks_exact(8) {
            out.push(CellStyle {
                fg: Rgb {
                    r: chunk[0],
                    g: chunk[1],
                    b: chunk[2],
                },
                bg: Rgb {
                    r: chunk[3],
                    g: chunk[4],
                    b: chunk[5],
                },
                flags: chunk[6],
            });
        }
        unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
        Ok(out)
    }

    pub fn dump_viewport_row_style_runs(&self, row: u16) -> Result<Vec<StyleRun>, Error> {
        let bytes = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_dump_viewport_row_style_runs(self.ptr.as_ptr(), row)
        };
        if bytes.ptr.is_null() {
            return Err(Error::DumpFailed);
        }
        if bytes.len == 0 {
            unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
            return Ok(Vec::new());
        }
        if bytes.len % 12 != 0 {
            unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
            return Err(Error::DumpFailed);
        }

        let slice = unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) };
        let mut out = Vec::with_capacity(bytes.len / 12);
        for chunk in slice.chunks_exact(12) {
            out.push(StyleRun {
                start_col: u16::from_ne_bytes([chunk[0], chunk[1]]),
                end_col: u16::from_ne_bytes([chunk[2], chunk[3]]),
                fg: Rgb {
                    r: chunk[4],
                    g: chunk[5],
                    b: chunk[6],
                },
                bg: Rgb {
                    r: chunk[7],
                    g: chunk[8],
                    b: chunk[9],
                },
                flags: chunk[10],
            });
        }
        unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
        Ok(out)
    }

    pub fn take_dirty_viewport_rows(&mut self, rows: u16) -> Result<Vec<u16>, Error> {
        let bytes = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_take_dirty_viewport_rows(self.ptr.as_ptr(), rows)
        };
        if bytes.ptr.is_null() || bytes.len == 0 {
            return Ok(Vec::new());
        }
        if bytes.len % 2 != 0 {
            unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
            return Err(Error::DumpFailed);
        }

        let slice = unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) };
        let mut out = Vec::with_capacity(bytes.len / 2);
        for chunk in slice.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
        Ok(out)
    }

    pub fn take_viewport_scroll_delta(&mut self) -> i32 {
        unsafe { ghostty_vt_sys::ghostty_vt_terminal_take_viewport_scroll_delta(self.ptr.as_ptr()) }
    }

    pub fn cursor_position(&self) -> Option<(u16, u16)> {
        let mut col: u16 = 0;
        let mut row: u16 = 0;
        let ok = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_cursor_position(
                self.ptr.as_ptr(),
                &mut col as *mut u16,
                &mut row as *mut u16,
            )
        };
        ok.then_some((col, row))
    }

    pub fn cursor_visible(&self) -> bool {
        unsafe { ghostty_vt_sys::ghostty_vt_terminal_cursor_visible(self.ptr.as_ptr()) }
    }

    pub fn hyperlink_at(&self, col: u16, row: u16) -> Option<String> {
        let bytes = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_hyperlink_at(self.ptr.as_ptr(), col, row)
        };
        if bytes.ptr.is_null() || bytes.len == 0 {
            return None;
        }
        let slice = unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) };
        let s = String::from_utf8_lossy(slice).into_owned();
        unsafe { ghostty_vt_sys::ghostty_vt_bytes_free(bytes) };
        Some(s)
    }

    pub fn scroll_viewport(&mut self, delta_lines: i32) -> Result<(), Error> {
        let rc = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_scroll_viewport(self.ptr.as_ptr(), delta_lines)
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(Error::ScrollFailed(rc))
        }
    }

    pub fn scroll_viewport_top(&mut self) -> Result<(), Error> {
        let rc =
            unsafe { ghostty_vt_sys::ghostty_vt_terminal_scroll_viewport_top(self.ptr.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(Error::ScrollFailed(rc))
        }
    }

    pub fn scroll_viewport_bottom(&mut self) -> Result<(), Error> {
        let rc = unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_scroll_viewport_bottom(self.ptr.as_ptr())
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(Error::ScrollFailed(rc))
        }
    }

    /// Raw C pointer for direct async operations.
    pub fn raw_ptr(&self) -> *mut c_void {
        self.ptr.as_ptr()
    }

    /// Start async I/O mode with dedicated parser thread.
    pub fn start_async(&mut self) -> Result<(), Error> {
        let rc = unsafe { ghostty_vt_sys::ghostty_vt_terminal_start_async(self.ptr.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(Error::FeedFailed(rc))
        }
    }

    /// Stop async I/O mode.
    pub fn stop_async(&mut self) {
        unsafe { ghostty_vt_sys::ghostty_vt_terminal_stop_async(self.ptr.as_ptr()) }
    }

    /// Feed data asynchronously via ring buffer. Non-blocking.
    /// Returns number of bytes pushed to the ring buffer.
    pub fn feed_async(&self, bytes: &[u8]) -> usize {
        unsafe {
            ghostty_vt_sys::ghostty_vt_terminal_feed_async(
                self.ptr.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
            )
        }
    }

    /// Check if new data has been parsed since the last check.
    pub fn has_new_data(&self) -> bool {
        unsafe { ghostty_vt_sys::ghostty_vt_terminal_has_new_data(self.ptr.as_ptr()) }
    }

    pub fn content_kind(&self) -> ContentKind {
        unsafe { ghostty_vt_sys::ghostty_vt_terminal_content_kind(self.ptr.as_ptr()) }.into()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { ghostty_vt_sys::ghostty_vt_terminal_free(self.ptr.as_ptr()) }
    }
}

/// Feed data asynchronously using a raw terminal pointer.
/// Non-blocking, pushes to the internal ring buffer.
///
/// # Safety
/// `raw_ptr` must be a valid terminal pointer from `Terminal::raw_ptr()`.
pub unsafe fn feed_async_raw(raw_ptr: *mut c_void, bytes: &[u8]) -> usize {
    unsafe { ghostty_vt_sys::ghostty_vt_terminal_feed_async(raw_ptr, bytes.as_ptr(), bytes.len()) }
}

pub fn classify_content(bytes: &[u8]) -> ContentStats {
    if bytes.is_empty() {
        return ContentStats::default();
    }

    let raw = unsafe { ghostty_vt_sys::ghostty_simd_detect_content(bytes.as_ptr(), bytes.len()) };
    ContentStats {
        kind: raw.kind.into(),
        flags: raw.flags,
        json_structural_count: raw.json_structural_count,
        markdown_marker_count: raw.markdown_marker_count,
    }
}

// ── DX12 GPU Renderer ──────────────────────────────────────────────────

pub use ghostty_vt_sys::{GpuCellData, GpuDamageRect, GpuDirtyCell, GpuDirtyRange};

pub struct GpuRenderer {
    ptr: NonNull<c_void>,
}

unsafe impl Send for GpuRenderer {}

impl GpuRenderer {
    /// Create a new DX12 GPU renderer with the given viewport size and font size.
    pub fn new(width: u32, height: u32, font_size: f32) -> Result<Self, Error> {
        let ptr = unsafe { ghostty_vt_sys::ghostty_gpu_renderer_new(width, height, font_size) };
        let ptr = NonNull::new(ptr).ok_or(Error::CreateFailed)?;
        Ok(Self { ptr })
    }

    /// Resize the offscreen render target. Returns true if resize succeeded.
    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_resize(self.ptr.as_ptr(), width, height) != 0
        }
    }

    /// Render a frame. Returns a slice of RGBA pixels (row-major, with stride padding).
    /// The returned slice is valid until the next call to `render` or `drop`.
    pub fn render(
        &mut self,
        cells: &[GpuCellData],
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<&[u8]> {
        let ptr = unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_render(
                self.ptr.as_ptr(),
                cells.as_ptr(),
                cells.len() as u32,
                term_cols,
                cell_width,
                cell_height,
            )
        };
        if ptr.is_null() {
            return None;
        }
        let stride = self.pixel_stride();
        let height = self.height();
        let len = (stride * height) as usize;
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    /// Render only the changed terminal ranges, preserving the previous render target contents.
    /// The returned slice is valid until the next call to `render`, `render_delta`, or `drop`.
    pub fn render_delta(
        &mut self,
        cells: &[GpuCellData],
        dirty_ranges: &[GpuDirtyRange],
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<&[u8]> {
        let ptr = unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_render_delta(
                self.ptr.as_ptr(),
                cells.as_ptr(),
                cells.len() as u32,
                dirty_ranges.as_ptr(),
                dirty_ranges.len() as u32,
                term_cols,
                cell_width,
                cell_height,
            )
        };
        if ptr.is_null() {
            return None;
        }
        let stride = self.pixel_stride();
        let height = self.height();
        let len = (stride * height) as usize;
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    /// Render a frame and leave the GPU render target resident for native presentation.
    pub fn render_to_texture(
        &mut self,
        cells: &[GpuCellData],
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> bool {
        unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_render_to_texture(
                self.ptr.as_ptr(),
                cells.as_ptr(),
                cells.len() as u32,
                term_cols,
                cell_width,
                cell_height,
            ) != 0
        }
    }

    pub fn render_to_surface(
        &mut self,
        target_resource: *mut c_void,
        cells: &[GpuCellData],
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> bool {
        unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_render_to_surface(
                self.ptr.as_ptr(),
                target_resource,
                cells.as_ptr(),
                cells.len() as u32,
                term_cols,
                cell_width,
                cell_height,
            ) != 0
        }
    }

    pub fn render_to_surface_delta(
        &mut self,
        target_resource: *mut c_void,
        cells: &[GpuCellData],
        damage_rects: &[GpuDamageRect],
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> bool {
        unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_render_to_surface_delta(
                self.ptr.as_ptr(),
                target_resource,
                cells.as_ptr(),
                cells.len() as u32,
                damage_rects.as_ptr(),
                damage_rects.len() as u32,
                term_cols,
                cell_width,
                cell_height,
            ) != 0
        }
    }

    pub fn render_to_surface_delta_cells(
        &mut self,
        target_resource: *mut c_void,
        cells: &[GpuCellData],
        dirty_cells: &[GpuDirtyCell],
        damage_rects: &[GpuDamageRect],
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> bool {
        unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_render_to_surface_delta_cells(
                self.ptr.as_ptr(),
                target_resource,
                cells.as_ptr(),
                cells.len() as u32,
                dirty_cells.as_ptr(),
                dirty_cells.len() as u32,
                damage_rects.as_ptr(),
                damage_rects.len() as u32,
                term_cols,
                cell_width,
                cell_height,
            ) != 0
        }
    }

    /// Padded row stride in bytes (aligned to 256 for DX12 readback).
    pub fn pixel_stride(&self) -> u32 {
        unsafe { ghostty_vt_sys::ghostty_gpu_renderer_pixel_stride(self.ptr.as_ptr() as *const _) }
    }

    pub fn width(&self) -> u32 {
        unsafe { ghostty_vt_sys::ghostty_gpu_renderer_width(self.ptr.as_ptr() as *const _) }
    }

    pub fn height(&self) -> u32 {
        unsafe { ghostty_vt_sys::ghostty_gpu_renderer_height(self.ptr.as_ptr() as *const _) }
    }

    pub fn device_ptr(&self) -> *mut c_void {
        unsafe { ghostty_vt_sys::ghostty_gpu_renderer_device_ptr(self.ptr.as_ptr() as *const _) }
    }

    pub fn command_queue_ptr(&self) -> *mut c_void {
        unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_command_queue_ptr(self.ptr.as_ptr() as *const _)
        }
    }

    pub fn render_target_ptr(&self) -> *mut c_void {
        unsafe {
            ghostty_vt_sys::ghostty_gpu_renderer_render_target_ptr(self.ptr.as_ptr() as *const _)
        }
    }
}

/// Retrieve the last GPU renderer init error diagnostic.
/// Returns `(stage, hresult)` where stage=0 means no error.
pub fn gpu_renderer_last_init_error() -> (u32, i32) {
    let mut stage: u32 = 0;
    let mut hr: i32 = 0;
    unsafe {
        ghostty_vt_sys::ghostty_gpu_renderer_last_init_error(&mut stage, &mut hr);
    }
    (stage, hr)
}

/// Human-readable description for a GPU init error stage code.
pub fn gpu_init_stage_name(stage: u32) -> &'static str {
    match stage {
        0 => "none",
        1 => "alloc_struct",
        2 => "alloc_glyph_map",
        3 => "alloc_atlas_bitmap",
        4 => "gdi_create_dc",
        5 => "gdi_create_font",
        6 => "dwrite_create_factory",
        7 => "dwrite_get_gdi_interop",
        8 => "dwrite_create_font_face",
        9 => "dwrite_font_metrics",
        10 => "dx12_create_device_hw",
        11 => "dx12_create_device_warp",
        12 => "dx12_command_queue",
        13 => "dx12_command_allocator",
        14 => "dx12_descriptor_heap_rtv",
        15 => "dx12_descriptor_heap_srv",
        16 => "dx12_fence",
        17 => "dx12_fence_event",
        18 => "dx12_pipeline",
        19 => "dx12_render_target",
        20 => "dx12_cell_buffers",
        21 => "dx12_atlas_texture",
        22 => "dx12_command_list",
        23 => "dx12_shader_compile_vs",
        24 => "dx12_shader_compile_ps",
        25 => "dx12_root_sig_serialize",
        26 => "dx12_root_sig_create",
        27 => "dx12_pso_create",
        _ => "unknown",
    }
}

impl Drop for GpuRenderer {
    fn drop(&mut self) {
        unsafe { ghostty_vt_sys::ghostty_gpu_renderer_free(self.ptr.as_ptr()) }
    }
}

// ── Vulkan GPU Renderer ──────────────────────────────────────────────

pub struct VulkanRenderer {
    ptr: NonNull<c_void>,
}

unsafe impl Send for VulkanRenderer {}

impl VulkanRenderer {
    /// Create a new Vulkan GPU renderer. Returns Err if Vulkan is unavailable.
    pub fn new(width: u32, height: u32, font_size: f32) -> Result<Self, Error> {
        let ptr =
            unsafe { ghostty_vt_sys::ghostty_vulkan_renderer_new(width, height, font_size) };
        let ptr = NonNull::new(ptr).ok_or(Error::CreateFailed)?;
        Ok(Self { ptr })
    }

    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        unsafe {
            ghostty_vt_sys::ghostty_vulkan_renderer_resize(self.ptr.as_ptr(), width, height) != 0
        }
    }

    pub fn render(
        &mut self,
        cells: &[GpuCellData],
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<&[u8]> {
        let ptr = unsafe {
            ghostty_vt_sys::ghostty_vulkan_renderer_render(
                self.ptr.as_ptr(),
                cells.as_ptr(),
                cells.len() as u32,
                term_cols,
                cell_width,
                cell_height,
            )
        };
        if ptr.is_null() {
            return None;
        }
        let stride = self.pixel_stride();
        let height = self.height();
        let len = (stride * height) as usize;
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    pub fn width(&self) -> u32 {
        unsafe { ghostty_vt_sys::ghostty_vulkan_renderer_width(self.ptr.as_ptr()) }
    }

    pub fn height(&self) -> u32 {
        unsafe { ghostty_vt_sys::ghostty_vulkan_renderer_height(self.ptr.as_ptr()) }
    }

    pub fn pixel_stride(&self) -> u32 {
        unsafe { ghostty_vt_sys::ghostty_vulkan_renderer_pixel_stride(self.ptr.as_ptr()) }
    }
}

impl Drop for VulkanRenderer {
    fn drop(&mut self) {
        unsafe { ghostty_vt_sys::ghostty_vulkan_renderer_free(self.ptr.as_ptr()) }
    }
}

/// Retrieve the last Vulkan renderer init error.
pub fn vulkan_renderer_last_init_error() -> (u32, i32) {
    let mut stage: u32 = 0;
    let mut hr: i32 = 0;
    unsafe {
        ghostty_vt_sys::ghostty_vulkan_renderer_last_init_error(&mut stage, &mut hr);
    }
    (stage, hr)
}

#[cfg(test)]
mod tests {
    use super::{ContentKind, Terminal, classify_content};

    #[test]
    fn constructors_create_terminal() {
        let terminal = Terminal::new(80, 24).expect("default constructor should succeed");
        drop(terminal);

        let terminal = Terminal::new_with_scrollback(80, 24, 0)
            .expect("scrollback constructor should succeed");
        drop(terminal);
    }

    #[test]
    fn feed_renders_ascii_and_utf8_text() {
        let mut terminal = Terminal::new(16, 4).expect("constructor should succeed");

        terminal
            .feed("hello".as_bytes())
            .expect("ascii feed should succeed");
        terminal
            .feed(" 世界".as_bytes())
            .expect("utf8 feed should succeed");

        let row = terminal
            .dump_viewport_row(0)
            .expect("row dump should succeed");
        assert!(row.contains("hello 世界"));
    }

    #[test]
    fn feed_stops_text_at_escape_sequence_boundary() {
        let mut terminal = Terminal::new(16, 4).expect("constructor should succeed");

        terminal
            .feed(b"abc\x1b[2;5HXY")
            .expect("feed with escape sequence should succeed");

        let first_row = terminal
            .dump_viewport_row(0)
            .expect("first row dump should succeed");
        let second_row = terminal
            .dump_viewport_row(1)
            .expect("second row dump should succeed");
        assert!(first_row.contains("abc"));
        assert!(second_row.contains("XY"));
    }

    #[test]
    fn cursor_visibility_tracks_terminal_mode() {
        let mut terminal = Terminal::new(16, 4).expect("constructor should succeed");

        assert!(terminal.cursor_visible());

        terminal
            .feed(b"\x1b[?25l")
            .expect("hiding cursor should succeed");
        assert!(!terminal.cursor_visible());

        terminal
            .feed(b"\x1b[?25h")
            .expect("showing cursor should succeed");
        assert!(terminal.cursor_visible());
    }

    #[test]
    fn classify_content_detects_json_streams() {
        let stats = classify_content(br#"{ "type": "message", "items": [1, 2, 3] }"#);
        assert_eq!(stats.kind, ContentKind::Json);
        assert!(stats.json_structural_count >= 6);
    }

    #[test]
    fn classify_content_detects_markdown_streams() {
        let stats = classify_content(b"# Title\n\n- item 1\n- item 2\n```json\n{}\n```\n");
        assert_eq!(stats.kind, ContentKind::Markdown);
        assert!(stats.markdown_marker_count >= 3);
    }

    #[test]
    fn classify_content_keeps_json_strings_from_becoming_markdown() {
        let stats = classify_content(b"{ \"body\": \"# not a heading\\n- not a list\" }");
        assert_eq!(stats.kind, ContentKind::Json);
        assert_eq!(stats.markdown_marker_count, 0);
    }

    #[test]
    fn classify_content_detects_large_json_streams_across_simd_chunks() {
        let payload = format!(
            r#"{{"type":"message","items":[{{"id":1,"text":"{}","ok":true}}],"meta":{{"source":"cli","format":"json"}}}}"#,
            "x".repeat(96)
        );
        let stats = classify_content(payload.as_bytes());
        assert_eq!(stats.kind, ContentKind::Json);
        assert!(stats.json_structural_count >= 6);
    }

    #[test]
    fn classify_content_handles_escaped_quotes_across_chunk_boundaries() {
        let payload = format!(
            "{{\"body\":\"{}\\\\\\\"# not markdown\\\\n- still string\\\\\\\"\",\"done\":false,\"items\":[1,2,3]}}",
            "a".repeat(48)
        );
        let stats = classify_content(payload.as_bytes());
        assert_eq!(stats.kind, ContentKind::Json);
        assert_eq!(stats.markdown_marker_count, 0);
    }

    #[test]
    fn classify_content_handles_even_backslashes_before_closing_quote_across_chunks() {
        let mut payload = String::from("{\"body\":\"");
        while payload.len() % 32 != 28 {
            payload.push('b');
        }
        payload.push('\\');
        payload.push('\\');
        payload.push('\\');
        payload.push('\\');
        payload.push('"');
        payload.push_str(",\"done\":false,\"items\":[1,2,3]}");

        let stats = classify_content(payload.as_bytes());
        assert_eq!(stats.kind, ContentKind::Json);
        assert!(stats.json_structural_count >= 6);
        assert_eq!(stats.markdown_marker_count, 0);
    }

    #[test]
    fn classify_content_does_not_treat_quoted_lines_as_markdown() {
        let stats = classify_content(br#""quoted prelude" - not markdown"#);
        assert_eq!(stats.kind, ContentKind::PlainText);
        assert_eq!(stats.markdown_marker_count, 0);
    }

    #[test]
    fn async_feed_tracks_content_kind() {
        let mut terminal = Terminal::new(32, 8).expect("constructor should succeed");
        terminal
            .start_async()
            .expect("starting async mode should succeed");

        let pushed = terminal.feed_async(br#"{ "type": "message", "ok": true }"#);
        assert!(pushed > 0);
        assert_eq!(terminal.content_kind(), ContentKind::Json);

        terminal.stop_async();
    }
}
