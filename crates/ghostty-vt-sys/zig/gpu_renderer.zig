//! DX12 GPU terminal renderer — offscreen cell rendering with glyph atlas.
//!
//! Architecture:
//!   1. DirectWrite rasterises glyphs → R8 bitmap atlas
//!   2. Atlas uploaded to DX12 SRV texture
//!   3. Per-frame: cell data → StructuredBuffer upload → instanced draw (6 verts/cell)
//!   4. Render target → readback buffer → CPU-accessible RGBA pixels
//!   5. Rust/GPUI displays the pixel buffer as an image
//!
//! All DX12 operations happen in this file; Rust never touches DX12 directly.

const std = @import("std");
const dx = @import("dx12.zig");
const shaders = @import("shaders.zig");

const Allocator = std.mem.Allocator;
const log = std.log.scoped(.gpu_renderer);

// ================================================================
// Public FFI cell type (C-compatible, 20 bytes)
// ================================================================

pub const GpuCellData = extern struct {
    col: u16,
    row: u16,
    codepoint: u32,
    fg_rgba: u32, // 0xAARRGGBB
    bg_rgba: u32, // 0xAARRGGBB
    flags: u16,
    _pad: u16 = 0,
};

pub const GpuDirtyRange = extern struct {
    start_instance: u32,
    instance_count: u32,
    row_start: u32,
    row_count: u32,
};

pub const GpuDamageRect = extern struct {
    start_col: u32,
    col_count: u32,
    row_start: u32,
    row_count: u32,
};

pub const GpuDirtyCell = extern struct {
    instance_index: u32,
};

const PixelCopyRect = struct {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
};

// ================================================================
// Internal types
// ================================================================

const ATLAS_SIZE: u32 = 2048;
const GLYPH_MAP_CAP: usize = 4096;
const GLYPH_PAD: u32 = 1;
const MAX_CELLS: u32 = 400 * 120; // 400 cols × 120 rows max
const COMMAND_FRAME_COUNT: usize = 2;
const COPY_RECT_FULL_ROW_MIN_COUNT: usize = 6;
const COPY_RECT_FULL_ROW_MIN_COVERAGE_NUM: u32 = 1;
const COPY_RECT_FULL_ROW_MIN_COVERAGE_DEN: u32 = 2;
const COMPUTE_THREADS_PER_GROUP: u32 = 64;

/// Per-cell data uploaded to GPU (matches HLSL CellData).
/// Position and atlas UVs are derived in the shader from the instance id and glyph index.
const GpuCellGpu = extern struct {
    glyph_idx: u32,
    codepoint: u32,
    fg_rgba: u32,
    bg_rgba: u32,
    attrs: u32,
};

const GlyphSlot = struct {
    key: u32, // codepoint
    atlas_index: u32,
    occupied: bool,
};

const CommandFrame = struct {
    cmd_alloc: *dx.ID3D12CommandAllocator,
    cmd_list: *dx.ID3D12GraphicsCommandList,
    fence_value: u64 = 0,
    generation: std.atomic.Value(u32) = std.atomic.Value(u32).init(0),
};

fn hr_hex(hr: dx.HRESULT) u32 {
    return @as(u32, @bitCast(hr));
}

fn blob_message(blob: *dx.ID3DBlob) []const u8 {
    const ptr = blob.GetBufferPointer() orelse return "<no message>";
    const bytes: [*]const u8 = @ptrCast(ptr);
    return bytes[0..blob.GetBufferSize()];
}

// ================================================================
// GpuRenderer
// ================================================================

