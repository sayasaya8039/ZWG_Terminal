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

// ================================================================
// Internal types
// ================================================================

const ATLAS_SIZE: u32 = 2048;
const GLYPH_MAP_CAP: usize = 4096;
const GLYPH_PAD: u32 = 1;
const MAX_CELLS: u32 = 400 * 120; // 400 cols × 120 rows max

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
    cmd_alloc: *dx.ID3D12CommandAllocator,
    cmd_list: *dx.ID3D12GraphicsCommandList,
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

    // Render target (offscreen RGBA8)
    rt_texture: *dx.ID3D12Resource,
    readback_buf: *dx.ID3D12Resource,
    width: u32,
    height: u32,
    readback_ptr: ?[*]u8, // mapped pointer (persistent)

    // Cell upload buffer (structured buffer for instanced draw)
    cell_buf_upload: *dx.ID3D12Resource,
    cell_buf_gpu: *dx.ID3D12Resource,
    cell_mapped: ?[*]u8,

    // Glyph atlas
    atlas_bitmap: []u8, // CPU R8 bitmap
    atlas_texture: *dx.ID3D12Resource,
    atlas_upload: *dx.ID3D12Resource,
    glyph_map: []GlyphSlot,
    glyph_count: u32,
    atlas_cols: u32,
    atlas_dirty: bool,
    atlas_dirty_start: u32,
    atlas_dirty_end: u32,

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
        self.atlas_dirty = false;
        self.glyph_count = 0;
        self.atlas_cols = 0;
        self.atlas_dirty_start = 0;
        self.atlas_dirty_end = 0;
        self.readback_ptr = null;
        self.cell_mapped = null;
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
        if (self.atlas_cols == 0 or (ATLAS_SIZE / slot_pitch_h) == 0) {
            log.err(
                "initGdi: atlas packing is invalid for glyph pitch {}x{}",
                .{ slot_pitch_w, slot_pitch_h },
            );
            setInitError(.dwrite_font_metrics, -1);
            self.deinitGdi();
            return false;
        }

        log.info(
            "DirectWrite glyph rasterizer ready glyph_cell={}x{} baseline={d:.2} atlas_cols={}",
            .{ self.glyph_cell_w, self.glyph_cell_h, self.glyph_baseline, self.atlas_cols },
        );
        return true;
    }

    fn deinitGdi(self: *GpuRenderer) void {
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
        const atlas_rows = ATLAS_SIZE / slot_pitch_h;
        const total_slots = self.atlas_cols * atlas_rows;
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
        if (!self.atlas_dirty) {
            self.atlas_dirty = true;
            self.atlas_dirty_start = atlas_index;
            self.atlas_dirty_end = atlas_index + 1;
            return;
        }
        self.atlas_dirty_start = @min(self.atlas_dirty_start, atlas_index);
        self.atlas_dirty_end = @max(self.atlas_dirty_end, atlas_index + 1);
    }

    fn hasAtlasDirtySlot(self: *const GpuRenderer, atlas_index: u32) bool {
        return self.atlas_dirty and atlas_index >= self.atlas_dirty_start and atlas_index < self.atlas_dirty_end;
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

        // 3. Command allocator
        log.info("initDx12: step 3 create command allocator", .{});
        var ca_raw: ?*anyopaque = null;
        const ca_hr = self.device.CreateCommandAllocator(.DIRECT, &dx.IID_ID3D12CommandAllocator, &ca_raw);
        if (!dx.SUCCEEDED(ca_hr)) {
            log.err("initDx12: CreateCommandAllocator failed hr=0x{x}", .{hr_hex(ca_hr)});
            setInitError(.dx12_command_allocator, ca_hr);
            return false;
        }
        self.cmd_alloc = @ptrCast(@alignCast(ca_raw.?));

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

        const srv_desc = dx.D3D12_DESCRIPTOR_HEAP_DESC{ .Type = .CBV_SRV_UAV, .NumDescriptors = 2, .Flags = .SHADER_VISIBLE };
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

        // 10. Create command list (initially closed)
        log.info("initDx12: step 10 create command list", .{});
        var cl_raw: ?*anyopaque = null;
        const cl_hr = self.device.CreateCommandList(
            0,
            .DIRECT,
            @ptrCast(self.cmd_alloc),
            null,
            &dx.IID_ID3D12GraphicsCommandList,
            &cl_raw,
        );
        if (!dx.SUCCEEDED(cl_hr)) {
            const removed_reason = self.device.GetDeviceRemovedReason();
            log.err("initDx12: CreateCommandList failed hr=0x{x} DeviceRemovedReason=0x{x}", .{ hr_hex(cl_hr), hr_hex(removed_reason) });
            setInitError(.dx12_command_list, cl_hr);
            return false;
        }
        self.cmd_list = @ptrCast(@alignCast(cl_raw.?));
        _ = self.cmd_list.Close();

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

        // Readback buffer
        const row_pitch = self.pixelStride();
        const rb_size = @as(u64, row_pitch) * @as(u64, self.height);
        const rb_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .BUFFER,
            .Width = rb_size,
            .Layout = .ROW_MAJOR, // Required for BUFFER resources
        };
        const heap_readback = dx.D3D12_HEAP_PROPERTIES{ .Type = .READBACK };

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
            log.err("createRenderTarget: readback buffer allocation failed hr=0x{x}", .{hr_hex(rb_hr)});
            return false;
        }
        self.readback_buf = @ptrCast(@alignCast(rb_raw.?));

        // Map readback buffer persistently
        var mapped: ?*anyopaque = null;
        const map_hr = self.readback_buf.Map(0, null, &mapped);
        if (dx.SUCCEEDED(map_hr)) {
            self.readback_ptr = @ptrCast(mapped);
        } else {
            log.err("createRenderTarget: readback buffer map failed hr=0x{x}", .{hr_hex(map_hr)});
            return false;
        }

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

        // GPU buffer (default heap, SRV)
        const heap_default = dx.D3D12_HEAP_PROPERTIES{ .Type = .DEFAULT };
        var gpu_raw: ?*anyopaque = null;
        const gpu_hr = self.device.CreateCommittedResource(
            &heap_default,
            .NONE,
            &buf_desc,
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
        self.device.CreateShaderResourceView(@ptrCast(self.cell_buf_upload), &srv_desc, handle);

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

    fn uploadAtlas(self: *GpuRenderer) void {
        if (!self.atlas_dirty) return;

        var mapped: ?*anyopaque = null;
        const map_hr = self.atlas_upload.Map(0, null, &mapped);
        if (!dx.SUCCEEDED(map_hr)) {
            log.err("uploadAtlas: atlas upload buffer map failed hr=0x{x}", .{hr_hex(map_hr)});
            return;
        }
        const dst: [*]u8 = @ptrCast(mapped orelse return);
        _ = self.cmd_alloc.Reset();
        _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);
        const slot_pitch_w = self.glyph_cell_w + GLYPH_PAD;
        const slot_pitch_h = self.glyph_cell_h + GLYPH_PAD;
        const upload_capacity = @as(u64, ((ATLAS_SIZE + 255) / 256) * 256) * @as(u64, ATLAS_SIZE);
        const start_row = self.atlas_dirty_start / self.atlas_cols;
        const end_row = (self.atlas_dirty_end - 1) / self.atlas_cols;
        const dst_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.atlas_texture),
            .Type = 0,
            .u = .{ .SubresourceIndex = 0 },
        };

        var upload_offset: u64 = 0;
        var row_index = start_row;
        while (row_index <= end_row) : (row_index += 1) {
            const row_slot_start = if (row_index == start_row) self.atlas_dirty_start % self.atlas_cols else 0;
            const row_slot_end = if (row_index == end_row) self.atlas_dirty_end % self.atlas_cols else 0;
            const row_slot_end_excl = if (row_index == end_row and row_slot_end != 0) row_slot_end else if (row_index == end_row) self.atlas_cols else self.atlas_cols;
            const region_x = row_slot_start * slot_pitch_w;
            const region_y = row_index * slot_pitch_h;
            const region_w = (row_slot_end_excl - row_slot_start) * slot_pitch_w;
            const region_h = slot_pitch_h;
            if (region_w == 0 or region_h == 0) continue;

            const row_pitch = ((region_w + 255) / 256) * 256;
            const region_bytes = @as(u64, row_pitch) * @as(u64, region_h);
            const aligned_offset = ((upload_offset + 511) / 512) * 512;
            if (aligned_offset + region_bytes > upload_capacity) {
                _ = self.cmd_list.Close();
                const lists = [_]*anyopaque{@ptrCast(self.cmd_list)};
                self.cmd_queue.ExecuteCommandLists(1, &lists);
                self.waitForGpu();
                _ = self.cmd_alloc.Reset();
                _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);
                upload_offset = 0;
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
            self.cmd_list.CopyTextureRegion(&dst_loc, region_x, region_y, 0, &src_loc, null);
            upload_offset += region_bytes;
        }
        self.atlas_upload.Unmap(0, null);

        const barrier = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.atlas_texture), .COPY_DEST, .PIXEL_SHADER_RESOURCE),
        };
        self.cmd_list.ResourceBarrier(1, &barrier);

        _ = self.cmd_list.Close();
        const lists = [_]*anyopaque{@ptrCast(self.cmd_list)};
        self.cmd_queue.ExecuteCommandLists(1, &lists);
        self.waitForGpu();

        self.atlas_dirty = false;
        self.atlas_dirty_start = 0;
        self.atlas_dirty_end = 0;
    }

    // ---- Frame rendering ----

    fn uploadAtlasIfDirty(self: *GpuRenderer) void {
        if (!self.atlas_dirty) return;

        _ = self.cmd_alloc.Reset();
        _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);
        const barrier = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.atlas_texture), .PIXEL_SHADER_RESOURCE, .COPY_DEST),
        };
        self.cmd_list.ResourceBarrier(1, &barrier);
        _ = self.cmd_list.Close();
        const lists = [_]*anyopaque{@ptrCast(self.cmd_list)};
        self.cmd_queue.ExecuteCommandLists(1, &lists);
        self.waitForGpu();
        self.uploadAtlas();
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

            var i: u32 = 0;
            while (i < instance_count) : (i += 1) {
                const cell_index = start_instance + i;
                const c = cells[cell_index];
                const glyph_idx = if (c.codepoint == 0)
                    0
                else
                    self.rasterizeGlyph(c.codepoint) orelse 0;

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

    /// Render terminal cells to offscreen buffer. Returns RGBA pixel pointer.
    pub fn renderFrame(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) ?[*]const u8 {
        const count = @min(cell_count, MAX_CELLS);
        if (count == 0) return self.readback_ptr;
        if (!self.renderToTexture(cells, cell_count, term_cols, cell_width, cell_height)) {
            return null;
        }

        // Copy RT → readback using a placed-footprint copy; texture->buffer CopyResource is invalid.
        const src_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.rt_texture),
            .Type = 0,
            .u = .{ .SubresourceIndex = 0 },
        };
        const dst_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.readback_buf),
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
        self.cmd_list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, null);

        // Close & execute
        _ = self.cmd_list.Close();
        const lists = [_]*anyopaque{@ptrCast(self.cmd_list)};
        self.cmd_queue.ExecuteCommandLists(1, &lists);
        self.waitForGpu();

        return self.readback_ptr;
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

        self.uploadAtlasIfDirty();
        self.uploadCellRange(cells, 0, count, cell_width, cell_height);
        self.uploadAtlasIfDirty();

        _ = self.cmd_alloc.Reset();
        _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);

        const b_rt = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(target_resource, before_clear, draw_state),
        };
        self.cmd_list.ResourceBarrier(1, &b_rt);

        const rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateRenderTargetView(target_resource, null, rtv);
        const clear_color = [4]f32{ 0.0, 0.0, 0.0, 1.0 };
        self.cmd_list.ClearRenderTargetView(rtv, &clear_color, 0, null);
        self.cmd_list.OMSetRenderTargets(1, @ptrCast(&rtv), dx.FALSE, null);

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
        self.cmd_list.RSSetViewports(1, &viewport);
        self.cmd_list.RSSetScissorRects(1, &scissor);

        self.cmd_list.SetGraphicsRootSignature(self.root_sig);
        self.cmd_list.SetPipelineState(self.pso);
        self.cmd_list.IASetPrimitiveTopology(.TRIANGLELIST);

        const heaps = [_]*anyopaque{@ptrCast(self.srv_heap)};
        self.cmd_list.SetDescriptorHeaps(1, &heaps);
        self.cmd_list.SetGraphicsRootDescriptorTable(0, self.srv_heap.GetGPUDescriptorHandleForHeapStart());

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
        self.cmd_list.SetGraphicsRoot32BitConstants(1, 12, &constants, 0);
        self.cmd_list.DrawInstanced(6, count, 0, 0);

        const b_copy = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(target_resource, before_finish, after_finish),
        };
        self.cmd_list.ResourceBarrier(1, &b_copy);

        _ = self.cmd_list.Close();
        const lists = [_]*anyopaque{@ptrCast(self.cmd_list)};
        self.cmd_queue.ExecuteCommandLists(1, &lists);
        self.waitForGpu();
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
        if (count == 0) return self.readback_ptr;
        if (dirty_range_count == 0) return self.renderFrame(cells, cell_count, term_cols, cell_width, cell_height);

        self.uploadAtlasIfDirty();

        var dirty_rects = std.ArrayList(dx.RECT).empty;
        defer dirty_rects.deinit(self.alloc);

        var i: u32 = 0;
        while (i < dirty_range_count) : (i += 1) {
            const range = dirty_ranges[i];
            if (range.row_count == 0 or range.row_start >= MAX_CELLS) continue;
            if (range.start_instance >= count) continue;

            const instance_end = @min(count, range.start_instance + range.instance_count);
            const instance_count_clamped = instance_end - range.start_instance;
            self.uploadCellRange(cells, range.start_instance, instance_count_clamped, cell_width, cell_height);

            const top = @as(i32, @intFromFloat(@floor(@as(f32, @floatFromInt(range.row_start)) * cell_height)));
            const row_end = range.row_start + range.row_count;
            const bottom_f = @min(
                @as(f32, @floatFromInt(self.height)),
                @as(f32, @floatFromInt(row_end)) * cell_height,
            );
            const bottom = @as(i32, @intFromFloat(@ceil(bottom_f)));
            if (bottom <= top) continue;
            dirty_rects.append(self.alloc, .{
                .left = 0,
                .top = top,
                .right = @intCast(self.width),
                .bottom = bottom,
            }) catch return null;
        }

        self.uploadAtlasIfDirty();
        if (dirty_rects.items.len == 0) return self.readback_ptr;

        _ = self.cmd_alloc.Reset();
        _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);

        const b_rt = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.rt_texture), .COPY_SOURCE, .RENDER_TARGET),
        };
        self.cmd_list.ResourceBarrier(1, &b_rt);

        const rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        const clear_color = [4]f32{ 0.0, 0.0, 0.0, 1.0 };
        self.cmd_list.ClearRenderTargetView(
            rtv,
            &clear_color,
            @intCast(dirty_rects.items.len),
            dirty_rects.items.ptr,
        );
        self.cmd_list.OMSetRenderTargets(1, @ptrCast(&rtv), dx.FALSE, null);

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
        self.cmd_list.RSSetViewports(1, &viewport);
        self.cmd_list.RSSetScissorRects(1, &scissor);
        self.cmd_list.SetGraphicsRootSignature(self.root_sig);
        self.cmd_list.SetPipelineState(self.pso);
        self.cmd_list.IASetPrimitiveTopology(.TRIANGLELIST);

        const heaps = [_]*anyopaque{@ptrCast(self.srv_heap)};
        self.cmd_list.SetDescriptorHeaps(1, &heaps);
        self.cmd_list.SetGraphicsRootDescriptorTable(0, self.srv_heap.GetGPUDescriptorHandleForHeapStart());

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
        self.cmd_list.SetGraphicsRoot32BitConstants(1, 12, &constants, 0);

        i = 0;
        while (i < dirty_range_count) : (i += 1) {
            const range = dirty_ranges[i];
            if (range.start_instance >= count) continue;
            const instance_end = @min(count, range.start_instance + range.instance_count);
            const instance_count_clamped = instance_end - range.start_instance;
            if (instance_count_clamped == 0) continue;
            self.cmd_list.DrawInstanced(6, instance_count_clamped, 0, range.start_instance);
        }

        const b_copy = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.rt_texture), .RENDER_TARGET, .COPY_SOURCE),
        };
        self.cmd_list.ResourceBarrier(1, &b_copy);
        const row_pitch = self.pixelStride();
        i = 0;
        while (i < dirty_range_count) : (i += 1) {
            const range = dirty_ranges[i];
            if (range.row_count == 0) continue;
            const row_end = @min(self.height, range.row_start + range.row_count);
            if (range.row_start >= row_end) continue;
            const row_count_clamped = row_end - range.row_start;

            const src_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
                .pResource = @ptrCast(self.rt_texture),
                .Type = 0,
                .u = .{ .SubresourceIndex = 0 },
            };
            const dst_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
                .pResource = @ptrCast(self.readback_buf),
                .Type = 1,
                .u = .{
                    .PlacedFootprint = .{
                        .Offset = @as(u64, range.row_start) * @as(u64, row_pitch),
                        .Format = .R8G8B8A8_UNORM,
                        .Width = self.width,
                        .Height = row_count_clamped,
                        .RowPitch = row_pitch,
                    },
                },
            };
            const src_box = dx.D3D12_BOX{
                .left = 0,
                .top = range.row_start,
                .right = self.width,
                .bottom = row_end,
            };
            self.cmd_list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, &src_box);
        }

        _ = self.cmd_list.Close();
        const lists = [_]*anyopaque{@ptrCast(self.cmd_list)};
        self.cmd_queue.ExecuteCommandLists(1, &lists);
        self.waitForGpu();

        return self.readback_ptr;
    }

    // ---- Helpers ----

    fn waitForGpu(self: *GpuRenderer) void {
        self.fence_value += 1;
        _ = self.cmd_queue.Signal(@ptrCast(self.fence), self.fence_value);
        if (self.fence.GetCompletedValue() < self.fence_value) {
            _ = self.fence.SetEventOnCompletion(self.fence_value, self.fence_event);
            _ = dx.WaitForSingleObject(self.fence_event, dx.INFINITE);
        }
    }

    fn deinitDx12(self: *GpuRenderer) void {
        // Release in reverse order
        _ = self.atlas_upload.Release();
        _ = self.atlas_texture.Release();
        if (self.cell_mapped != null) self.cell_buf_upload.Unmap(0, null);
        _ = self.cell_buf_upload.Release();
        _ = self.cell_buf_gpu.Release();
        if (self.readback_ptr != null) self.readback_buf.Unmap(0, null);
        _ = self.readback_buf.Release();
        _ = self.rt_texture.Release();
        const pso: *dx.IUnknown = @ptrCast(@alignCast(self.pso));
        _ = pso.Release();
        const rs: *dx.IUnknown = @ptrCast(@alignCast(self.root_sig));
        _ = rs.Release();
        _ = self.cmd_list.Release();
        _ = self.fence.Release();
        _ = dx.CloseHandle(self.fence_event);
        _ = self.srv_heap.Release();
        _ = self.rtv_heap.Release();
        _ = self.cmd_alloc.Release();
        _ = self.cmd_queue.Release();
        _ = self.device.Release();
    }

    pub fn resize(self: *GpuRenderer, new_width: u32, new_height: u32) bool {
        if (new_width == self.width and new_height == self.height) return true;
        self.waitForGpu();

        // Release old render target & readback
        if (self.readback_ptr != null) {
            self.readback_buf.Unmap(0, null);
            self.readback_ptr = null;
        }
        _ = self.readback_buf.Release();
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
