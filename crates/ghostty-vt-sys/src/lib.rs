//! ghostty-vt-sys — Raw C FFI bindings to Ghostty's VT terminal

#[repr(C)]
pub struct ghostty_vt_bytes_t {
    pub ptr: *const u8,
    pub len: usize,
}

pub const PINNED_GHOSTTY_TAG: &str = "v1.3.0";
pub const PINNED_ZIG_VERSION: &str = "0.15.2";

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ghostty_vt_content_stats_t {
    pub kind: u8,
    pub flags: u8,
    pub reserved0: u8,
    pub reserved1: u8,
    pub json_structural_count: u32,
    pub markdown_marker_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuDirtyRange {
    pub start_instance: u32,
    pub instance_count: u32,
    pub row_start: u32,
    pub row_count: u32,
}

unsafe extern "C" {
    pub fn ghostty_vt_terminal_new(cols: u16, rows: u16) -> *mut core::ffi::c_void;
    pub fn ghostty_vt_terminal_new_with_scrollback(
        cols: u16,
        rows: u16,
        max_scrollback: usize,
    ) -> *mut core::ffi::c_void;
    pub fn ghostty_vt_terminal_free(terminal: *mut core::ffi::c_void);

    pub fn ghostty_vt_terminal_set_default_colors(
        terminal: *mut core::ffi::c_void,
        fg_r: u8,
        fg_g: u8,
        fg_b: u8,
        bg_r: u8,
        bg_g: u8,
        bg_b: u8,
    );

    pub fn ghostty_vt_terminal_feed(
        terminal: *mut core::ffi::c_void,
        bytes: *const u8,
        len: usize,
    ) -> core::ffi::c_int;

    pub fn ghostty_vt_terminal_resize(
        terminal: *mut core::ffi::c_void,
        cols: u16,
        rows: u16,
    ) -> core::ffi::c_int;

    pub fn ghostty_vt_terminal_scroll_viewport(
        terminal: *mut core::ffi::c_void,
        delta_lines: i32,
    ) -> core::ffi::c_int;

    pub fn ghostty_vt_terminal_scroll_viewport_top(
        terminal: *mut core::ffi::c_void,
    ) -> core::ffi::c_int;

    pub fn ghostty_vt_terminal_scroll_viewport_bottom(
        terminal: *mut core::ffi::c_void,
    ) -> core::ffi::c_int;

    pub fn ghostty_vt_terminal_cursor_position(
        terminal: *mut core::ffi::c_void,
        col_out: *mut u16,
        row_out: *mut u16,
    ) -> bool;
    pub fn ghostty_vt_terminal_cursor_visible(terminal: *mut core::ffi::c_void) -> bool;

    pub fn ghostty_vt_terminal_dump_viewport(
        terminal: *mut core::ffi::c_void,
    ) -> ghostty_vt_bytes_t;

    pub fn ghostty_vt_terminal_dump_viewport_row(
        terminal: *mut core::ffi::c_void,
        row: u16,
    ) -> ghostty_vt_bytes_t;

    pub fn ghostty_vt_terminal_dump_viewport_row_cell_styles(
        terminal: *mut core::ffi::c_void,
        row: u16,
    ) -> ghostty_vt_bytes_t;

    pub fn ghostty_vt_terminal_dump_viewport_row_style_runs(
        terminal: *mut core::ffi::c_void,
        row: u16,
    ) -> ghostty_vt_bytes_t;

    pub fn ghostty_vt_terminal_take_dirty_viewport_rows(
        terminal: *mut core::ffi::c_void,
        rows: u16,
    ) -> ghostty_vt_bytes_t;

    pub fn ghostty_vt_terminal_take_viewport_scroll_delta(terminal: *mut core::ffi::c_void) -> i32;

    pub fn ghostty_vt_terminal_hyperlink_at(
        terminal: *mut core::ffi::c_void,
        col: u16,
        row: u16,
    ) -> ghostty_vt_bytes_t;

    pub fn ghostty_vt_encode_key_named(
        name: *const u8,
        name_len: usize,
        modifiers: u16,
    ) -> ghostty_vt_bytes_t;

    pub fn ghostty_vt_bytes_free(bytes: ghostty_vt_bytes_t);

    // Async I/O
    pub fn ghostty_vt_terminal_start_async(terminal: *mut core::ffi::c_void) -> core::ffi::c_int;
    pub fn ghostty_vt_terminal_stop_async(terminal: *mut core::ffi::c_void);
    pub fn ghostty_vt_terminal_feed_async(
        terminal: *mut core::ffi::c_void,
        bytes: *const u8,
        len: usize,
    ) -> usize;
    pub fn ghostty_vt_terminal_has_new_data(terminal: *mut core::ffi::c_void) -> bool;
    pub fn ghostty_vt_terminal_content_kind(terminal: *const core::ffi::c_void) -> u8;
    pub fn ghostty_simd_detect_content(bytes: *const u8, len: usize) -> ghostty_vt_content_stats_t;

    // DX12 GPU renderer
    pub fn ghostty_gpu_renderer_new(
        width: u32,
        height: u32,
        font_size: f32,
    ) -> *mut core::ffi::c_void;
    pub fn ghostty_gpu_renderer_free(renderer: *mut core::ffi::c_void);
    pub fn ghostty_gpu_renderer_resize(
        renderer: *mut core::ffi::c_void,
        width: u32,
        height: u32,
    ) -> u8;
    pub fn ghostty_gpu_renderer_render(
        renderer: *mut core::ffi::c_void,
        cells: *const GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> *const u8;
    pub fn ghostty_gpu_renderer_render_delta(
        renderer: *mut core::ffi::c_void,
        cells: *const GpuCellData,
        cell_count: u32,
        dirty_ranges: *const GpuDirtyRange,
        dirty_range_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> *const u8;
    pub fn ghostty_gpu_renderer_render_to_texture(
        renderer: *mut core::ffi::c_void,
        cells: *const GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> u8;
    pub fn ghostty_gpu_renderer_render_to_surface(
        renderer: *mut core::ffi::c_void,
        target_resource: *mut core::ffi::c_void,
        cells: *const GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) -> u8;
    pub fn ghostty_gpu_renderer_pixel_stride(renderer: *const core::ffi::c_void) -> u32;
    pub fn ghostty_gpu_renderer_width(renderer: *const core::ffi::c_void) -> u32;
    pub fn ghostty_gpu_renderer_height(renderer: *const core::ffi::c_void) -> u32;
    pub fn ghostty_gpu_renderer_device_ptr(
        renderer: *const core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
    pub fn ghostty_gpu_renderer_command_queue_ptr(
        renderer: *const core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
    pub fn ghostty_gpu_renderer_render_target_ptr(
        renderer: *const core::ffi::c_void,
    ) -> *mut core::ffi::c_void;

    /// Retrieve the last GPU renderer init error (stage code + HRESULT).
    /// stage=0 means no error. See GpuInitStage enum in gpu_renderer.zig.
    pub fn ghostty_gpu_renderer_last_init_error(stage_out: *mut u32, hr_out: *mut i32);
}

/// Cell data passed to the GPU renderer (20 bytes, C-compatible).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GpuCellData {
    pub col: u16,
    pub row: u16,
    pub codepoint: u32,
    pub fg_rgba: u32,
    pub bg_rgba: u32,
    pub flags: u16,
    pub _pad: u16,
}