pub const GpuRenderer = struct {
    alloc: Allocator,

    // DX12 core objects (stored as opaque for safety)
    device: *dx.ID3D12Device,
    cmd_queue: *dx.ID3D12CommandQueue,
    frames: [COMMAND_FRAME_COUNT]CommandFrame,
    frame_cursor: std.atomic.Value(u32),
    fence: *dx.ID3D12Fence,
    fence_event: dx.HANDLE,
    fence_value: u64,

    // Descriptor heaps
    rtv_heap: *dx.ID3D12DescriptorHeap,
    srv_heap: *dx.ID3D12DescriptorHeap,
    rtv_inc: u32,
    srv_inc: u32,

    // Pipeline
    root_sig: *anyopaque, // ID3D12RootSignature
    pso: *anyopaque, // ID3D12PipelineState
    compute_root_sig: *anyopaque, // ID3D12RootSignature
    compute_pso: *anyopaque, // ID3D12PipelineState

    // Render target (offscreen RGBA8)
    rt_texture: *dx.ID3D12Resource,
    readback_bufs: [COMMAND_FRAME_COUNT]*dx.ID3D12Resource,
    width: u32,
    height: u32,
    readback_ptrs: [COMMAND_FRAME_COUNT]?[*]u8, // mapped pointers (persistent, double-buffered)
    readback_active: u32, // index of the readback buffer last written to

    // Cell upload buffer (structured buffer for instanced draw)
    cell_buf_upload: *dx.ID3D12Resource,
    cell_buf_gpu: *dx.ID3D12Resource,
    cell_mapped: ?[*]u8,
    cell_buf_gpu_state: dx.D3D12_RESOURCE_STATES,
    dirty_value_upload: *dx.ID3D12Resource,
    dirty_value_mapped: ?[*]u8,
    dirty_index_upload: *dx.ID3D12Resource,
    dirty_index_mapped: ?[*]u8,

    // Glyph atlas
    atlas_bitmap: []u8, // CPU R8 bitmap
    atlas_texture: *dx.ID3D12Resource,
    atlas_upload: *dx.ID3D12Resource,
    atlas_upload_mapped: ?[*]u8,
    pending_glyphs: std.AutoHashMapUnmanaged(u32, void),
    glyph_map: []GlyphSlot,
    glyph_count: u32,
    atlas_cols: u32,
    atlas_rows: u32,
    atlas_slot_count: u32,
    atlas_dirty: bool,
    atlas_dirty_words: []u64,

    // GDI font selection + DirectWrite glyph rasterization
    gdi_dc: dx.HDC,
    gdi_font: dx.HFONT,
    dwrite_factory: ?*dx.IDWriteFactory,
    dwrite_gdi_interop: ?*dx.IDWriteGdiInterop,
    dwrite_font_face: ?*dx.IDWriteFontFace,
    glyph_cell_w: u32,
    glyph_cell_h: u32,
    glyph_baseline: f32,

    // ---- Initialization ----

    pub fn init(alloc: Allocator, width: u32, height: u32, font_size: f32) ?*GpuRenderer {
        log.info(
            "initializing DX12 GPU renderer width={} height={} font_size={d:.2}",
            .{ width, height, font_size },
        );
        const self = alloc.create(GpuRenderer) catch {
            log.err("failed to allocate GpuRenderer struct", .{});
            setInitError(.alloc_struct, -1);
            return null;
        };
        self.* = undefined;
        self.alloc = alloc;
        self.width = width;
        self.height = height;
        self.fence_value = 0;
        self.frame_cursor = std.atomic.Value(u32).init(0);
        self.atlas_dirty = false;
        self.pending_glyphs = .empty;
        self.glyph_count = 0;
        self.atlas_cols = 0;
        self.atlas_rows = 0;
        self.atlas_slot_count = 0;
        self.readback_ptrs = .{ null, null };
        self.readback_active = 0;
        self.cell_mapped = null;
        self.cell_buf_gpu_state = .COMMON;
        self.dirty_value_mapped = null;
        self.dirty_index_mapped = null;
        self.atlas_upload_mapped = null;
        self.gdi_dc = null;
        self.gdi_font = null;
        self.dwrite_factory = null;
        self.dwrite_gdi_interop = null;
        self.dwrite_font_face = null;
        self.glyph_baseline = 0;

        // Init glyph map
        self.glyph_map = alloc.alloc(GlyphSlot, GLYPH_MAP_CAP) catch {
            log.err("failed to allocate glyph map capacity={}", .{GLYPH_MAP_CAP});
            setInitError(.alloc_glyph_map, -1);
            alloc.destroy(self);
            return null;
        };
        for (self.glyph_map) |*s| s.* = .{ .key = 0, .atlas_index = 0, .occupied = false };

        // Atlas bitmap (CPU)
        self.atlas_bitmap = alloc.alloc(u8, ATLAS_SIZE * ATLAS_SIZE) catch {
            log.err("failed to allocate atlas bitmap bytes={}", .{ATLAS_SIZE * ATLAS_SIZE});
            setInitError(.alloc_atlas_bitmap, -1);
            alloc.free(self.glyph_map);
            alloc.destroy(self);
            return null;
        };
        @memset(self.atlas_bitmap, 0);
        self.atlas_dirty_words = &.{};

        // Init font selection and DirectWrite glyph rasterization
        if (!self.initGdi(font_size)) {
            log.err("GPU renderer initialization aborted during font rasterizer setup", .{});
            // initGdi already called setInitError
            alloc.free(self.atlas_bitmap);
            alloc.free(self.glyph_map);
            alloc.destroy(self);
            return null;
        }

        // Init DX12
        if (!self.initDx12()) {
            log.err("GPU renderer initialization aborted during DX12 setup", .{});
            // initDx12 already called setInitError
            self.deinitGdi();
            alloc.free(self.atlas_bitmap);
            alloc.free(self.glyph_map);
            alloc.destroy(self);
            return null;
        }

        log.info("DX12 GPU renderer initialized successfully", .{});
        return self;
    }

    pub fn deinit(self: *GpuRenderer) void {
        self.waitForGpu();
        self.deinitDx12();
        self.deinitGdi();
        self.pending_glyphs.deinit(self.alloc);
        self.alloc.free(self.atlas_bitmap);
        self.alloc.free(self.glyph_map);
        self.alloc.destroy(self);
    }

    // ---- DirectWrite glyph rasterization ----

    fn initGdi(self: *GpuRenderer, font_size: f32) bool {
        self.gdi_dc = dx.CreateCompatibleDC(null);
        if (self.gdi_dc == null) {
            log.err("initGdi: CreateCompatibleDC failed", .{});
            setInitError(.gdi_create_dc, -1);
            return false;
        }

        // Create font
        const font_name = std.unicode.utf8ToUtf16LeStringLiteral("Consolas");
        self.gdi_font = dx.CreateFontW(
            -@as(i32, @intFromFloat(font_size)),
            0, 0, 0,
            400, // FW_NORMAL
            0, 0, 0,
            0, // DEFAULT_CHARSET
            0, 0,
            5, // CLEARTYPE_QUALITY
            0x31, // FIXED_PITCH | FF_MODERN
            font_name,
        );
        if (self.gdi_font == null) {
            log.err("initGdi: CreateFontW failed font_size={d:.2}", .{font_size});
            setInitError(.gdi_create_font, -1);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }
        _ = dx.SelectObject(self.gdi_dc, self.gdi_font);

        // Measure cell dimensions
        var sz: dx.SIZE = .{};
        const sample = [_]u16{'M'};
        _ = dx.GetTextExtentPoint32W(self.gdi_dc, &sample, 1, &sz);
        self.glyph_cell_w = @intCast(@max(sz.cx, 1));
        self.glyph_cell_h = @intCast(@max(sz.cy, 1));
        const slot_pitch_w = self.glyph_cell_w + GLYPH_PAD;
        if (slot_pitch_w == 0 or slot_pitch_w > ATLAS_SIZE) {
            log.err(
                "initGdi: glyph cell width {} is invalid for atlas size {}",
                .{ self.glyph_cell_w, ATLAS_SIZE },
            );
            setInitError(.dwrite_font_metrics, -1);
            _ = dx.DeleteObject(self.gdi_font);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }
        var factory_raw: ?*anyopaque = null;
        const factory_hr = dx.DWriteCreateFactory(.SHARED, &dx.IID_IDWriteFactory, &factory_raw);
        if (!dx.SUCCEEDED(factory_hr) or factory_raw == null) {
            log.err("initGdi: DWriteCreateFactory failed hr=0x{x}", .{hr_hex(factory_hr)});
            setInitError(.dwrite_create_factory, factory_hr);
            _ = dx.DeleteObject(self.gdi_font);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }
        self.dwrite_factory = @ptrCast(@alignCast(factory_raw.?));

        var interop_raw: ?*anyopaque = null;
        const interop_hr = self.dwrite_factory.?.GetGdiInterop(&interop_raw);
        if (!dx.SUCCEEDED(interop_hr) or interop_raw == null) {
            log.err("initGdi: IDWriteFactory::GetGdiInterop failed hr=0x{x}", .{hr_hex(interop_hr)});
            setInitError(.dwrite_get_gdi_interop, interop_hr);
            _ = self.dwrite_factory.?.Release();
            self.dwrite_factory = null;
            _ = dx.DeleteObject(self.gdi_font);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }
        self.dwrite_gdi_interop = @ptrCast(@alignCast(interop_raw.?));

        var font_face_raw: ?*anyopaque = null;
        const font_face_hr = self.dwrite_gdi_interop.?.CreateFontFaceFromHdc(self.gdi_dc, &font_face_raw);
        if (!dx.SUCCEEDED(font_face_hr) or font_face_raw == null) {
            log.err("initGdi: IDWriteGdiInterop::CreateFontFaceFromHdc failed hr=0x{x}", .{hr_hex(font_face_hr)});
            setInitError(.dwrite_create_font_face, font_face_hr);
            _ = self.dwrite_gdi_interop.?.Release();
            _ = self.dwrite_factory.?.Release();
            self.dwrite_gdi_interop = null;
            self.dwrite_factory = null;
            _ = dx.DeleteObject(self.gdi_font);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }
        self.dwrite_font_face = @ptrCast(@alignCast(font_face_raw.?));

        var metrics: dx.DWRITE_FONT_METRICS = .{};
        const metrics_hr = self.dwrite_font_face.?.GetGdiCompatibleMetrics(font_size, 1.0, null, &metrics);
        if (!dx.SUCCEEDED(metrics_hr)) {
            log.err("initGdi: IDWriteFontFace::GetGdiCompatibleMetrics failed hr=0x{x}", .{hr_hex(metrics_hr)});
            setInitError(.dwrite_font_metrics, metrics_hr);
            _ = self.dwrite_font_face.?.Release();
            _ = self.dwrite_gdi_interop.?.Release();
            _ = self.dwrite_factory.?.Release();
            self.dwrite_font_face = null;
            self.dwrite_gdi_interop = null;
            self.dwrite_factory = null;
            _ = dx.DeleteObject(self.gdi_font);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }

        const metrics_height = @as(u32, metrics.ascent) + @as(u32, metrics.descent);
        self.glyph_cell_h = @max(self.glyph_cell_h, @max(metrics_height, 1));
        self.glyph_baseline = @as(f32, @floatFromInt(@min(metrics.ascent, @as(u16, @intCast(self.glyph_cell_h)))));

        const slot_pitch_h = self.glyph_cell_h + GLYPH_PAD;
        if (slot_pitch_h == 0 or slot_pitch_h > ATLAS_SIZE) {
            log.err(
                "initGdi: glyph cell height {} is invalid for atlas size {}",
                .{ self.glyph_cell_h, ATLAS_SIZE },
            );
            setInitError(.dwrite_font_metrics, -1);
            self.deinitGdi();
            return false;
        }
        self.atlas_cols = ATLAS_SIZE / slot_pitch_w;
        self.atlas_rows = ATLAS_SIZE / slot_pitch_h;
        if (self.atlas_cols == 0 or self.atlas_rows == 0) {
            log.err(
                "initGdi: atlas packing is invalid for glyph pitch {}x{}",
                .{ slot_pitch_w, slot_pitch_h },
            );
            setInitError(.dwrite_font_metrics, -1);
            self.deinitGdi();
            return false;
        }
        self.atlas_slot_count = self.atlas_cols * self.atlas_rows;
        const atlas_dirty_word_count = @as(usize, @intCast((self.atlas_slot_count + 63) / 64));
        self.atlas_dirty_words = self.alloc.alloc(u64, atlas_dirty_word_count) catch {
            log.err("initGdi: failed to allocate atlas dirty bitset words={}", .{atlas_dirty_word_count});
            setInitError(.alloc_atlas_bitmap, -1);
            self.deinitGdi();
            return false;
        };
        @memset(self.atlas_dirty_words, 0);

        log.info(
            "DirectWrite glyph rasterizer ready glyph_cell={}x{} baseline={d:.2} atlas_cols={} atlas_rows={}",
            .{ self.glyph_cell_w, self.glyph_cell_h, self.glyph_baseline, self.atlas_cols, self.atlas_rows },
        );
        return true;
    }

    fn deinitGdi(self: *GpuRenderer) void {
        if (self.atlas_dirty_words.len != 0) self.alloc.free(self.atlas_dirty_words);
        self.atlas_dirty_words = &.{};
        if (self.dwrite_font_face) |font_face| _ = font_face.Release();
        if (self.dwrite_gdi_interop) |interop| _ = interop.Release();
        if (self.dwrite_factory) |factory| _ = factory.Release();
        if (self.gdi_font != null) _ = dx.DeleteObject(self.gdi_font);
        if (self.gdi_dc != null) _ = dx.DeleteDC(self.gdi_dc);
        self.dwrite_font_face = null;
        self.dwrite_gdi_interop = null;
        self.dwrite_factory = null;
        self.gdi_font = null;
        self.gdi_dc = null;
    }

    /// Rasterise a single codepoint and upload to atlas. Returns atlas index + 1.
    /// Index 0 is reserved for "no glyph".
    fn rasterizeGlyph(self: *GpuRenderer, codepoint: u32) ?u32 {
        // Check cache first
        if (self.lookupGlyph(codepoint)) |idx| return idx;

        const w = self.glyph_cell_w;
        const h = self.glyph_cell_h;
        const slot_pitch_w = w + GLYPH_PAD;
        const slot_pitch_h = h + GLYPH_PAD;
        if (self.atlas_cols == 0) return null;
        const total_slots = self.atlas_cols * self.atlas_rows;
        if (self.glyph_count >= total_slots) return null;
        const atlas_index = self.glyph_count;
        const dst_x = (atlas_index % self.atlas_cols) * slot_pitch_w;
        const dst_y = (atlas_index / self.atlas_cols) * slot_pitch_h;

        const font_face = self.dwrite_font_face orelse return null;
        const factory = self.dwrite_factory orelse return null;
        const codepoints = [_]u32{codepoint};
        var glyph_indices = [_]u16{0};
        if (!dx.SUCCEEDED(font_face.GetGlyphIndices(&codepoints, 1, &glyph_indices))) return null;

        const advances = [_]f32{0};
        const offsets = [_]dx.DWRITE_GLYPH_OFFSET{.{}};
        const glyph_run: dx.DWRITE_GLYPH_RUN = .{
            .fontFace = font_face,
            .fontEmSize = @as(f32, @floatFromInt(h)),
            .glyphCount = 1,
            .glyphIndices = &glyph_indices,
            .glyphAdvances = &advances,
            .glyphOffsets = &offsets,
            .isSideways = dx.FALSE,
            .bidiLevel = 0,
        };

        var analysis_raw: ?*anyopaque = null;
        const analysis_hr = factory.CreateGlyphRunAnalysis(
            &glyph_run,
            1.0,
            null,
            .NATURAL_SYMMETRIC,
            .NATURAL,
            0,
            self.glyph_baseline,
            &analysis_raw,
        );
        if (!dx.SUCCEEDED(analysis_hr) or analysis_raw == null) return null;
        const analysis: *dx.IDWriteGlyphRunAnalysis = @ptrCast(@alignCast(analysis_raw.?));
        defer _ = analysis.Release();

        var bounds: dx.RECT = .{ .left = 0, .top = 0, .right = 0, .bottom = 0 };
        const bounds_hr = analysis.GetAlphaTextureBounds(.ALIASED_1x1, &bounds);
        if (!dx.SUCCEEDED(bounds_hr)) return null;

        if (bounds.right > bounds.left and bounds.bottom > bounds.top) {
            const tex_w: u32 = @intCast(bounds.right - bounds.left);
            const tex_h: u32 = @intCast(bounds.bottom - bounds.top);
            const alpha_len = tex_w * tex_h;
            const alpha_buf = self.alloc.alloc(u8, alpha_len) catch return null;
            defer self.alloc.free(alpha_buf);

            const alpha_hr = analysis.CreateAlphaTexture(.ALIASED_1x1, &bounds, alpha_buf.ptr, @intCast(alpha_buf.len));
            if (!dx.SUCCEEDED(alpha_hr)) return null;

            var row: u32 = 0;
            while (row < tex_h) : (row += 1) {
                var col: u32 = 0;
                while (col < tex_w) : (col += 1) {
                    const atlas_px_x = @as(i32, @intCast(dst_x)) + bounds.left + @as(i32, @intCast(col));
                    const atlas_px_y = @as(i32, @intCast(dst_y)) + bounds.top + @as(i32, @intCast(row));
                    if (atlas_px_x < 0 or atlas_px_y < 0) continue;
                    if (atlas_px_x >= @as(i32, @intCast(ATLAS_SIZE)) or atlas_px_y >= @as(i32, @intCast(ATLAS_SIZE))) continue;
                    const dst_idx = @as(usize, @intCast(atlas_px_y)) * ATLAS_SIZE + @as(usize, @intCast(atlas_px_x));
                    self.atlas_bitmap[dst_idx] = alpha_buf[row * tex_w + col];
                }
            }
            self.markAtlasDirty(atlas_index);
        }

        self.insertGlyph(codepoint, atlas_index);
        if (!hasAtlasDirtySlot(self, atlas_index)) self.markAtlasDirty(atlas_index);
        return atlas_index + 1;
    }

    fn markAtlasDirty(self: *GpuRenderer, atlas_index: u32) void {
        if (atlas_index >= self.atlas_slot_count) return;
        if (!self.atlas_dirty) {
            self.atlas_dirty = true;
        }
        const word_index = atlas_index / 64;
        const bit_index = @as(u6, @intCast(atlas_index % 64));
        self.atlas_dirty_words[word_index] |= (@as(u64, 1) << bit_index);
    }

    fn hasAtlasDirtySlot(self: *const GpuRenderer, atlas_index: u32) bool {
        if (!self.atlas_dirty or atlas_index >= self.atlas_slot_count) return false;
        const word_index = atlas_index / 64;
        const bit_index = @as(u6, @intCast(atlas_index % 64));
        return (self.atlas_dirty_words[word_index] & (@as(u64, 1) << bit_index)) != 0;
    }

    fn lookupGlyph(self: *const GpuRenderer, cp: u32) ?u32 {
        var idx = @as(usize, cp % GLYPH_MAP_CAP);
        var probes: usize = 0;
        while (probes < GLYPH_MAP_CAP) : (probes += 1) {
            const s = &self.glyph_map[idx];
            if (!s.occupied) return null;
            if (s.key == cp) return s.atlas_index + 1;
            idx = (idx + 1) % GLYPH_MAP_CAP;
        }
        return null;
    }

    fn insertGlyph(self: *GpuRenderer, cp: u32, atlas_index: u32) void {
        var idx = @as(usize, cp % GLYPH_MAP_CAP);
        var probes: usize = 0;
        while (probes < GLYPH_MAP_CAP) : (probes += 1) {
            const s = &self.glyph_map[idx];
            if (!s.occupied) {
                self.glyph_map[idx] = .{ .key = cp, .atlas_index = atlas_index, .occupied = true };
                self.glyph_count += 1;
                return;
            }
            if (s.key == cp) {
                self.glyph_map[idx].atlas_index = atlas_index;
                return;
            }
            idx = (idx + 1) % GLYPH_MAP_CAP;
        }
    }

    // ---- DX12 Initialization ----

    fn initDx12(self: *GpuRenderer) bool {
        // D3D12 debug layer (enable for diagnostics by setting env ZWG_DX12_DEBUG=1)
        if (@import("builtin").mode == .Debug) {
            var debug_raw: ?*anyopaque = null;
            const dbg_hr = dx.D3D12GetDebugInterface(&dx.IID_ID3D12Debug, &debug_raw);
            if (dx.SUCCEEDED(dbg_hr)) {
                if (debug_raw) |raw| {
                    const debug: *dx.ID3D12Debug = @ptrCast(@alignCast(raw));
                    debug.EnableDebugLayer();
                    _ = debug.Release();
                    log.info("initDx12: D3D12 debug layer enabled", .{});
                }
            }
        }

        // 1. Create device (default adapter, feature level 11.0)
        log.info("initDx12: step 1 create device", .{});
        var device_raw: ?*anyopaque = null;
        const device_hr = dx.D3D12CreateDevice(null, .@"11_0", &dx.IID_ID3D12Device, &device_raw);
        if (!dx.SUCCEEDED(device_hr)) {
            log.warn("initDx12: hardware D3D12CreateDevice failed hr=0x{x}, trying WARP adapter", .{hr_hex(device_hr)});

            // Try WARP (software) adapter as fallback
            var factory_raw: ?*anyopaque = null;
            const factory_hr = dx.CreateDXGIFactory1(&dx.IID_IDXGIFactory4, &factory_raw);
            if (!dx.SUCCEEDED(factory_hr)) {
                log.err("initDx12: CreateDXGIFactory1 also failed hr=0x{x}", .{hr_hex(factory_hr)});
                setInitError(.dx12_create_device_hw, device_hr);
                return false;
            }
            const factory: *dx.IDXGIFactory4 = @ptrCast(@alignCast(factory_raw.?));
            defer _ = factory.Release();

            var warp_raw: ?*anyopaque = null;
            const warp_hr = factory.EnumWarpAdapter(&dx.IID_IDXGIAdapter, &warp_raw);
            if (!dx.SUCCEEDED(warp_hr)) {
                log.err("initDx12: EnumWarpAdapter failed hr=0x{x}", .{hr_hex(warp_hr)});
                setInitError(.dx12_create_device_warp, warp_hr);
                return false;
            }
            defer {
                const adapter: *dx.IUnknown = @ptrCast(@alignCast(warp_raw.?));
                _ = adapter.Release();
            }

            const warp_device_hr = dx.D3D12CreateDevice(warp_raw, .@"11_0", &dx.IID_ID3D12Device, &device_raw);
            if (!dx.SUCCEEDED(warp_device_hr)) {
                log.err("initDx12: WARP D3D12CreateDevice failed hr=0x{x}", .{hr_hex(warp_device_hr)});
                setInitError(.dx12_create_device_warp, warp_device_hr);
                return false;
            }
            log.info("initDx12: using WARP (software) adapter for DX12", .{});
        }
        self.device = @ptrCast(@alignCast(device_raw.?));

        // 2. Command queue
        log.info("initDx12: step 2 create command queue", .{});
        const cq_desc = dx.D3D12_COMMAND_QUEUE_DESC{};
        var cq_raw: ?*anyopaque = null;
        const cq_hr = self.device.CreateCommandQueue(&cq_desc, &dx.IID_ID3D12CommandQueue, &cq_raw);
        if (!dx.SUCCEEDED(cq_hr)) {
            log.err("initDx12: CreateCommandQueue failed hr=0x{x}", .{hr_hex(cq_hr)});
            setInitError(.dx12_command_queue, cq_hr);
            return false;
        }
        self.cmd_queue = @ptrCast(@alignCast(cq_raw.?));

        // 3. Command allocators
        log.info("initDx12: step 3 create command allocators", .{});
        var frame_index: usize = 0;
        while (frame_index < COMMAND_FRAME_COUNT) : (frame_index += 1) {
            var ca_raw: ?*anyopaque = null;
            const ca_hr = self.device.CreateCommandAllocator(
                .DIRECT,
                &dx.IID_ID3D12CommandAllocator,
                &ca_raw,
            );
            if (!dx.SUCCEEDED(ca_hr)) {
                log.err(
                    "initDx12: CreateCommandAllocator[{}] failed hr=0x{x}",
                    .{ frame_index, hr_hex(ca_hr) },
                );
                setInitError(.dx12_command_allocator, ca_hr);
                return false;
            }
            self.frames[frame_index].cmd_alloc = @ptrCast(@alignCast(ca_raw.?));
            self.frames[frame_index].cmd_list = undefined;
            self.frames[frame_index].fence_value = 0;
            self.frames[frame_index].generation = std.atomic.Value(u32).init(0);
        }

        // 4. Descriptor heaps
        log.info("initDx12: step 4 create descriptor heaps", .{});
        const rtv_desc = dx.D3D12_DESCRIPTOR_HEAP_DESC{ .Type = .RTV, .NumDescriptors = 1 };
        var rtv_raw: ?*anyopaque = null;
        const rtv_hr = self.device.CreateDescriptorHeap(&rtv_desc, &dx.IID_ID3D12DescriptorHeap, &rtv_raw);
        if (!dx.SUCCEEDED(rtv_hr)) {
            log.err("initDx12: CreateDescriptorHeap RTV failed hr=0x{x}", .{hr_hex(rtv_hr)});
            setInitError(.dx12_descriptor_heap_rtv, rtv_hr);
            return false;
        }
        self.rtv_heap = @ptrCast(@alignCast(rtv_raw.?));
        self.rtv_inc = self.device.GetDescriptorHandleIncrementSize(.RTV);

        const srv_desc = dx.D3D12_DESCRIPTOR_HEAP_DESC{ .Type = .CBV_SRV_UAV, .NumDescriptors = 5, .Flags = .SHADER_VISIBLE };
        var srv_raw: ?*anyopaque = null;
        const srv_hr = self.device.CreateDescriptorHeap(&srv_desc, &dx.IID_ID3D12DescriptorHeap, &srv_raw);
        if (!dx.SUCCEEDED(srv_hr)) {
            log.err("initDx12: CreateDescriptorHeap SRV failed hr=0x{x}", .{hr_hex(srv_hr)});
            setInitError(.dx12_descriptor_heap_srv, srv_hr);
            return false;
        }
        self.srv_heap = @ptrCast(@alignCast(srv_raw.?));
        self.srv_inc = self.device.GetDescriptorHandleIncrementSize(.CBV_SRV_UAV);

        // 5. Fence
        log.info("initDx12: step 5 create fence", .{});
        var fence_raw: ?*anyopaque = null;
        const fence_hr = self.device.CreateFence(0, .NONE, &dx.IID_ID3D12Fence, &fence_raw);
        if (!dx.SUCCEEDED(fence_hr)) {
            log.err("initDx12: CreateFence failed hr=0x{x}", .{hr_hex(fence_hr)});
            setInitError(.dx12_fence, fence_hr);
            return false;
        }
        self.fence = @ptrCast(@alignCast(fence_raw.?));
        self.fence_event = dx.CreateEventW(null, dx.FALSE, dx.FALSE, null) orelse {
            log.err("initDx12: CreateEventW failed for fence event", .{});
            setInitError(.dx12_fence_event, -1);
            return false;
        };

        // 6. Compile shaders & create PSO
        log.info("initDx12: step 6 create pipeline", .{});
        if (!self.createPipeline()) {
            log.err("initDx12: createPipeline failed", .{});
            // createPipeline already called setInitError
            return false;
        }

        // 7. Create render target + readback
        log.info("initDx12: step 7 create render target", .{});
        if (!self.createRenderTarget()) {
            log.err("initDx12: createRenderTarget failed width={} height={}", .{ self.width, self.height });
            setInitError(.dx12_render_target, -1);
            return false;
        }

        // 8. Create cell buffers
        log.info("initDx12: step 8 create cell buffers", .{});
        if (!self.createCellBuffers()) {
            log.err("initDx12: createCellBuffers failed", .{});
            setInitError(.dx12_cell_buffers, -1);
            return false;
        }

        // 9. Create atlas texture
        log.info("initDx12: step 9 create atlas texture", .{});
        if (!self.createAtlasTexture()) {
            log.err("initDx12: createAtlasTexture failed", .{});
            setInitError(.dx12_atlas_texture, -1);
            return false;
        }

        // 10. Create command lists (initially closed)
        log.info("initDx12: step 10 create command list ring", .{});
        frame_index = 0;
        while (frame_index < COMMAND_FRAME_COUNT) : (frame_index += 1) {
            var cl_raw: ?*anyopaque = null;
            const cl_hr = self.device.CreateCommandList(
                0,
                .DIRECT,
                @ptrCast(self.frames[frame_index].cmd_alloc),
                null,
                &dx.IID_ID3D12GraphicsCommandList,
                &cl_raw,
            );
            if (!dx.SUCCEEDED(cl_hr)) {
                const removed_reason = self.device.GetDeviceRemovedReason();
                log.err(
                    "initDx12: CreateCommandList[{}] failed hr=0x{x} DeviceRemovedReason=0x{x}",
                    .{ frame_index, hr_hex(cl_hr), hr_hex(removed_reason) },
                );
                setInitError(.dx12_command_list, cl_hr);
                return false;
            }
            self.frames[frame_index].cmd_list = @ptrCast(@alignCast(cl_raw.?));
            _ = self.frames[frame_index].cmd_list.Close();
        }

        // Pre-rasterize ASCII glyphs
        log.info("initDx12: step 11 pre-rasterize ASCII glyphs", .{});
        var cp: u32 = 32;
        while (cp < 127) : (cp += 1) {
            _ = self.rasterizeGlyph(cp);
        }
        log.info("initDx12: step 12 upload atlas", .{});
        self.uploadAtlas();

        log.info("DX12 core objects initialized successfully", .{});
        return true;
    }

    fn createPipeline(self: *GpuRenderer) bool {
        // Compile vertex shader
        var vs_blob: ?*dx.ID3DBlob = null;
        var vs_err: ?*dx.ID3DBlob = null;
        const vs_hr = dx.D3DCompile(
            shaders.VS_SOURCE.ptr,
            shaders.VS_SOURCE.len,
            "vs",
            null,
            null,
            "main",
            "vs_5_0",
            0,
            0,
            &vs_blob,
            &vs_err,
        );
        if (!dx.SUCCEEDED(vs_hr)) {
            if (vs_err) |e| {
                log.err(
                    "createPipeline: vertex shader compile failed hr=0x{x} message={s}",
                    .{ hr_hex(vs_hr), blob_message(e) },
                );
                _ = e.Release();
            } else {
                log.err("createPipeline: vertex shader compile failed hr=0x{x}", .{hr_hex(vs_hr)});
            }
            setInitError(.dx12_shader_compile_vs, vs_hr);
            return false;
        }
        defer _ = vs_blob.?.Release();
        if (vs_err) |e| _ = e.Release();

        // Compile pixel shader
        var ps_blob: ?*dx.ID3DBlob = null;
        var ps_err: ?*dx.ID3DBlob = null;
        const ps_hr = dx.D3DCompile(
            shaders.PS_SOURCE.ptr,
            shaders.PS_SOURCE.len,
            "ps",
            null,
            null,
            "main",
            "ps_5_0",
            0,
            0,
            &ps_blob,
            &ps_err,
        );
        if (!dx.SUCCEEDED(ps_hr)) {
            if (ps_err) |e| {
                log.err(
                    "createPipeline: pixel shader compile failed hr=0x{x} message={s}",
                    .{ hr_hex(ps_hr), blob_message(e) },
                );
                _ = e.Release();
            } else {
                log.err("createPipeline: pixel shader compile failed hr=0x{x}", .{hr_hex(ps_hr)});
            }
            setInitError(.dx12_shader_compile_ps, ps_hr);
            return false;
        }
        defer _ = ps_blob.?.Release();
        if (ps_err) |e| _ = e.Release();

        // Root signature: [0] = SRV table (t0 for cells, t1 for atlas), [1] = root CBV (constants)
        var ranges = [_]dx.D3D12_DESCRIPTOR_RANGE{
            .{ .RangeType = .SRV, .NumDescriptors = 2, .BaseShaderRegister = 0 },
        };
        var params = [_]dx.D3D12_ROOT_PARAMETER{
            // Param 0: SRV descriptor table
            .{
                .ParameterType = .DESCRIPTOR_TABLE,
                .u = .{ .DescriptorTable = .{ .NumDescriptorRanges = 1, .pDescriptorRanges = &ranges } },
                .ShaderVisibility = .ALL,
            },
            // Param 1: root constants (12 floats = viewport/cell/atlas metadata)
            .{
                .ParameterType = .@"32BIT_CONSTANTS",
                .u = .{ .Constants = .{ .ShaderRegister = 0, .Num32BitValues = 12 } },
                .ShaderVisibility = .VERTEX,
            },
        };
        var sampler = [_]dx.D3D12_STATIC_SAMPLER_DESC{.{}};
        const rs_desc = dx.D3D12_ROOT_SIGNATURE_DESC{
            .NumParameters = 2,
            .pParameters = &params,
            .NumStaticSamplers = 1,
            .pStaticSamplers = &sampler,
        };

        var sig_blob: ?*dx.ID3DBlob = null;
        var sig_err: ?*dx.ID3DBlob = null;
        const sig_hr = dx.D3D12SerializeRootSignature(&rs_desc, .@"1_0", &sig_blob, &sig_err);
        if (!dx.SUCCEEDED(sig_hr)) {
            if (sig_err) |e| {
                log.err(
                    "createPipeline: D3D12SerializeRootSignature failed hr=0x{x} message={s}",
                    .{ hr_hex(sig_hr), blob_message(e) },
                );
                _ = e.Release();
            } else {
                log.err(
                    "createPipeline: D3D12SerializeRootSignature failed hr=0x{x}",
                    .{hr_hex(sig_hr)},
                );
            }
            setInitError(.dx12_root_sig_serialize, sig_hr);
            return false;
        }
        defer _ = sig_blob.?.Release();
        if (sig_err) |e| _ = e.Release();

        var rs_raw: ?*anyopaque = null;
        const rs_hr = self.device.CreateRootSignature(
            0,
            sig_blob.?.GetBufferPointer(),
            sig_blob.?.GetBufferSize(),
            &dx.IID_ID3D12RootSignature,
            &rs_raw,
        );
        if (!dx.SUCCEEDED(rs_hr)) {
            log.err("createPipeline: CreateRootSignature failed hr=0x{x}", .{hr_hex(rs_hr)});
            setInitError(.dx12_root_sig_create, rs_hr);
            return false;
        }
        self.root_sig = rs_raw.?;

        // Graphics PSO (no vertex input — vertex data comes from StructuredBuffer)
        var pso_desc = dx.D3D12_GRAPHICS_PIPELINE_STATE_DESC{};
        pso_desc.pRootSignature = self.root_sig;
        pso_desc.VS = .{
            .pShaderBytecode = vs_blob.?.GetBufferPointer(),
            .BytecodeLength = vs_blob.?.GetBufferSize(),
        };
        pso_desc.PS = .{
            .pShaderBytecode = ps_blob.?.GetBufferPointer(),
            .BytecodeLength = ps_blob.?.GetBufferSize(),
        };
        pso_desc.RTVFormats[0] = .R8G8B8A8_UNORM;
        // Blend state: alpha blending for text compositing
        pso_desc.BlendState.RenderTarget[0] = .{};

        var pso_raw: ?*anyopaque = null;
        const pso_hr = self.device.CreateGraphicsPipelineState(
            &pso_desc,
            &dx.IID_ID3D12PipelineState,
            &pso_raw,
        );
        if (!dx.SUCCEEDED(pso_hr)) {
            log.err("createPipeline: CreateGraphicsPipelineState failed hr=0x{x}", .{hr_hex(pso_hr)});
            setInitError(.dx12_pso_create, pso_hr);
            return false;
        }
        self.pso = pso_raw.?;

        var compute_srv_ranges = [_]dx.D3D12_DESCRIPTOR_RANGE{
            .{ .RangeType = .SRV, .NumDescriptors = 2, .BaseShaderRegister = 0 },
        };
        var compute_uav_ranges = [_]dx.D3D12_DESCRIPTOR_RANGE{
            .{ .RangeType = .UAV, .NumDescriptors = 1, .BaseShaderRegister = 0 },
        };
        var compute_params = [_]dx.D3D12_ROOT_PARAMETER{
            .{
                .ParameterType = .DESCRIPTOR_TABLE,
                .u = .{ .DescriptorTable = .{ .NumDescriptorRanges = 1, .pDescriptorRanges = &compute_srv_ranges } },
                .ShaderVisibility = .ALL,
            },
            .{
                .ParameterType = .DESCRIPTOR_TABLE,
                .u = .{ .DescriptorTable = .{ .NumDescriptorRanges = 1, .pDescriptorRanges = &compute_uav_ranges } },
                .ShaderVisibility = .ALL,
            },
        };
        const compute_rs_desc = dx.D3D12_ROOT_SIGNATURE_DESC{
            .NumParameters = 2,
            .pParameters = &compute_params,
            .NumStaticSamplers = 0,
            .pStaticSamplers = null,
            .Flags = .ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        };

        var compute_sig_blob: ?*dx.ID3DBlob = null;
        var compute_sig_err: ?*dx.ID3DBlob = null;
        const compute_sig_hr = dx.D3D12SerializeRootSignature(&compute_rs_desc, .@"1_0", &compute_sig_blob, &compute_sig_err);
        if (!dx.SUCCEEDED(compute_sig_hr)) {
            if (compute_sig_err) |e| {
                log.err(
                    "createPipeline: compute root signature serialize failed hr=0x{x} message={s}",
                    .{ hr_hex(compute_sig_hr), blob_message(e) },
                );
                _ = e.Release();
            } else {
                log.err("createPipeline: compute root signature serialize failed hr=0x{x}", .{hr_hex(compute_sig_hr)});
            }
            return false;
        }
        defer _ = compute_sig_blob.?.Release();
        if (compute_sig_err) |e| _ = e.Release();

        var compute_rs_raw: ?*anyopaque = null;
        const compute_rs_hr = self.device.CreateRootSignature(
            0,
            compute_sig_blob.?.GetBufferPointer(),
            compute_sig_blob.?.GetBufferSize(),
            &dx.IID_ID3D12RootSignature,
            &compute_rs_raw,
        );
        if (!dx.SUCCEEDED(compute_rs_hr)) {
            log.err("createPipeline: CreateRootSignature compute failed hr=0x{x}", .{hr_hex(compute_rs_hr)});
            return false;
        }
        self.compute_root_sig = compute_rs_raw.?;

        var cs_blob: ?*dx.ID3DBlob = null;
        var cs_err: ?*dx.ID3DBlob = null;
        const cs_hr = dx.D3DCompile(
            shaders.CS_SOURCE.ptr,
            shaders.CS_SOURCE.len,
            "cs",
            null,
            null,
            "main",
            "cs_5_0",
            0,
            0,
            &cs_blob,
            &cs_err,
        );
        if (!dx.SUCCEEDED(cs_hr)) {
            if (cs_err) |e| {
                log.err(
                    "createPipeline: compute shader compile failed hr=0x{x} message={s}",
                    .{ hr_hex(cs_hr), blob_message(e) },
                );
                _ = e.Release();
            } else {
                log.err("createPipeline: compute shader compile failed hr=0x{x}", .{hr_hex(cs_hr)});
            }
            return false;
        }
        defer _ = cs_blob.?.Release();
        if (cs_err) |e| _ = e.Release();

        var compute_pso_desc = dx.D3D12_COMPUTE_PIPELINE_STATE_DESC{};
        compute_pso_desc.pRootSignature = self.compute_root_sig;
        compute_pso_desc.CS = .{
            .pShaderBytecode = cs_blob.?.GetBufferPointer(),
            .BytecodeLength = cs_blob.?.GetBufferSize(),
        };

        var compute_pso_raw: ?*anyopaque = null;
        const compute_pso_hr = self.device.CreateComputePipelineState(
            &compute_pso_desc,
            &dx.IID_ID3D12PipelineState,
            &compute_pso_raw,
        );
        if (!dx.SUCCEEDED(compute_pso_hr)) {
            log.err("createPipeline: CreateComputePipelineState failed hr=0x{x}", .{hr_hex(compute_pso_hr)});
            return false;
        }
        self.compute_pso = compute_pso_raw.?;

        return true;
    }

    fn createRenderTarget(self: *GpuRenderer) bool {
        // Offscreen render target (RGBA8)
        const rt_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .TEXTURE2D,
            .Width = self.width,
            .Height = self.height,
            .Format = .R8G8B8A8_UNORM,
            .Flags = .ALLOW_RENDER_TARGET,
        };
        const heap_default = dx.D3D12_HEAP_PROPERTIES{ .Type = .DEFAULT };
        const clear_val = dx.D3D12_CLEAR_VALUE{
            .Format = .R8G8B8A8_UNORM,
            .Color = .{ 0.0, 0.0, 0.0, 1.0 },
        };

        var rt_raw: ?*anyopaque = null;
        const rt_hr = self.device.CreateCommittedResource(
            &heap_default,
            .NONE,
            &rt_desc,
            .RENDER_TARGET,
            &clear_val,
            &dx.IID_ID3D12Resource,
            &rt_raw,
        );
        if (!dx.SUCCEEDED(rt_hr)) {
            log.err("createRenderTarget: rt texture allocation failed hr=0x{x}", .{hr_hex(rt_hr)});
            return false;
        }
        self.rt_texture = @ptrCast(@alignCast(rt_raw.?));

        // RTV
        const rtv_handle = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateRenderTargetView(@ptrCast(self.rt_texture), null, rtv_handle);

        // Double-buffered readback buffers — CPU reads from previous frame
        // while GPU writes to the current one, eliminating synchronous fence stalls.
        const row_pitch = self.pixelStride();
        const rb_size = @as(u64, row_pitch) * @as(u64, self.height);
        const rb_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .BUFFER,
            .Width = rb_size,
            .Layout = .ROW_MAJOR, // Required for BUFFER resources
        };
        const heap_readback = dx.D3D12_HEAP_PROPERTIES{ .Type = .READBACK };

        var rb_idx: usize = 0;
        while (rb_idx < COMMAND_FRAME_COUNT) : (rb_idx += 1) {
            var rb_raw: ?*anyopaque = null;
            const rb_hr = self.device.CreateCommittedResource(
                &heap_readback,
                .NONE,
                &rb_desc,
                .COPY_DEST,
                null,
                &dx.IID_ID3D12Resource,
                &rb_raw,
            );
            if (!dx.SUCCEEDED(rb_hr)) {
                log.err("createRenderTarget: readback buffer[{}] allocation failed hr=0x{x}", .{ rb_idx, hr_hex(rb_hr) });
                return false;
            }
            self.readback_bufs[rb_idx] = @ptrCast(@alignCast(rb_raw.?));

            // Map readback buffer persistently
            var mapped: ?*anyopaque = null;
            const map_hr = self.readback_bufs[rb_idx].Map(0, null, &mapped);
            if (dx.SUCCEEDED(map_hr)) {
                self.readback_ptrs[rb_idx] = @ptrCast(mapped);
            } else {
                log.err("createRenderTarget: readback buffer[{}] map failed hr=0x{x}", .{ rb_idx, hr_hex(map_hr) });
                return false;
            }
        }
        self.readback_active = 0;

        return true;
    }

    fn pixelStride(self: *const GpuRenderer) u32 {
        return ((self.width * 4 + 255) / 256) * 256;
    }

    fn createCellBuffers(self: *GpuRenderer) bool {
        const cell_gpu_size = @sizeOf(GpuCellGpu);
        const buf_size = @as(u64, MAX_CELLS) * @as(u64, cell_gpu_size);

        // Upload buffer (CPU writable)
        const heap_upload = dx.D3D12_HEAP_PROPERTIES{ .Type = .UPLOAD };
        const buf_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .BUFFER,
            .Width = buf_size,
            .Layout = .ROW_MAJOR, // Required for BUFFER resources
        };

        var upload_raw: ?*anyopaque = null;
        const upload_hr = self.device.CreateCommittedResource(
            &heap_upload,
            .NONE,
            &buf_desc,
            .GENERIC_READ,
            null,
            &dx.IID_ID3D12Resource,
            &upload_raw,
        );
        if (!dx.SUCCEEDED(upload_hr)) {
            log.err("createCellBuffers: upload buffer allocation failed hr=0x{x}", .{hr_hex(upload_hr)});
            return false;
        }
        self.cell_buf_upload = @ptrCast(@alignCast(upload_raw.?));

        // Map it
        var mapped: ?*anyopaque = null;
        const map_hr = self.cell_buf_upload.Map(0, null, &mapped);
        if (dx.SUCCEEDED(map_hr)) {
            self.cell_mapped = @ptrCast(mapped);
        } else {
            log.err("createCellBuffers: upload buffer map failed hr=0x{x}", .{hr_hex(map_hr)});
            return false;
        }

        // GPU buffer (default heap, SRV/UAV)
        const heap_default = dx.D3D12_HEAP_PROPERTIES{ .Type = .DEFAULT };
        const gpu_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .BUFFER,
            .Width = buf_size,
            .Layout = .ROW_MAJOR,
            .Flags = .ALLOW_UNORDERED_ACCESS,
        };
        var gpu_raw: ?*anyopaque = null;
        const gpu_hr = self.device.CreateCommittedResource(
            &heap_default,
            .NONE,
            &gpu_desc,
            .COMMON,
            null,
            &dx.IID_ID3D12Resource,
            &gpu_raw,
        );
        if (!dx.SUCCEEDED(gpu_hr)) {
            log.err("createCellBuffers: gpu buffer allocation failed hr=0x{x}", .{hr_hex(gpu_hr)});
            return false;
        }
        self.cell_buf_gpu = @ptrCast(@alignCast(gpu_raw.?));

        // Create SRV for cell buffer (slot 0 in srv_heap).
        // The vertex shader reads this as StructuredBuffer<CellData> at t0.
        const srv_desc = dx.D3D12_SHADER_RESOURCE_VIEW_DESC{
            .Format = .UNKNOWN,
            .ViewDimension = .BUFFER,
            .u = .{
                .Buffer = .{
                    .FirstElement = 0,
                    .NumElements = MAX_CELLS,
                    .StructureByteStride = @sizeOf(GpuCellGpu),
                    .Flags = 0,
                },
            },
        };
        const handle = self.srv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateShaderResourceView(@ptrCast(self.cell_buf_gpu), &srv_desc, handle);

        var dirty_value_raw: ?*anyopaque = null;
        const dirty_value_hr = self.device.CreateCommittedResource(
            &heap_upload,
            .NONE,
            &buf_desc,
            .GENERIC_READ,
            null,
            &dx.IID_ID3D12Resource,
            &dirty_value_raw,
        );
        if (!dx.SUCCEEDED(dirty_value_hr)) {
            log.err("createCellBuffers: dirty value upload allocation failed hr=0x{x}", .{hr_hex(dirty_value_hr)});
            return false;
        }
        self.dirty_value_upload = @ptrCast(@alignCast(dirty_value_raw.?));
        mapped = null;
        const dirty_value_map_hr = self.dirty_value_upload.Map(0, null, &mapped);
        if (dx.SUCCEEDED(dirty_value_map_hr)) {
            self.dirty_value_mapped = @ptrCast(mapped);
        } else {
            log.err("createCellBuffers: dirty value upload map failed hr=0x{x}", .{hr_hex(dirty_value_map_hr)});
            return false;
        }

        var dirty_value_handle = self.srv_heap.GetCPUDescriptorHandleForHeapStart();
        dirty_value_handle.ptr += self.srv_inc * 2;
        self.device.CreateShaderResourceView(@ptrCast(self.dirty_value_upload), &srv_desc, dirty_value_handle);

        const index_buf_size = @as(u64, MAX_CELLS) * @as(u64, @sizeOf(u32));
        const index_buf_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .BUFFER,
            .Width = index_buf_size,
            .Layout = .ROW_MAJOR,
        };
        var dirty_index_raw: ?*anyopaque = null;
        const dirty_index_hr = self.device.CreateCommittedResource(
            &heap_upload,
            .NONE,
            &index_buf_desc,
            .GENERIC_READ,
            null,
            &dx.IID_ID3D12Resource,
            &dirty_index_raw,
        );
        if (!dx.SUCCEEDED(dirty_index_hr)) {
            log.err("createCellBuffers: dirty index upload allocation failed hr=0x{x}", .{hr_hex(dirty_index_hr)});
            return false;
        }
        self.dirty_index_upload = @ptrCast(@alignCast(dirty_index_raw.?));
        mapped = null;
        const dirty_index_map_hr = self.dirty_index_upload.Map(0, null, &mapped);
        if (dx.SUCCEEDED(dirty_index_map_hr)) {
            self.dirty_index_mapped = @ptrCast(mapped);
        } else {
            log.err("createCellBuffers: dirty index upload map failed hr=0x{x}", .{hr_hex(dirty_index_map_hr)});
            return false;
        }

        const dirty_index_srv_desc = dx.D3D12_SHADER_RESOURCE_VIEW_DESC{
            .Format = .UNKNOWN,
            .ViewDimension = .BUFFER,
            .u = .{
                .Buffer = .{
                    .FirstElement = 0,
                    .NumElements = MAX_CELLS,
                    .StructureByteStride = @sizeOf(u32),
                    .Flags = 0,
                },
            },
        };
        var dirty_index_handle = self.srv_heap.GetCPUDescriptorHandleForHeapStart();
        dirty_index_handle.ptr += self.srv_inc * 3;
        self.device.CreateShaderResourceView(@ptrCast(self.dirty_index_upload), &dirty_index_srv_desc, dirty_index_handle);

        const dirty_uav_desc = dx.D3D12_UNORDERED_ACCESS_VIEW_DESC{
            .Format = .UNKNOWN,
            .ViewDimension = .BUFFER,
            .u = .{
                .Buffer = .{
                    .FirstElement = 0,
                    .NumElements = MAX_CELLS,
                    .StructureByteStride = @sizeOf(GpuCellGpu),
                    .CounterOffsetInBytes = 0,
                    .Flags = .NONE,
                },
            },
        };
        var dirty_uav_handle = self.srv_heap.GetCPUDescriptorHandleForHeapStart();
        dirty_uav_handle.ptr += self.srv_inc * 4;
        self.device.CreateUnorderedAccessView(@ptrCast(self.cell_buf_gpu), null, &dirty_uav_desc, dirty_uav_handle);

        return true;
    }

    fn createAtlasTexture(self: *GpuRenderer) bool {
        // Atlas GPU texture (R8_UNORM)
        const tex_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .TEXTURE2D,
            .Width = ATLAS_SIZE,
            .Height = ATLAS_SIZE,
            .Format = .R8_UNORM,
        };
        const heap_default = dx.D3D12_HEAP_PROPERTIES{ .Type = .DEFAULT };

        var tex_raw: ?*anyopaque = null;
        const tex_hr = self.device.CreateCommittedResource(
            &heap_default,
            .NONE,
            &tex_desc,
            .COPY_DEST,
            null,
            &dx.IID_ID3D12Resource,
            &tex_raw,
        );
        if (!dx.SUCCEEDED(tex_hr)) {
            log.err("createAtlasTexture: atlas texture allocation failed hr=0x{x}", .{hr_hex(tex_hr)});
            return false;
        }
        self.atlas_texture = @ptrCast(@alignCast(tex_raw.?));

        // Upload buffer for atlas data
        const row_pitch = ((ATLAS_SIZE + 255) / 256) * 256;
        const upload_size = @as(u64, row_pitch) * @as(u64, ATLAS_SIZE);
        const buf_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .BUFFER,
            .Width = upload_size,
            .Layout = .ROW_MAJOR, // Required for BUFFER resources
        };
        const heap_upload = dx.D3D12_HEAP_PROPERTIES{ .Type = .UPLOAD };

        var upload_raw: ?*anyopaque = null;
        const upload_hr = self.device.CreateCommittedResource(
            &heap_upload,
            .NONE,
            &buf_desc,
            .GENERIC_READ,
            null,
            &dx.IID_ID3D12Resource,
            &upload_raw,
        );
        if (!dx.SUCCEEDED(upload_hr)) {
            log.err("createAtlasTexture: atlas upload buffer allocation failed hr=0x{x}", .{hr_hex(upload_hr)});
            return false;
        }
        self.atlas_upload = @ptrCast(@alignCast(upload_raw.?));
        var mapped: ?*anyopaque = null;
        const map_hr = self.atlas_upload.Map(0, null, &mapped);
        if (!dx.SUCCEEDED(map_hr)) {
            log.err("createAtlasTexture: atlas upload buffer map failed hr=0x{x}", .{hr_hex(map_hr)});
            return false;
        }
        self.atlas_upload_mapped = @ptrCast(mapped);

        // Create SRV for atlas texture (slot 1 in srv_heap). The pixel shader samples this at t1.
        const srv_desc = dx.D3D12_SHADER_RESOURCE_VIEW_DESC{
            .Format = .R8_UNORM,
            .ViewDimension = .TEXTURE2D,
            .u = .{
                .Texture2D = .{
                    .MostDetailedMip = 0,
                    .MipLevels = 1,
                    .PlaneSlice = 0,
                    .ResourceMinLODClamp = 0,
                },
            },
        };
        var handle = self.srv_heap.GetCPUDescriptorHandleForHeapStart();
        handle.ptr += self.srv_inc; // slot 1
        self.device.CreateShaderResourceView(@ptrCast(self.atlas_texture), &srv_desc, handle);

        return true;
    }

    fn transitionCellGpuBuffer(
        self: *GpuRenderer,
        frame: *CommandFrame,
        next_state: dx.D3D12_RESOURCE_STATES,
    ) void {
        if (self.cell_buf_gpu_state == next_state) return;
        const barrier = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.cell_buf_gpu), self.cell_buf_gpu_state, next_state),
        };
        frame.cmd_list.ResourceBarrier(1, &barrier);
        self.cell_buf_gpu_state = next_state;
    }

    fn copyCellUploadToGpu(self: *GpuRenderer, frame: *CommandFrame) void {
        self.transitionCellGpuBuffer(frame, .COPY_DEST);
        frame.cmd_list.CopyResource(@ptrCast(self.cell_buf_gpu), @ptrCast(self.cell_buf_upload));
        self.transitionCellGpuBuffer(frame, .GENERIC_READ);
    }

    fn encodeDirtyCellCompute(
        self: *GpuRenderer,
        frame: *CommandFrame,
        cells: [*]const GpuCellData,
        cell_count: u32,
        dirty_cells: [*]const GpuDirtyCell,
        dirty_cell_count: u32,
    ) bool {
        if (dirty_cell_count == 0) return true;
        const dirty_value_ptr = self.dirty_value_mapped orelse return false;
        const dirty_index_ptr = self.dirty_index_mapped orelse return false;
        const dirty_values: [*]GpuCellGpu = @ptrCast(@alignCast(dirty_value_ptr));
        const dirty_indices: [*]u32 = @ptrCast(@alignCast(dirty_index_ptr));
        const clamped_cell_count = @min(cell_count, MAX_CELLS);
        var last_codepoint: u32 = 0;
        var last_glyph_idx: u32 = 0;
        var last_valid = false;
        var dirty_write_count: u32 = 0;
        var dirty_index: u32 = 0;
        while (dirty_index < dirty_cell_count) : (dirty_index += 1) {
            const cell_index = dirty_cells[dirty_index].instance_index;
            if (cell_index >= clamped_cell_count) continue;
            const c = cells[cell_index];
            const glyph_idx = self.resolveGlyphIndexCached(
                c.codepoint,
                &last_codepoint,
                &last_glyph_idx,
                &last_valid,
            );
            dirty_values[dirty_write_count] = .{
                .glyph_idx = glyph_idx,
                .codepoint = c.codepoint,
                .fg_rgba = c.fg_rgba,
                .bg_rgba = c.bg_rgba,
                .attrs = c.flags,
            };
            dirty_indices[dirty_write_count] = cell_index;
            dirty_write_count += 1;
        }
        if (dirty_write_count == 0) return true;

        self.transitionCellGpuBuffer(frame, .UNORDERED_ACCESS);
        const heaps = [_]*anyopaque{@ptrCast(self.srv_heap)};
        frame.cmd_list.SetDescriptorHeaps(1, &heaps);
        frame.cmd_list.SetComputeRootSignature(self.compute_root_sig);
        frame.cmd_list.SetPipelineState(self.compute_pso);

        var compute_srv_handle = self.srv_heap.GetGPUDescriptorHandleForHeapStart();
        compute_srv_handle.ptr += @as(u64, self.srv_inc) * 2;
        frame.cmd_list.SetComputeRootDescriptorTable(0, compute_srv_handle);

        var compute_uav_handle = self.srv_heap.GetGPUDescriptorHandleForHeapStart();
        compute_uav_handle.ptr += @as(u64, self.srv_inc) * 4;
        frame.cmd_list.SetComputeRootDescriptorTable(1, compute_uav_handle);

        const group_count_x = (dirty_write_count + COMPUTE_THREADS_PER_GROUP - 1) / COMPUTE_THREADS_PER_GROUP;
        frame.cmd_list.Dispatch(group_count_x, 1, 1);
        self.transitionCellGpuBuffer(frame, .GENERIC_READ);
        return true;
    }

    fn encodeAtlasUpload(self: *GpuRenderer, frame: *CommandFrame, transition_to_copy_dest: bool) void {
        if (!self.atlas_dirty) return;

        if (transition_to_copy_dest) {
            const to_copy_dest = [_]dx.D3D12_RESOURCE_BARRIER{
                dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.atlas_texture), .PIXEL_SHADER_RESOURCE, .COPY_DEST),
            };
            frame.cmd_list.ResourceBarrier(1, &to_copy_dest);
        }

        const dst = self.atlas_upload_mapped orelse {
            log.err("encodeAtlasUpload: atlas upload buffer is not mapped", .{});
            return;
        };
        const slot_pitch_w = self.glyph_cell_w + GLYPH_PAD;
        const slot_pitch_h = self.glyph_cell_h + GLYPH_PAD;
        const upload_capacity = @as(u64, ((ATLAS_SIZE + 255) / 256) * 256) * @as(u64, ATLAS_SIZE);
        const dst_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.atlas_texture),
            .Type = 0,
            .u = .{ .SubresourceIndex = 0 },
        };

        var upload_offset: u64 = 0;
        var row_index: u32 = 0;
        while (row_index < self.atlas_rows) : (row_index += 1) {
            // Fast dirty-span scan using @ctz on atlas_dirty_words.
            // Skips entire 64-slot words that are clean, then extracts
            // contiguous dirty runs within each word.
            const row_base = row_index * self.atlas_cols;
            var slot_index: u32 = 0;
            while (slot_index < self.atlas_cols) {
                const global_slot = row_base + slot_index;
                const word_idx = global_slot >> 6;
                const bit_idx: u6 = @intCast(global_slot & 63);
                if (word_idx >= self.atlas_dirty_words.len) break;
                // Mask off already-scanned bits in this word
                const word = self.atlas_dirty_words[word_idx] >> bit_idx;
                if (word == 0) {
                    // Skip remaining bits in this word
                    slot_index += 64 - @as(u32, bit_idx);
                    continue;
                }
                // Find first dirty bit
                const first_dirty: u32 = @intCast(@ctz(word));
                slot_index += first_dirty;
                if (slot_index >= self.atlas_cols) break;
                const span_start = slot_index;
                // Find first clean bit after the dirty run
                const shift_amt: u6 = @intCast(@min(first_dirty, 63));
                const inverted = ~(word >> shift_amt);
                const run_len: u32 = if (inverted == 0) 64 - first_dirty - @as(u32, bit_idx) else @intCast(@ctz(inverted));
                slot_index += run_len;
                if (slot_index > self.atlas_cols) slot_index = self.atlas_cols;
                const span_end = slot_index;
                const region_x = span_start * slot_pitch_w;
                const region_y = row_index * slot_pitch_h;
                const region_w = (span_end - span_start) * slot_pitch_w;
                const region_h = slot_pitch_h;
                if (region_w == 0 or region_h == 0) continue;

                const row_pitch = ((region_w + 255) / 256) * 256;
                const region_bytes = @as(u64, row_pitch) * @as(u64, region_h);
                const aligned_offset = ((upload_offset + 511) / 512) * 512;
                if (aligned_offset + region_bytes > upload_capacity) {
                    log.err("encodeAtlasUpload: atlas upload region exceeded capacity region_bytes={} aligned_offset={}", .{ region_bytes, aligned_offset });
                    return;
                } else {
                    upload_offset = aligned_offset;
                }

                var row: u32 = 0;
                while (row < region_h) : (row += 1) {
                    const src_off = (region_y + row) * ATLAS_SIZE + region_x;
                    const dst_off = @as(usize, @intCast(upload_offset + @as(u64, row) * @as(u64, row_pitch)));
                    @memcpy(dst[dst_off .. dst_off + region_w], self.atlas_bitmap[src_off .. src_off + region_w]);
                }

                const src_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
                    .pResource = @ptrCast(self.atlas_upload),
                    .Type = 1,
                    .u = .{
                        .PlacedFootprint = .{
                            .Offset = upload_offset,
                            .Format = .R8_UNORM,
                            .Width = region_w,
                            .Height = region_h,
                            .RowPitch = row_pitch,
                        },
                    },
                };
                frame.cmd_list.CopyTextureRegion(&dst_loc, region_x, region_y, 0, &src_loc, null);
                upload_offset += region_bytes;
            }
        }
        const barrier = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.atlas_texture), .COPY_DEST, .PIXEL_SHADER_RESOURCE),
        };
        frame.cmd_list.ResourceBarrier(1, &barrier);

        self.atlas_dirty = false;
        @memset(self.atlas_dirty_words, 0);
    }

    fn uploadAtlas(self: *GpuRenderer) void {
        if (!self.atlas_dirty) return;
        const frame = self.acquireFrame();
        self.encodeAtlasUpload(frame, false);
        self.submitFrame(frame, true);
    }

    // ---- Frame rendering ----

    fn uploadAtlasIfDirty(self: *GpuRenderer) void {
        if (!self.atlas_dirty) return;
        const frame = self.acquireFrame();
        self.encodeAtlasUpload(frame, true);
        self.submitFrame(frame, true);
    }

    fn collectMissingGlyphsInRange(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        start_instance: u32,
        instance_count: u32,
    ) void {
        if (instance_count == 0) return;
        var i: u32 = 0;
        while (i < instance_count) : (i += 1) {
            const cell_index = start_instance + i;
            const codepoint = cells[cell_index].codepoint;
            if (codepoint == 0) continue;
            if (self.lookupGlyph(codepoint) == null) {
                self.pending_glyphs.put(self.alloc, codepoint, {}) catch {};
            }
        }
    }

    fn collectMissingGlyphsInDamageRects(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        damage_rects: [*]const GpuDamageRect,
        damage_rect_count: u32,
        term_cols: u32,
    ) void {
        if (damage_rect_count == 0) return;
        const clamped_cell_count = @min(cell_count, MAX_CELLS);
        var rect_index: u32 = 0;
        while (rect_index < damage_rect_count) : (rect_index += 1) {
            const rect = damage_rects[rect_index];
            if (rect.col_count == 0 or rect.row_count == 0) continue;
            if (rect.start_col >= term_cols) continue;
            const col_end = @min(term_cols, rect.start_col + rect.col_count);
            var row = rect.row_start;
            while (row < rect.row_start + rect.row_count) : (row += 1) {
                const row_start_index = row * term_cols + rect.start_col;
                if (row_start_index >= clamped_cell_count) break;
                const row_span_len = @min(clamped_cell_count - row_start_index, col_end - rect.start_col);
                self.collectMissingGlyphsInRange(cells, row_start_index, row_span_len);
            }
        }
    }

    fn collectMissingGlyphsInDirtyCells(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        dirty_cells: [*]const GpuDirtyCell,
        dirty_cell_count: u32,
    ) void {
        if (dirty_cell_count == 0) return;
        const clamped_cell_count = @min(cell_count, MAX_CELLS);
        var dirty_index: u32 = 0;
        while (dirty_index < dirty_cell_count) : (dirty_index += 1) {
            const instance_index = dirty_cells[dirty_index].instance_index;
            if (instance_index >= clamped_cell_count) continue;
            self.collectMissingGlyphsInRange(cells, instance_index, 1);
        }
    }

    fn rasterizePendingGlyphs(self: *GpuRenderer) void {
        var iterator = self.pending_glyphs.keyIterator();
        while (iterator.next()) |codepoint| {
            if (self.lookupGlyph(codepoint.*) == null) {
                _ = self.rasterizeGlyph(codepoint.*);
            }
        }
        self.pending_glyphs.clearRetainingCapacity();
    }

    fn resolveGlyphIndexCached(
        self: *GpuRenderer,
        codepoint: u32,
        last_codepoint: *u32,
        last_glyph_idx: *u32,
        last_valid: *bool,
    ) u32 {
        if (codepoint == 0) {
            last_valid.* = false;
            return 0;
        }
        if (last_valid.* and last_codepoint.* == codepoint) {
            return last_glyph_idx.*;
        }
        const glyph_idx = self.lookupGlyph(codepoint) orelse 0;
        last_codepoint.* = codepoint;
        last_glyph_idx.* = glyph_idx;
        last_valid.* = true;
        return glyph_idx;
    }

    fn uploadCellRange(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        start_instance: u32,
        instance_count: u32,
        _: f32,
        _: f32,
    ) void {
        if (instance_count == 0) return;
        if (self.cell_mapped) |mapped| {
            const gpu_cells: [*]GpuCellGpu = @ptrCast(@alignCast(mapped));
            var last_codepoint: u32 = 0;
            var last_glyph_idx: u32 = 0;
            var last_valid = false;

            // Process cells in batches of 4 with prefetch for better cache usage.
            // The glyph lookup itself is hash-map based and cannot be vectorized,
            // but prefetching the next batch of cells while processing the current
            // one hides memory latency (cell data + GPU mapped memory are on
            // separate cache lines).
            const end = start_instance + instance_count;
            var i: u32 = start_instance;
            const batch_end = if (instance_count >= 4) end - 3 else start_instance;
            while (i < batch_end) : (i += 4) {
                // Prefetch the next batch of source cells
                if (i + 8 < end) {
                    @prefetch(cells + (i + 4), .{ .rw = .read, .locality = 1 });
                    @prefetch(cells + (i + 6), .{ .rw = .read, .locality = 1 });
                }

                inline for (0..4) |offset| {
                    const ci = i + offset;
                    const c = cells[ci];
                    const glyph_idx = self.resolveGlyphIndexCached(
                        c.codepoint,
                        &last_codepoint,
                        &last_glyph_idx,
                        &last_valid,
                    );
                    gpu_cells[ci] = .{
                        .glyph_idx = glyph_idx,
                        .codepoint = c.codepoint,
                        .fg_rgba = c.fg_rgba,
                        .bg_rgba = c.bg_rgba,
                        .attrs = c.flags,
                    };
                }
            }
            // Remainder
            while (i < end) : (i += 1) {
                const c = cells[i];
                const glyph_idx = self.resolveGlyphIndexCached(
                    c.codepoint,
                    &last_codepoint,
                    &last_glyph_idx,
                    &last_valid,
                );
                gpu_cells[i] = .{
                    .glyph_idx = glyph_idx,
                    .codepoint = c.codepoint,
                    .fg_rgba = c.fg_rgba,
                    .bg_rgba = c.bg_rgba,
                    .attrs = c.flags,
                };
            }
        }
    }

    fn uploadDamageRects(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        damage_rects: [*]const GpuDamageRect,
        damage_rect_count: u32,
        term_cols: u32,
    ) void {
        if (damage_rect_count == 0) return;
        if (self.cell_mapped) |mapped| {
            const gpu_cells: [*]GpuCellGpu = @ptrCast(@alignCast(mapped));
            const clamped_cell_count = @min(cell_count, MAX_CELLS);
            var last_codepoint: u32 = 0;
            var last_glyph_idx: u32 = 0;
            var last_valid = false;
            var rect_index: u32 = 0;
            while (rect_index < damage_rect_count) : (rect_index += 1) {
                const rect = damage_rects[rect_index];
                if (rect.col_count == 0 or rect.row_count == 0) continue;
                if (rect.start_col >= term_cols) continue;
                const col_end = @min(term_cols, rect.start_col + rect.col_count);
                var row = rect.row_start;
                while (row < rect.row_start + rect.row_count) : (row += 1) {
                    const row_start_index = row * term_cols + rect.start_col;
                    if (row_start_index >= clamped_cell_count) break;
                    const row_span_len = @min(clamped_cell_count - row_start_index, col_end - rect.start_col);
                    var col_offset: u32 = 0;
                    while (col_offset < row_span_len) : (col_offset += 1) {
                        const cell_index = row_start_index + col_offset;
                        const c = cells[cell_index];
                        const glyph_idx = self.resolveGlyphIndexCached(
                            c.codepoint,
                            &last_codepoint,
                            &last_glyph_idx,
                            &last_valid,
                        );
                        gpu_cells[cell_index] = .{
                            .glyph_idx = glyph_idx,
                            .codepoint = c.codepoint,
                            .fg_rgba = c.fg_rgba,
                            .bg_rgba = c.bg_rgba,
                            .attrs = c.flags,
                        };
                    }
                }
            }
        }
    }

    fn uploadDirtyCells(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        dirty_cells: [*]const GpuDirtyCell,
        dirty_cell_count: u32,
    ) void {
        if (dirty_cell_count == 0) return;
        if (self.cell_mapped) |mapped| {
            const gpu_cells: [*]GpuCellGpu = @ptrCast(@alignCast(mapped));
            const clamped_cell_count = @min(cell_count, MAX_CELLS);
            var last_codepoint: u32 = 0;
            var last_glyph_idx: u32 = 0;
            var last_valid = false;
            var dirty_index: u32 = 0;
            while (dirty_index < dirty_cell_count) : (dirty_index += 1) {
                const cell_index = dirty_cells[dirty_index].instance_index;
                if (cell_index >= clamped_cell_count) continue;
                const c = cells[cell_index];
                const glyph_idx = self.resolveGlyphIndexCached(
                    c.codepoint,
                    &last_codepoint,
                    &last_glyph_idx,
                    &last_valid,
                );
                gpu_cells[cell_index] = .{
                    .glyph_idx = glyph_idx,
                    .codepoint = c.codepoint,
                    .fg_rgba = c.fg_rgba,
                    .bg_rgba = c.bg_rgba,
                    .attrs = c.flags,
                };
            }
        }
    }

    fn damageRectsFromDirtyRanges(
        self: *GpuRenderer,
        dirty_ranges: [*]const GpuDirtyRange,
        dirty_range_count: u32,
        cell_count: u32,
        term_cols: u32,
    ) !std.ArrayList(GpuDamageRect) {
        var rects = std.ArrayList(GpuDamageRect).empty;
        errdefer rects.deinit(self.alloc);

        if (term_cols == 0) return rects;

        var range_index: u32 = 0;
        while (range_index < dirty_range_count) : (range_index += 1) {
            const range = dirty_ranges[range_index];
            if (range.instance_count == 0 or range.row_count == 0) continue;
            if (range.start_instance >= cell_count) continue;

            var remaining = @min(range.instance_count, cell_count - range.start_instance);
            var instance = range.start_instance;
            var row_offset: u32 = 0;
            while (row_offset < range.row_count and remaining > 0) : (row_offset += 1) {
                const start_col = if (row_offset == 0) instance % term_cols else 0;
                const col_count = @min(remaining, term_cols - start_col);
                if (col_count == 0) break;
                try rects.append(self.alloc, .{
                    .start_col = start_col,
                    .col_count = col_count,
                    .row_start = range.row_start + row_offset,
                    .row_count = 1,
                });
                remaining -= col_count;
                instance += col_count;
            }
        }

        if (rects.items.len <= 1) return rects;

        std.mem.sort(GpuDamageRect, rects.items, {}, struct {
            fn lessThan(_: void, lhs: GpuDamageRect, rhs: GpuDamageRect) bool {
                return if (lhs.start_col != rhs.start_col)
                    lhs.start_col < rhs.start_col
                else if (lhs.col_count != rhs.col_count)
                    lhs.col_count < rhs.col_count
                else
                    lhs.row_start < rhs.row_start;
            }
        }.lessThan);

        var write_index: usize = 0;
        for (rects.items) |rect| {
            if (write_index == 0) {
                rects.items[0] = rect;
                write_index = 1;
                continue;
            }
            var last = &rects.items[write_index - 1];
            const last_row_end = last.row_start + last.row_count;
            if (last.start_col == rect.start_col and
                last.col_count == rect.col_count and
                rect.row_start <= last_row_end)
            {
                const rect_end = rect.row_start + rect.row_count;
                last.row_count = @max(last.row_count, rect_end - last.row_start);
            } else {
                rects.items[write_index] = rect;
                write_index += 1;
            }
        }
        rects.items.len = write_index;
        return rects;
    }

    fn mergePixelCopyRects(
        self: *GpuRenderer,
        rects: *std.ArrayList(PixelCopyRect),
    ) !void {
        if (rects.items.len <= 1) return;

        std.mem.sort(PixelCopyRect, rects.items, {}, struct {
            fn lessThan(_: void, lhs: PixelCopyRect, rhs: PixelCopyRect) bool {
                return if (lhs.top != rhs.top)
                    lhs.top < rhs.top
                else if (lhs.bottom != rhs.bottom)
                    lhs.bottom < rhs.bottom
                else if (lhs.left != rhs.left)
                    lhs.left < rhs.left
                else
                    lhs.right < rhs.right;
            }
        }.lessThan);

        var band_write_index: usize = 0;
        var band_start: usize = 0;
        while (band_start < rects.items.len) {
            const band_top = rects.items[band_start].top;
            const band_bottom = rects.items[band_start].bottom;
            var band_end = band_start + 1;
            var total_width: u32 = rects.items[band_start].right - rects.items[band_start].left;
            while (band_end < rects.items.len and
                rects.items[band_end].top == band_top and
                rects.items[band_end].bottom == band_bottom)
            {
                total_width += rects.items[band_end].right - rects.items[band_end].left;
                band_end += 1;
            }

            const band_count = band_end - band_start;
            const collapse_to_full_row = band_count >= COPY_RECT_FULL_ROW_MIN_COUNT or
                (band_count >= 3 and total_width * COPY_RECT_FULL_ROW_MIN_COVERAGE_DEN >= self.width * COPY_RECT_FULL_ROW_MIN_COVERAGE_NUM);
            if (collapse_to_full_row) {
                rects.items[band_write_index] = .{
                    .left = 0,
                    .top = band_top,
                    .right = self.width,
                    .bottom = band_bottom,
                };
                band_write_index += 1;
            } else {
                var index = band_start;
                while (index < band_end) : (index += 1) {
                    rects.items[band_write_index] = rects.items[index];
                    band_write_index += 1;
                }
            }
            band_start = band_end;
        }
        rects.items.len = band_write_index;
        if (rects.items.len <= 1) return;

        std.mem.sort(PixelCopyRect, rects.items, {}, struct {
            fn lessThan(_: void, lhs: PixelCopyRect, rhs: PixelCopyRect) bool {
                return if (lhs.left != rhs.left)
                    lhs.left < rhs.left
                else if (lhs.right != rhs.right)
                    lhs.right < rhs.right
                else if (lhs.top != rhs.top)
                    lhs.top < rhs.top
                else
                    lhs.bottom < rhs.bottom;
            }
        }.lessThan);

        var write_index: usize = 0;
        for (rects.items) |rect| {
            if (write_index == 0) {
                rects.items[0] = rect;
                write_index = 1;
                continue;
            }
            var last = &rects.items[write_index - 1];
            if (last.left == rect.left and
                last.right == rect.right and
                rect.top <= last.bottom)
            {
                last.bottom = @max(last.bottom, rect.bottom);
            } else {
                rects.items[write_index] = rect;
                write_index += 1;
            }
        }
        rects.items.len = write_index;
        if (rects.items.len <= 1) return;

        std.mem.sort(PixelCopyRect, rects.items, {}, struct {
            fn lessThan(_: void, lhs: PixelCopyRect, rhs: PixelCopyRect) bool {
                return if (lhs.top != rhs.top)
                    lhs.top < rhs.top
                else if (lhs.bottom != rhs.bottom)
                    lhs.bottom < rhs.bottom
                else if (lhs.left != rhs.left)
                    lhs.left < rhs.left
                else
                    lhs.right < rhs.right;
            }
        }.lessThan);

        write_index = 0;
        for (rects.items) |rect| {
            if (write_index == 0) {
                rects.items[0] = rect;
                write_index = 1;
                continue;
            }
            var last = &rects.items[write_index - 1];
            if (last.top == rect.top and
                last.bottom == rect.bottom and
                rect.left <= last.right)
            {
                last.right = @max(last.right, rect.right);
            } else {
                rects.items[write_index] = rect;
                write_index += 1;
            }
        }
        rects.items.len = write_index;
    }

    /// Render terminal cells to offscreen buffer. Returns RGBA pixel pointer.
    /// Uses double-buffered readback: GPU copies to readback_bufs[active], then
    /// we wait only for that specific frame's fence before returning the pointer.
    /// The previous frame's readback buffer remains available for CPU access
    /// without any GPU stall.
    pub fn renderFrame(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) ?[*]const u8 {
        const count = @min(cell_count, MAX_CELLS);
        if (count == 0) return self.readback_ptrs[self.readback_active];
        if (!self.renderToTexture(cells, cell_count, term_cols, cell_width, cell_height)) {
            return null;
        }

        // Cycle readback buffer index for double-buffering
        const rb_idx = (self.readback_active +% 1) % COMMAND_FRAME_COUNT;

        // Copy RT → readback using a placed-footprint copy; texture->buffer CopyResource is invalid.
        const src_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.rt_texture),
            .Type = 0,
            .u = .{ .SubresourceIndex = 0 },
        };
        const dst_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.readback_bufs[rb_idx]),
            .Type = 1,
            .u = .{
                .PlacedFootprint = .{
                    .Offset = 0,
                    .Format = .R8G8B8A8_UNORM,
                    .Width = self.width,
                    .Height = self.height,
                    .RowPitch = self.pixelStride(),
                },
            },
        };
        const frame = self.acquireFrame();
        frame.cmd_list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, null);
        self.submitFrame(frame, true);
        self.readback_active = @intCast(rb_idx);

        return self.readback_ptrs[rb_idx];
    }

    pub fn renderToTexture(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) bool {
        return self.renderToTarget(
            @ptrCast(self.rt_texture),
            .COPY_SOURCE,
            .RENDER_TARGET,
            .RENDER_TARGET,
            .COPY_SOURCE,
            cells,
            cell_count,
            term_cols,
            cell_width,
            cell_height,
        );
    }

    pub fn renderToSurface(
        self: *GpuRenderer,
        target_resource: *anyopaque,
        cells: [*]const GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) bool {
        return self.renderToTarget(
            target_resource,
            .COMMON,
            .RENDER_TARGET,
            .RENDER_TARGET,
            .COMMON,
            cells,
            cell_count,
            term_cols,
            cell_width,
            cell_height,
        );
    }

    pub fn renderToSurfaceDelta(
        self: *GpuRenderer,
        target_resource: *anyopaque,
        cells: [*]const GpuCellData,
        cell_count: u32,
        damage_rects: [*]const GpuDamageRect,
        damage_rect_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) bool {
        const count = @min(cell_count, MAX_CELLS);
        if (count == 0 or damage_rect_count == 0) return true;

        self.pending_glyphs.clearRetainingCapacity();
        self.collectMissingGlyphsInDamageRects(cells, count, damage_rects, damage_rect_count, term_cols);
        self.rasterizePendingGlyphs();
        self.uploadDamageRects(cells, count, damage_rects, damage_rect_count, term_cols);
        self.uploadAtlasIfDirty();

        var clear_rects = std.ArrayList(dx.RECT).empty;
        defer clear_rects.deinit(self.alloc);

        var rect_index: u32 = 0;
        while (rect_index < damage_rect_count) : (rect_index += 1) {
            const rect = damage_rects[rect_index];
            if (rect.col_count == 0 or rect.row_count == 0) continue;
            if (rect.start_col >= term_cols) continue;

            const left_f = @as(f32, @floatFromInt(rect.start_col)) * cell_width;
            const right_f = @as(f32, @floatFromInt(@min(term_cols, rect.start_col + rect.col_count))) * cell_width;
            const top_f = @as(f32, @floatFromInt(rect.row_start)) * cell_height;
            const bottom_f = @as(f32, @floatFromInt(rect.row_start + rect.row_count)) * cell_height;
            const left = @as(i32, @intFromFloat(@floor(left_f)));
            const right = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.width)), right_f))));
            const top = @as(i32, @intFromFloat(@floor(top_f)));
            const bottom = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.height)), bottom_f))));
            if (right <= left or bottom <= top) continue;

            clear_rects.append(self.alloc, .{
                .left = left,
                .top = top,
                .right = right,
                .bottom = bottom,
            }) catch return false;
        }

        if (clear_rects.items.len == 0) return true;

        const frame = self.acquireFrame();
        self.copyCellUploadToGpu(frame);

        const b_rt = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(target_resource, .COMMON, .RENDER_TARGET),
        };
        frame.cmd_list.ResourceBarrier(1, &b_rt);

        const rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateRenderTargetView(target_resource, null, rtv);
        const clear_color = [4]f32{ 0.0, 0.0, 0.0, 1.0 };
        frame.cmd_list.ClearRenderTargetView(
            rtv,
            &clear_color,
            @intCast(clear_rects.items.len),
            clear_rects.items.ptr,
        );
        frame.cmd_list.OMSetRenderTargets(1, @ptrCast(&rtv), dx.FALSE, null);

        const viewport = [_]dx.D3D12_VIEWPORT{.{
            .Width = @floatFromInt(self.width),
            .Height = @floatFromInt(self.height),
        }};
        frame.cmd_list.RSSetViewports(1, &viewport);

        frame.cmd_list.SetGraphicsRootSignature(self.root_sig);
        frame.cmd_list.SetPipelineState(self.pso);
        frame.cmd_list.IASetPrimitiveTopology(.TRIANGLELIST);

        const heaps = [_]*anyopaque{@ptrCast(self.srv_heap)};
        frame.cmd_list.SetDescriptorHeaps(1, &heaps);
        frame.cmd_list.SetGraphicsRootDescriptorTable(0, self.srv_heap.GetGPUDescriptorHandleForHeapStart());

        const slot_pitch_w = @as(f32, @floatFromInt(self.glyph_cell_w + GLYPH_PAD));
        const slot_pitch_h = @as(f32, @floatFromInt(self.glyph_cell_h + GLYPH_PAD));
        const constants = [12]f32{
            @floatFromInt(self.width),
            @floatFromInt(self.height),
            cell_width,
            cell_height,
            slot_pitch_w / @as(f32, @floatFromInt(ATLAS_SIZE)),
            slot_pitch_h / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @as(f32, @floatFromInt(self.glyph_cell_w)) / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @as(f32, @floatFromInt(self.glyph_cell_h)) / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @floatFromInt(term_cols),
            @floatFromInt(self.atlas_cols),
            0,
            0,
        };
        frame.cmd_list.SetGraphicsRoot32BitConstants(1, 12, &constants, 0);

        rect_index = 0;
        while (rect_index < damage_rect_count) : (rect_index += 1) {
            const rect = damage_rects[rect_index];
            if (rect.col_count == 0 or rect.row_count == 0) continue;
            if (rect.start_col >= term_cols) continue;
            const col_end = @min(term_cols, rect.start_col + rect.col_count);
            const draw_scissor = [_]dx.RECT{.{
                .left = @as(i32, @intFromFloat(@floor(@as(f32, @floatFromInt(rect.start_col)) * cell_width))),
                .top = @as(i32, @intFromFloat(@floor(@as(f32, @floatFromInt(rect.row_start)) * cell_height))),
                .right = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.width)), @as(f32, @floatFromInt(col_end)) * cell_width)))),
                .bottom = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.height)), @as(f32, @floatFromInt(rect.row_start + rect.row_count)) * cell_height)))),
            }};
            frame.cmd_list.RSSetScissorRects(1, &draw_scissor);
            var row = rect.row_start;
            while (row < rect.row_start + rect.row_count) : (row += 1) {
                const instance_start = row * term_cols + rect.start_col;
                if (instance_start >= count) break;
                const available = count - instance_start;
                const instance_count = @min(available, col_end - rect.start_col);
                if (instance_count == 0) continue;
                frame.cmd_list.DrawInstanced(6, instance_count, 0, instance_start);
            }
        }

        const b_present = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(target_resource, .RENDER_TARGET, .COMMON),
        };
        frame.cmd_list.ResourceBarrier(1, &b_present);
        self.submitFrame(frame, false);
        return true;
    }

    pub fn renderToSurfaceDeltaCells(
        self: *GpuRenderer,
        target_resource: *anyopaque,
        cells: [*]const GpuCellData,
        cell_count: u32,
        dirty_cells: [*]const GpuDirtyCell,
        dirty_cell_count: u32,
        damage_rects: [*]const GpuDamageRect,
        damage_rect_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) bool {
        const count = @min(cell_count, MAX_CELLS);
        if (count == 0 or damage_rect_count == 0) return true;
        if (dirty_cell_count == 0) {
            return self.renderToSurfaceDelta(
                target_resource,
                cells,
                cell_count,
                damage_rects,
                damage_rect_count,
                term_cols,
                cell_width,
                cell_height,
            );
        }

        self.pending_glyphs.clearRetainingCapacity();
        self.collectMissingGlyphsInDirtyCells(cells, count, dirty_cells, dirty_cell_count);
        self.rasterizePendingGlyphs();
        self.uploadAtlasIfDirty();

        var clear_rects = std.ArrayList(dx.RECT).empty;
        defer clear_rects.deinit(self.alloc);

        var rect_index: u32 = 0;
        while (rect_index < damage_rect_count) : (rect_index += 1) {
            const rect = damage_rects[rect_index];
            if (rect.col_count == 0 or rect.row_count == 0) continue;
            if (rect.start_col >= term_cols) continue;

            const left_f = @as(f32, @floatFromInt(rect.start_col)) * cell_width;
            const right_f = @as(f32, @floatFromInt(@min(term_cols, rect.start_col + rect.col_count))) * cell_width;
            const top_f = @as(f32, @floatFromInt(rect.row_start)) * cell_height;
            const bottom_f = @as(f32, @floatFromInt(rect.row_start + rect.row_count)) * cell_height;
            const left = @as(i32, @intFromFloat(@floor(left_f)));
            const right = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.width)), right_f))));
            const top = @as(i32, @intFromFloat(@floor(top_f)));
            const bottom = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.height)), bottom_f))));
            if (right <= left or bottom <= top) continue;

            clear_rects.append(self.alloc, .{
                .left = left,
                .top = top,
                .right = right,
                .bottom = bottom,
            }) catch return false;
        }

        if (clear_rects.items.len == 0) return true;

        const frame = self.acquireFrame();

        if (!self.encodeDirtyCellCompute(frame, cells, count, dirty_cells, dirty_cell_count)) {
            return false;
        }

        const b_rt = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(target_resource, .COMMON, .RENDER_TARGET),
        };
        frame.cmd_list.ResourceBarrier(1, &b_rt);

        const rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateRenderTargetView(target_resource, null, rtv);
        const clear_color = [4]f32{ 0.0, 0.0, 0.0, 1.0 };
        frame.cmd_list.ClearRenderTargetView(
            rtv,
            &clear_color,
            @intCast(clear_rects.items.len),
            clear_rects.items.ptr,
        );
        frame.cmd_list.OMSetRenderTargets(1, @ptrCast(&rtv), dx.FALSE, null);

        const viewport = [_]dx.D3D12_VIEWPORT{.{
            .Width = @floatFromInt(self.width),
            .Height = @floatFromInt(self.height),
        }};
        frame.cmd_list.RSSetViewports(1, &viewport);

        frame.cmd_list.SetGraphicsRootSignature(self.root_sig);
        frame.cmd_list.SetPipelineState(self.pso);
        frame.cmd_list.IASetPrimitiveTopology(.TRIANGLELIST);

        const heaps = [_]*anyopaque{@ptrCast(self.srv_heap)};
        frame.cmd_list.SetDescriptorHeaps(1, &heaps);
        frame.cmd_list.SetGraphicsRootDescriptorTable(0, self.srv_heap.GetGPUDescriptorHandleForHeapStart());

        const slot_pitch_w = @as(f32, @floatFromInt(self.glyph_cell_w + GLYPH_PAD));
        const slot_pitch_h = @as(f32, @floatFromInt(self.glyph_cell_h + GLYPH_PAD));
        const constants = [12]f32{
            @floatFromInt(self.width),
            @floatFromInt(self.height),
            cell_width,
            cell_height,
            slot_pitch_w / @as(f32, @floatFromInt(ATLAS_SIZE)),
            slot_pitch_h / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @as(f32, @floatFromInt(self.glyph_cell_w)) / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @as(f32, @floatFromInt(self.glyph_cell_h)) / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @floatFromInt(term_cols),
            @floatFromInt(self.atlas_cols),
            0,
            0,
        };
        frame.cmd_list.SetGraphicsRoot32BitConstants(1, 12, &constants, 0);

        rect_index = 0;
        while (rect_index < damage_rect_count) : (rect_index += 1) {
            const rect = damage_rects[rect_index];
            if (rect.col_count == 0 or rect.row_count == 0) continue;
            if (rect.start_col >= term_cols) continue;
            const col_end = @min(term_cols, rect.start_col + rect.col_count);
            const draw_scissor = [_]dx.RECT{.{
                .left = @as(i32, @intFromFloat(@floor(@as(f32, @floatFromInt(rect.start_col)) * cell_width))),
                .top = @as(i32, @intFromFloat(@floor(@as(f32, @floatFromInt(rect.row_start)) * cell_height))),
                .right = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.width)), @as(f32, @floatFromInt(col_end)) * cell_width)))),
                .bottom = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.height)), @as(f32, @floatFromInt(rect.row_start + rect.row_count)) * cell_height)))),
            }};
            frame.cmd_list.RSSetScissorRects(1, &draw_scissor);
            var row = rect.row_start;
            while (row < rect.row_start + rect.row_count) : (row += 1) {
                const instance_start = row * term_cols + rect.start_col;
                if (instance_start >= count) break;
                const available = count - instance_start;
                const instance_count = @min(available, col_end - rect.start_col);
                if (instance_count == 0) continue;
                frame.cmd_list.DrawInstanced(6, instance_count, 0, instance_start);
            }
        }

        const b_present = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(target_resource, .RENDER_TARGET, .COMMON),
        };
        frame.cmd_list.ResourceBarrier(1, &b_present);
        self.submitFrame(frame, false);
        return true;
    }

    fn renderToTarget(
        self: *GpuRenderer,
        target_resource: *anyopaque,
        before_clear: dx.D3D12_RESOURCE_STATES,
        draw_state: dx.D3D12_RESOURCE_STATES,
        before_finish: dx.D3D12_RESOURCE_STATES,
        after_finish: dx.D3D12_RESOURCE_STATES,
        cells: [*]const GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) bool {
        const count = @min(cell_count, MAX_CELLS);
        if (count == 0) return true;

        self.pending_glyphs.clearRetainingCapacity();
        self.collectMissingGlyphsInRange(cells, 0, count);
        self.rasterizePendingGlyphs();
        self.uploadCellRange(cells, 0, count, cell_width, cell_height);
        self.uploadAtlasIfDirty();

        const frame = self.acquireFrame();
        self.copyCellUploadToGpu(frame);

        const b_rt = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(target_resource, before_clear, draw_state),
        };
        frame.cmd_list.ResourceBarrier(1, &b_rt);

        const rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateRenderTargetView(target_resource, null, rtv);
        const clear_color = [4]f32{ 0.0, 0.0, 0.0, 1.0 };
        frame.cmd_list.ClearRenderTargetView(rtv, &clear_color, 0, null);
        frame.cmd_list.OMSetRenderTargets(1, @ptrCast(&rtv), dx.FALSE, null);

        const viewport = [_]dx.D3D12_VIEWPORT{.{
            .Width = @floatFromInt(self.width),
            .Height = @floatFromInt(self.height),
        }};
        const scissor = [_]dx.RECT{.{
            .left = 0,
            .top = 0,
            .right = @intCast(self.width),
            .bottom = @intCast(self.height),
        }};
        frame.cmd_list.RSSetViewports(1, &viewport);
        frame.cmd_list.RSSetScissorRects(1, &scissor);

        frame.cmd_list.SetGraphicsRootSignature(self.root_sig);
        frame.cmd_list.SetPipelineState(self.pso);
        frame.cmd_list.IASetPrimitiveTopology(.TRIANGLELIST);

        const heaps = [_]*anyopaque{@ptrCast(self.srv_heap)};
        frame.cmd_list.SetDescriptorHeaps(1, &heaps);
        frame.cmd_list.SetGraphicsRootDescriptorTable(0, self.srv_heap.GetGPUDescriptorHandleForHeapStart());

        const slot_pitch_w = @as(f32, @floatFromInt(self.glyph_cell_w + GLYPH_PAD));
        const slot_pitch_h = @as(f32, @floatFromInt(self.glyph_cell_h + GLYPH_PAD));
        const constants = [12]f32{
            @floatFromInt(self.width),
            @floatFromInt(self.height),
            cell_width,
            cell_height,
            slot_pitch_w / @as(f32, @floatFromInt(ATLAS_SIZE)),
            slot_pitch_h / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @as(f32, @floatFromInt(self.glyph_cell_w)) / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @as(f32, @floatFromInt(self.glyph_cell_h)) / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @floatFromInt(term_cols),
            @floatFromInt(self.atlas_cols),
            0,
            0,
        };
        frame.cmd_list.SetGraphicsRoot32BitConstants(1, 12, &constants, 0);
        frame.cmd_list.DrawInstanced(6, count, 0, 0);

        const b_copy = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(target_resource, before_finish, after_finish),
        };
        frame.cmd_list.ResourceBarrier(1, &b_copy);
        self.submitFrame(frame, after_finish != .COMMON);
        return true;
    }

    pub fn renderFrameDelta(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        dirty_ranges: [*]const GpuDirtyRange,
        dirty_range_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) ?[*]const u8 {
        const count = @min(cell_count, MAX_CELLS);
        if (count == 0) return self.readback_ptrs[self.readback_active];
        if (dirty_range_count == 0) return self.renderFrame(cells, cell_count, term_cols, cell_width, cell_height);

        // Cycle readback buffer for double-buffering
        const rb_idx = (self.readback_active +% 1) % COMMAND_FRAME_COUNT;

        var damage_rects = self.damageRectsFromDirtyRanges(
            dirty_ranges,
            dirty_range_count,
            count,
            term_cols,
        ) catch return null;
        defer damage_rects.deinit(self.alloc);
        if (damage_rects.items.len == 0) return self.readback_ptrs[self.readback_active];

        self.pending_glyphs.clearRetainingCapacity();
        self.collectMissingGlyphsInDamageRects(cells, count, damage_rects.items.ptr, @intCast(damage_rects.items.len), term_cols);
        self.rasterizePendingGlyphs();
        self.uploadDamageRects(cells, count, damage_rects.items.ptr, @intCast(damage_rects.items.len), term_cols);
        self.uploadAtlasIfDirty();

        const frame = self.acquireFrame();
        self.copyCellUploadToGpu(frame);

        const b_rt = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.rt_texture), .COPY_SOURCE, .RENDER_TARGET),
        };
        frame.cmd_list.ResourceBarrier(1, &b_rt);

        const rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        const clear_color = [4]f32{ 0.0, 0.0, 0.0, 1.0 };
        var dirty_rects = std.ArrayList(dx.RECT).empty;
        defer dirty_rects.deinit(self.alloc);
        for (damage_rects.items) |rect| {
            const left_f = @as(f32, @floatFromInt(rect.start_col)) * cell_width;
            const right_f = @as(f32, @floatFromInt(@min(term_cols, rect.start_col + rect.col_count))) * cell_width;
            const top_f = @as(f32, @floatFromInt(rect.row_start)) * cell_height;
            const bottom_f = @as(f32, @floatFromInt(rect.row_start + rect.row_count)) * cell_height;
            const left = @as(i32, @intFromFloat(@floor(left_f)));
            const right = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.width)), right_f))));
            const top = @as(i32, @intFromFloat(@floor(top_f)));
            const bottom = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.height)), bottom_f))));
            if (right <= left or bottom <= top) continue;
            dirty_rects.append(self.alloc, .{
                .left = left,
                .top = top,
                .right = right,
                .bottom = bottom,
            }) catch return null;
        }
        if (dirty_rects.items.len == 0) return self.readback_ptrs[self.readback_active];
        frame.cmd_list.ClearRenderTargetView(
            rtv,
            &clear_color,
            @intCast(dirty_rects.items.len),
            dirty_rects.items.ptr,
        );
        frame.cmd_list.OMSetRenderTargets(1, @ptrCast(&rtv), dx.FALSE, null);

        const viewport = [_]dx.D3D12_VIEWPORT{.{
            .Width = @floatFromInt(self.width),
            .Height = @floatFromInt(self.height),
        }};
        const scissor = [_]dx.RECT{.{
            .left = 0,
            .top = 0,
            .right = @intCast(self.width),
            .bottom = @intCast(self.height),
        }};
        frame.cmd_list.RSSetViewports(1, &viewport);
        frame.cmd_list.RSSetScissorRects(1, &scissor);
        frame.cmd_list.SetGraphicsRootSignature(self.root_sig);
        frame.cmd_list.SetPipelineState(self.pso);
        frame.cmd_list.IASetPrimitiveTopology(.TRIANGLELIST);

        const heaps = [_]*anyopaque{@ptrCast(self.srv_heap)};
        frame.cmd_list.SetDescriptorHeaps(1, &heaps);
        frame.cmd_list.SetGraphicsRootDescriptorTable(0, self.srv_heap.GetGPUDescriptorHandleForHeapStart());

        const slot_pitch_w = @as(f32, @floatFromInt(self.glyph_cell_w + GLYPH_PAD));
        const slot_pitch_h = @as(f32, @floatFromInt(self.glyph_cell_h + GLYPH_PAD));
        const constants = [12]f32{
            @floatFromInt(self.width),
            @floatFromInt(self.height),
            cell_width,
            cell_height,
            slot_pitch_w / @as(f32, @floatFromInt(ATLAS_SIZE)),
            slot_pitch_h / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @as(f32, @floatFromInt(self.glyph_cell_w)) / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @as(f32, @floatFromInt(self.glyph_cell_h)) / @as(f32, @floatFromInt(ATLAS_SIZE)),
            @floatFromInt(term_cols),
            @floatFromInt(self.atlas_cols),
            0,
            0,
        };
        frame.cmd_list.SetGraphicsRoot32BitConstants(1, 12, &constants, 0);

        for (damage_rects.items) |rect| {
            if (rect.start_col >= term_cols) continue;
            const col_end = @min(term_cols, rect.start_col + rect.col_count);
            const draw_scissor = [_]dx.RECT{.{
                .left = @as(i32, @intFromFloat(@floor(@as(f32, @floatFromInt(rect.start_col)) * cell_width))),
                .top = @as(i32, @intFromFloat(@floor(@as(f32, @floatFromInt(rect.row_start)) * cell_height))),
                .right = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.width)), @as(f32, @floatFromInt(col_end)) * cell_width)))),
                .bottom = @as(i32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.height)), @as(f32, @floatFromInt(rect.row_start + rect.row_count)) * cell_height)))),
            }};
            frame.cmd_list.RSSetScissorRects(1, &draw_scissor);
            var row = rect.row_start;
            while (row < rect.row_start + rect.row_count) : (row += 1) {
                const instance_start = row * term_cols + rect.start_col;
                if (instance_start >= count) break;
                const available = count - instance_start;
                const instance_count = @min(available, col_end - rect.start_col);
                if (instance_count == 0) continue;
                frame.cmd_list.DrawInstanced(6, instance_count, 0, instance_start);
            }
        }

        const b_copy = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.rt_texture), .RENDER_TARGET, .COPY_SOURCE),
        };
        frame.cmd_list.ResourceBarrier(1, &b_copy);
        const row_pitch = self.pixelStride();
        const src_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.rt_texture),
            .Type = 0,
            .u = .{ .SubresourceIndex = 0 },
        };
        const dst_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.readback_bufs[rb_idx]),
            .Type = 1,
            .u = .{
                .PlacedFootprint = .{
                    .Offset = 0,
                    .Format = .R8G8B8A8_UNORM,
                    .Width = self.width,
                    .Height = self.height,
                    .RowPitch = row_pitch,
                },
            },
        };
        var copy_rects = std.ArrayList(PixelCopyRect).empty;
        defer copy_rects.deinit(self.alloc);
        for (damage_rects.items) |rect| {
            if (rect.start_col >= term_cols) continue;
            const col_end = @min(term_cols, rect.start_col + rect.col_count);
            const src_left = @min(self.width, @as(u32, @intFromFloat(@floor(@as(f32, @floatFromInt(rect.start_col)) * cell_width))));
            const src_top = @min(self.height, @as(u32, @intFromFloat(@floor(@as(f32, @floatFromInt(rect.row_start)) * cell_height))));
            const src_right = @min(self.width, @as(u32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.width)), @as(f32, @floatFromInt(col_end)) * cell_width)))));
            const src_bottom = @min(self.height, @as(u32, @intFromFloat(@ceil(@min(@as(f32, @floatFromInt(self.height)), @as(f32, @floatFromInt(rect.row_start + rect.row_count)) * cell_height)))));
            if (src_right <= src_left or src_bottom <= src_top) continue;
            copy_rects.append(self.alloc, .{
                .left = src_left,
                .top = src_top,
                .right = src_right,
                .bottom = src_bottom,
            }) catch return null;
        }
        try self.mergePixelCopyRects(&copy_rects);
        for (copy_rects.items) |rect| {
            const src_box = dx.D3D12_BOX{
                .left = rect.left,
                .top = rect.top,
                .right = rect.right,
                .bottom = rect.bottom,
            };
            frame.cmd_list.CopyTextureRegion(&dst_loc, rect.left, rect.top, 0, &src_loc, &src_box);
        }

        self.submitFrame(frame, true);
        self.readback_active = @intCast(rb_idx);

        return self.readback_ptrs[rb_idx];
    }

    // ---- Helpers ----

    fn acquireFrame(self: *GpuRenderer) *CommandFrame {
        const frame_idx = self.frame_cursor.fetchAdd(1, .monotonic) % COMMAND_FRAME_COUNT;
        const frame = &self.frames[frame_idx];
        self.waitForFrame(frame);
        _ = frame.cmd_alloc.Reset();
        _ = frame.cmd_list.Reset(@ptrCast(frame.cmd_alloc), null);
        return frame;
    }

    fn submitFrame(self: *GpuRenderer, frame: *CommandFrame, wait: bool) void {
        _ = frame.cmd_list.Close();
        const lists = [_]*anyopaque{@ptrCast(frame.cmd_list)};
        self.cmd_queue.ExecuteCommandLists(1, &lists);
        self.fence_value += 1;
        frame.fence_value = self.fence_value;
        _ = frame.generation.fetchAdd(1, .monotonic);
        _ = self.cmd_queue.Signal(@ptrCast(self.fence), frame.fence_value);
        if (wait) self.waitForFrame(frame);
    }

    fn waitForFrame(self: *GpuRenderer, frame: *CommandFrame) void {
        if (frame.fence_value == 0) return;
        if (self.fence.GetCompletedValue() < frame.fence_value) {
            _ = self.fence.SetEventOnCompletion(frame.fence_value, self.fence_event);
            // Use 100ms timeout instead of INFINITE to detect GPU hangs early
            // and avoid blocking the render thread indefinitely. Normal frames
            // complete in <2ms; 100ms gives ample headroom.
            const result = dx.WaitForSingleObject(self.fence_event, 100);
            if (result != 0) { // WAIT_OBJECT_0 == 0
                log.warn("waitForFrame: fence wait timed out or failed (result=0x{x}, fence_value={})", .{
                    @as(u32, @bitCast(result)),
                    frame.fence_value,
                });
                // Spin-poll for up to 500ms more before giving up
                var spin: u32 = 0;
                while (spin < 50) : (spin += 1) {
                    if (self.fence.GetCompletedValue() >= frame.fence_value) break;
                    std.Thread.sleep(10 * std.time.ns_per_ms);
                }
            }
        }
    }

    fn waitForGpu(self: *GpuRenderer) void {
        for (&self.frames) |*frame| {
            self.waitForFrame(frame);
        }
    }

    fn deinitDx12(self: *GpuRenderer) void {
        // Release in reverse order
        if (self.atlas_upload_mapped != null) self.atlas_upload.Unmap(0, null);
        _ = self.atlas_upload.Release();
        _ = self.atlas_texture.Release();
        if (self.dirty_index_mapped != null) self.dirty_index_upload.Unmap(0, null);
        _ = self.dirty_index_upload.Release();
        if (self.dirty_value_mapped != null) self.dirty_value_upload.Unmap(0, null);
        _ = self.dirty_value_upload.Release();
        if (self.cell_mapped != null) self.cell_buf_upload.Unmap(0, null);
        _ = self.cell_buf_upload.Release();
        _ = self.cell_buf_gpu.Release();
        for (self.readback_ptrs, 0..) |ptr, i| {
            if (ptr != null) self.readback_bufs[i].Unmap(0, null);
            _ = self.readback_bufs[i].Release();
        }
        _ = self.rt_texture.Release();
        const compute_pso: *dx.IUnknown = @ptrCast(@alignCast(self.compute_pso));
        _ = compute_pso.Release();
        const compute_rs: *dx.IUnknown = @ptrCast(@alignCast(self.compute_root_sig));
        _ = compute_rs.Release();
        const pso: *dx.IUnknown = @ptrCast(@alignCast(self.pso));
        _ = pso.Release();
        const rs: *dx.IUnknown = @ptrCast(@alignCast(self.root_sig));
        _ = rs.Release();
        _ = self.fence.Release();
        _ = dx.CloseHandle(self.fence_event);
        _ = self.srv_heap.Release();
        _ = self.rtv_heap.Release();
        for (self.frames) |frame| {
            _ = frame.cmd_list.Release();
            _ = frame.cmd_alloc.Release();
        }
        _ = self.cmd_queue.Release();
        _ = self.device.Release();
    }

    pub fn resize(self: *GpuRenderer, new_width: u32, new_height: u32) bool {
        if (new_width == self.width and new_height == self.height) return true;
        self.waitForGpu();

        // Release old render target & readback (double-buffered)
        for (self.readback_ptrs, 0..) |ptr, i| {
            if (ptr != null) {
                self.readback_bufs[i].Unmap(0, null);
                self.readback_ptrs[i] = null;
            }
            _ = self.readback_bufs[i].Release();
        }
        _ = self.rt_texture.Release();

        self.width = new_width;
        self.height = new_height;
        return self.createRenderTarget();
    }
};

fn unpackRgba(c: u32) [4]f32 {
    return .{
        @as(f32, @floatFromInt((c >> 16) & 0xFF)) / 255.0,
        @as(f32, @floatFromInt((c >> 8) & 0xFF)) / 255.0,
        @as(f32, @floatFromInt(c & 0xFF)) / 255.0,
        @as(f32, @floatFromInt((c >> 24) & 0xFF)) / 255.0,
    };
}

// ================================================================
// Init error diagnostics
// ================================================================

/// Error stages for diagnosing GPU renderer initialization failures.
/// Returned via ghostty_gpu_renderer_last_init_error().
pub const GpuInitStage = enum(u32) {
    none = 0,
    alloc_struct = 1,
    alloc_glyph_map = 2,
    alloc_atlas_bitmap = 3,
    gdi_create_dc = 4,
    gdi_create_font = 5,
    dwrite_create_factory = 6,
    dwrite_get_gdi_interop = 7,
    dwrite_create_font_face = 8,
    dwrite_font_metrics = 9,
    dx12_create_device_hw = 10,
    dx12_create_device_warp = 11,
    dx12_command_queue = 12,
    dx12_command_allocator = 13,
    dx12_descriptor_heap_rtv = 14,
    dx12_descriptor_heap_srv = 15,
    dx12_fence = 16,
    dx12_fence_event = 17,
    dx12_pipeline = 18,
    dx12_render_target = 19,
    dx12_cell_buffers = 20,
    dx12_atlas_texture = 21,
    dx12_command_list = 22,
    dx12_shader_compile_vs = 23,
    dx12_shader_compile_ps = 24,
    dx12_root_sig_serialize = 25,
    dx12_root_sig_create = 26,
    dx12_pso_create = 27,
};

var last_init_stage: u32 = 0;
var last_init_hr: i32 = 0;

fn setInitError(stage: GpuInitStage, hr: dx.HRESULT) void {
    last_init_stage = @intFromEnum(stage);
    last_init_hr = hr;
}

fn clearInitError() void {
    last_init_stage = 0;
    last_init_hr = 0;
}

// ================================================================
// C API exports
// ================================================================

var gpu_gpa: ?std.heap.GeneralPurposeAllocator(.{}) = null;

export fn ghostty_gpu_renderer_new(width: u32, height: u32, font_size: f32) ?*GpuRenderer {
    clearInitError();
    if (gpu_gpa == null) gpu_gpa = std.heap.GeneralPurposeAllocator(.{}){};
    return GpuRenderer.init(gpu_gpa.?.allocator(), width, height, font_size);
}

export fn ghostty_gpu_renderer_last_init_error(stage_out: *u32, hr_out: *i32) callconv(.c) void {
    stage_out.* = last_init_stage;
    hr_out.* = last_init_hr;
}

export fn ghostty_gpu_renderer_free(r: ?*GpuRenderer) void {
    if (r) |v| v.deinit();
}

export fn ghostty_gpu_renderer_resize(r: ?*GpuRenderer, width: u32, height: u32) u8 {
    if (r) |v| return @intFromBool(v.resize(width, height));
    return 0;
}

export fn ghostty_gpu_renderer_render(
    r: ?*GpuRenderer,
    cells: ?[*]const GpuCellData,
    cell_count: u32,
    term_cols: u32,
    cell_width: f32,
    cell_height: f32,
) ?[*]const u8 {
    if (r) |v| {
        if (cells) |c| return v.renderFrame(c, cell_count, term_cols, cell_width, cell_height);
    }
    return null;
}

export fn ghostty_gpu_renderer_render_delta(
    r: ?*GpuRenderer,
    cells: ?[*]const GpuCellData,
    cell_count: u32,
    dirty_ranges: ?[*]const GpuDirtyRange,
    dirty_range_count: u32,
    term_cols: u32,
    cell_width: f32,
    cell_height: f32,
) ?[*]const u8 {
    if (r) |v| {
        if (cells) |c| {
            if (dirty_ranges) |ranges| {
                return v.renderFrameDelta(c, cell_count, ranges, dirty_range_count, term_cols, cell_width, cell_height);
            }
            return v.renderFrame(c, cell_count, term_cols, cell_width, cell_height);
        }
    }
    return null;
}

export fn ghostty_gpu_renderer_render_to_texture(
    r: ?*GpuRenderer,
    cells: ?[*]const GpuCellData,
    cell_count: u32,
    term_cols: u32,
    cell_width: f32,
    cell_height: f32,
) u8 {
    if (r) |v| {
        if (cells) |c| return @intFromBool(v.renderToTexture(c, cell_count, term_cols, cell_width, cell_height));
    }
    return 0;
}

export fn ghostty_gpu_renderer_render_to_surface(
    r: ?*GpuRenderer,
    target_resource: ?*anyopaque,
    cells: ?[*]const GpuCellData,
    cell_count: u32,
    term_cols: u32,
    cell_width: f32,
    cell_height: f32,
) u8 {
    if (r) |v| {
        if (target_resource) |target| {
            if (cells) |c| {
                return @intFromBool(v.renderToSurface(target, c, cell_count, term_cols, cell_width, cell_height));
            }
        }
    }
    return 0;
}

export fn ghostty_gpu_renderer_render_to_surface_delta(
    r: ?*GpuRenderer,
    target_resource: ?*anyopaque,
    cells: ?[*]const GpuCellData,
    cell_count: u32,
    damage_rects: ?[*]const GpuDamageRect,
    damage_rect_count: u32,
    term_cols: u32,
    cell_width: f32,
    cell_height: f32,
) u8 {
    if (r) |v| {
        if (target_resource) |target| {
            if (cells) |c| {
                if (damage_rects) |rects| {
                    return @intFromBool(v.renderToSurfaceDelta(
                        target,
                        c,
                        cell_count,
                        rects,
                        damage_rect_count,
                        term_cols,
                        cell_width,
                        cell_height,
                    ));
                }
                return @intFromBool(v.renderToSurface(target, c, cell_count, term_cols, cell_width, cell_height));
            }
        }
    }
    return 0;
}

export fn ghostty_gpu_renderer_render_to_surface_delta_cells(
    r: ?*GpuRenderer,
    target_resource: ?*anyopaque,
    cells: ?[*]const GpuCellData,
    cell_count: u32,
    dirty_cells: ?[*]const GpuDirtyCell,
    dirty_cell_count: u32,
    damage_rects: ?[*]const GpuDamageRect,
    damage_rect_count: u32,
    term_cols: u32,
    cell_width: f32,
    cell_height: f32,
) u8 {
    if (r) |v| {
        if (target_resource) |target| {
            if (cells) |c| {
                if (damage_rects) |rects| {
                    if (dirty_cells) |indices| {
                        return @intFromBool(v.renderToSurfaceDeltaCells(
                            target,
                            c,
                            cell_count,
                            indices,
                            dirty_cell_count,
                            rects,
                            damage_rect_count,
                            term_cols,
                            cell_width,
                            cell_height,
                        ));
                    }
                    return @intFromBool(v.renderToSurfaceDelta(
                        target,
                        c,
                        cell_count,
                        rects,
                        damage_rect_count,
                        term_cols,
                        cell_width,
                        cell_height,
                    ));
                }
                return @intFromBool(v.renderToSurface(target, c, cell_count, term_cols, cell_width, cell_height));
            }
        }
    }
    return 0;
}

export fn ghostty_gpu_renderer_pixel_stride(r: ?*const GpuRenderer) u32 {
    if (r) |v| return v.pixelStride();
    return 0;
}

export fn ghostty_gpu_renderer_width(r: ?*const GpuRenderer) u32 {
    if (r) |v| return v.width;
    return 0;
}

export fn ghostty_gpu_renderer_height(r: ?*const GpuRenderer) u32 {
    if (r) |v| return v.height;
    return 0;
}

export fn ghostty_gpu_renderer_device_ptr(r: ?*const GpuRenderer) ?*anyopaque {
    if (r) |v| return @ptrCast(v.device);
    return null;
}

export fn ghostty_gpu_renderer_command_queue_ptr(r: ?*const GpuRenderer) ?*anyopaque {
    if (r) |v| return @ptrCast(v.cmd_queue);
    return null;
}

export fn ghostty_gpu_renderer_render_target_ptr(r: ?*const GpuRenderer) ?*anyopaque {
    if (r) |v| return @ptrCast(v.rt_texture);
    return null;
}
