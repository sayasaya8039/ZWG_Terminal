//! DX12 GPU terminal renderer — offscreen cell rendering with glyph atlas.
//!
//! Architecture:
//!   1. GDI rasterises glyphs → R8 bitmap atlas
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

// ================================================================
// Internal types
// ================================================================

const ATLAS_SIZE: u32 = 2048;
const GLYPH_MAP_CAP: usize = 4096;
const GLYPH_PAD: u32 = 1;
const MAX_CELLS: u32 = 400 * 120; // 400 cols × 120 rows max

/// Per-cell data uploaded to GPU (matches HLSL CellData)
const GpuCellGpu = extern struct {
    pos: [2]f32, // pixel position
    uv_origin: [2]f32,
    uv_size: [2]f32,
    fg: [4]f32,
    bg: [4]f32,
};

const AtlasRect = struct {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
};

const GlyphSlot = struct {
    key: u32, // codepoint
    rect: AtlasRect,
    occupied: bool,
};

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
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    atlas_dirty: bool,

    // GDI for glyph rasterization
    gdi_dc: dx.HDC,
    gdi_bmp: dx.HBITMAP,
    gdi_bits: ?*anyopaque,
    gdi_font: dx.HFONT,
    glyph_cell_w: u32,
    glyph_cell_h: u32,

    // ---- Initialization ----

    pub fn init(alloc: Allocator, width: u32, height: u32, font_size: f32) ?*GpuRenderer {
        const self = alloc.create(GpuRenderer) catch return null;
        self.* = undefined;
        self.alloc = alloc;
        self.width = width;
        self.height = height;
        self.fence_value = 0;
        self.atlas_dirty = false;
        self.glyph_count = 0;
        self.shelf_x = 0;
        self.shelf_y = 0;
        self.shelf_h = 0;
        self.readback_ptr = null;
        self.cell_mapped = null;
        self.gdi_bits = null;

        // Init glyph map
        self.glyph_map = alloc.alloc(GlyphSlot, GLYPH_MAP_CAP) catch {
            alloc.destroy(self);
            return null;
        };
        for (self.glyph_map) |*s| s.* = .{ .key = 0, .rect = .{ .x = 0, .y = 0, .w = 0, .h = 0 }, .occupied = false };

        // Atlas bitmap (CPU)
        self.atlas_bitmap = alloc.alloc(u8, ATLAS_SIZE * ATLAS_SIZE) catch {
            alloc.free(self.glyph_map);
            alloc.destroy(self);
            return null;
        };
        @memset(self.atlas_bitmap, 0);

        // Init GDI for glyph rasterization
        if (!self.initGdi(font_size)) {
            alloc.free(self.atlas_bitmap);
            alloc.free(self.glyph_map);
            alloc.destroy(self);
            return null;
        }

        // Init DX12
        if (!self.initDx12()) {
            self.deinitGdi();
            alloc.free(self.atlas_bitmap);
            alloc.free(self.glyph_map);
            alloc.destroy(self);
            return null;
        }

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

    // ---- GDI glyph rasterization ----

    fn initGdi(self: *GpuRenderer, font_size: f32) bool {
        self.gdi_dc = dx.CreateCompatibleDC(null);
        if (self.gdi_dc == null) return false;

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

        // Create DIB section for rasterizing individual glyphs
        var bmi: dx.BITMAPINFO = .{};
        bmi.bmiHeader.biWidth = @intCast(self.glyph_cell_w);
        bmi.bmiHeader.biHeight = -@as(i32, @intCast(self.glyph_cell_h)); // top-down
        self.gdi_bmp = dx.CreateDIBSection(self.gdi_dc, &bmi, 0, &self.gdi_bits, null, 0);
        if (self.gdi_bmp == null) {
            _ = dx.DeleteObject(self.gdi_font);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }
        _ = dx.SelectObject(self.gdi_dc, self.gdi_bmp);
        _ = dx.SetBkMode(self.gdi_dc, 1); // TRANSPARENT
        _ = dx.SetTextColor(self.gdi_dc, 0x00FFFFFF); // white text
        _ = dx.SetBkColor(self.gdi_dc, 0x00000000); // black bg

        return true;
    }

    fn deinitGdi(self: *GpuRenderer) void {
        if (self.gdi_bmp != null) _ = dx.DeleteObject(self.gdi_bmp);
        if (self.gdi_font != null) _ = dx.DeleteObject(self.gdi_font);
        if (self.gdi_dc != null) _ = dx.DeleteDC(self.gdi_dc);
    }

    /// Rasterise a single codepoint and upload to atlas. Returns atlas rect.
    fn rasterizeGlyph(self: *GpuRenderer, codepoint: u32) ?AtlasRect {
        // Check cache first
        if (self.lookupGlyph(codepoint)) |r| return r;

        const w = self.glyph_cell_w;
        const h = self.glyph_cell_h;
        const gw = w + GLYPH_PAD;
        const gh = h + GLYPH_PAD;

        // Shelf packing
        if (self.shelf_x + gw > ATLAS_SIZE) {
            self.shelf_y += self.shelf_h;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        if (self.shelf_y + gh > ATLAS_SIZE) return null; // atlas full
        if (gh > self.shelf_h) self.shelf_h = gh;

        // Clear DIB and draw glyph
        if (self.gdi_bits) |bits| {
            const byte_size = w * h * 4;
            const slice: [*]u8 = @ptrCast(bits);
            @memset(slice[0..byte_size], 0);
        }
        const ch: [1]u16 = .{@intCast(codepoint & 0xFFFF)};
        _ = dx.TextOutW(self.gdi_dc, 0, 0, &ch, 1);

        // Extract R8 (alpha from blue channel of BGRA DIB)
        if (self.gdi_bits) |bits| {
            const src: [*]const u8 = @ptrCast(bits);
            const dst_x = self.shelf_x;
            const dst_y = self.shelf_y;
            var row: u32 = 0;
            while (row < h) : (row += 1) {
                var col: u32 = 0;
                while (col < w) : (col += 1) {
                    // BGRA layout: blue at offset 0 per pixel
                    const src_idx = (row * w + col) * 4;
                    // Use max of R,G,B as alpha (GDI ClearType gives per-channel)
                    const r = src[src_idx + 2];
                    const g = src[src_idx + 1];
                    const b = src[src_idx + 0];
                    const alpha = @max(r, @max(g, b));
                    const dst_idx = (dst_y + row) * ATLAS_SIZE + dst_x + col;
                    if (dst_idx < self.atlas_bitmap.len) {
                        self.atlas_bitmap[dst_idx] = alpha;
                    }
                }
            }
        }

        const rect = AtlasRect{
            .x = @intCast(self.shelf_x),
            .y = @intCast(self.shelf_y),
            .w = @intCast(w),
            .h = @intCast(h),
        };
        self.insertGlyph(codepoint, rect);
        self.shelf_x += gw;
        self.atlas_dirty = true;

        return rect;
    }

    fn lookupGlyph(self: *const GpuRenderer, cp: u32) ?AtlasRect {
        var idx = @as(usize, cp % GLYPH_MAP_CAP);
        var probes: usize = 0;
        while (probes < GLYPH_MAP_CAP) : (probes += 1) {
            const s = &self.glyph_map[idx];
            if (!s.occupied) return null;
            if (s.key == cp) return s.rect;
            idx = (idx + 1) % GLYPH_MAP_CAP;
        }
        return null;
    }

    fn insertGlyph(self: *GpuRenderer, cp: u32, rect: AtlasRect) void {
        var idx = @as(usize, cp % GLYPH_MAP_CAP);
        var probes: usize = 0;
        while (probes < GLYPH_MAP_CAP) : (probes += 1) {
            const s = &self.glyph_map[idx];
            if (!s.occupied) {
                self.glyph_map[idx] = .{ .key = cp, .rect = rect, .occupied = true };
                self.glyph_count += 1;
                return;
            }
            if (s.key == cp) {
                self.glyph_map[idx].rect = rect;
                return;
            }
            idx = (idx + 1) % GLYPH_MAP_CAP;
        }
    }

    // ---- DX12 Initialization ----

    fn initDx12(self: *GpuRenderer) bool {
        // 1. Create device (default adapter, feature level 11.0)
        var device_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(dx.D3D12CreateDevice(null, .@"11_0", &dx.IID_ID3D12Device, &device_raw))) return false;
        self.device = @ptrCast(@alignCast(device_raw.?));

        // 2. Command queue
        const cq_desc = dx.D3D12_COMMAND_QUEUE_DESC{};
        var cq_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateCommandQueue(&cq_desc, &dx.IID_ID3D12CommandQueue, &cq_raw))) return false;
        self.cmd_queue = @ptrCast(@alignCast(cq_raw.?));

        // 3. Command allocator
        var ca_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateCommandAllocator(.DIRECT, &dx.IID_ID3D12CommandAllocator, &ca_raw))) return false;
        self.cmd_alloc = @ptrCast(@alignCast(ca_raw.?));

        // 4. Descriptor heaps
        const rtv_desc = dx.D3D12_DESCRIPTOR_HEAP_DESC{ .Type = .RTV, .NumDescriptors = 1 };
        var rtv_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateDescriptorHeap(&rtv_desc, &dx.IID_ID3D12DescriptorHeap, &rtv_raw))) return false;
        self.rtv_heap = @ptrCast(@alignCast(rtv_raw.?));
        self.rtv_inc = self.device.GetDescriptorHandleIncrementSize(.RTV);

        const srv_desc = dx.D3D12_DESCRIPTOR_HEAP_DESC{ .Type = .CBV_SRV_UAV, .NumDescriptors = 2, .Flags = .SHADER_VISIBLE };
        var srv_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateDescriptorHeap(&srv_desc, &dx.IID_ID3D12DescriptorHeap, &srv_raw))) return false;
        self.srv_heap = @ptrCast(@alignCast(srv_raw.?));
        self.srv_inc = self.device.GetDescriptorHandleIncrementSize(.CBV_SRV_UAV);

        // 5. Fence
        var fence_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateFence(0, .NONE, &dx.IID_ID3D12Fence, &fence_raw))) return false;
        self.fence = @ptrCast(@alignCast(fence_raw.?));
        self.fence_event = dx.CreateEventW(null, dx.FALSE, dx.FALSE, null) orelse return false;

        // 6. Compile shaders & create PSO
        if (!self.createPipeline()) return false;

        // 7. Create render target + readback
        if (!self.createRenderTarget()) return false;

        // 8. Create cell buffers
        if (!self.createCellBuffers()) return false;

        // 9. Create atlas texture
        if (!self.createAtlasTexture()) return false;

        // 10. Create command list (initially closed)
        var cl_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateCommandList(0, .DIRECT, @ptrCast(self.cmd_alloc), null, &dx.IID_ID3D12GraphicsCommandList, &cl_raw))) return false;
        self.cmd_list = @ptrCast(@alignCast(cl_raw.?));
        _ = self.cmd_list.Close();

        // Pre-rasterize ASCII glyphs
        var cp: u32 = 32;
        while (cp < 127) : (cp += 1) {
            _ = self.rasterizeGlyph(cp);
        }
        self.uploadAtlas();

        return true;
    }

    fn createPipeline(self: *GpuRenderer) bool {
        // Compile vertex shader
        var vs_blob: ?*dx.ID3DBlob = null;
        var vs_err: ?*dx.ID3DBlob = null;
        if (!dx.SUCCEEDED(dx.D3DCompile(
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
        ))) {
            if (vs_err) |e| _ = e.Release();
            return false;
        }
        defer _ = vs_blob.?.Release();
        if (vs_err) |e| _ = e.Release();

        // Compile pixel shader
        var ps_blob: ?*dx.ID3DBlob = null;
        var ps_err: ?*dx.ID3DBlob = null;
        if (!dx.SUCCEEDED(dx.D3DCompile(
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
        ))) {
            if (ps_err) |e| _ = e.Release();
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
            // Param 1: root constants (8 floats = viewport_size, cell_size, atlas_inv_size, pad)
            .{
                .ParameterType = .@"32BIT_CONSTANTS",
                .u = .{ .Constants = .{ .ShaderRegister = 0, .Num32BitValues = 8 } },
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
        if (!dx.SUCCEEDED(dx.D3D12SerializeRootSignature(&rs_desc, .@"1_0", &sig_blob, &sig_err))) {
            if (sig_err) |e| _ = e.Release();
            return false;
        }
        defer _ = sig_blob.?.Release();
        if (sig_err) |e| _ = e.Release();

        var rs_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateRootSignature(
            0,
            sig_blob.?.GetBufferPointer(),
            sig_blob.?.GetBufferSize(),
            &dx.IID_ID3D12RootSignature,
            &rs_raw,
        ))) return false;
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
        if (!dx.SUCCEEDED(self.device.CreateGraphicsPipelineState(&pso_desc, &dx.IID_ID3D12PipelineState, &pso_raw))) return false;
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
            .Color = .{ 0.118, 0.118, 0.180, 1.0 }, // Catppuccin Mocha base
        };

        var rt_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateCommittedResource(
            &heap_default,
            .NONE,
            &rt_desc,
            .RENDER_TARGET,
            &clear_val,
            &dx.IID_ID3D12Resource,
            &rt_raw,
        ))) return false;
        self.rt_texture = @ptrCast(@alignCast(rt_raw.?));

        // RTV
        const rtv_handle = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateRenderTargetView(@ptrCast(self.rt_texture), null, rtv_handle);

        // Readback buffer
        const row_pitch = ((self.width * 4 + 255) / 256) * 256; // 256-byte aligned
        const rb_size = @as(u64, row_pitch) * @as(u64, self.height);
        const rb_desc = dx.D3D12_RESOURCE_DESC{
            .Dimension = .BUFFER,
            .Width = rb_size,
        };
        const heap_readback = dx.D3D12_HEAP_PROPERTIES{ .Type = .READBACK };

        var rb_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateCommittedResource(
            &heap_readback,
            .NONE,
            &rb_desc,
            .COPY_DEST,
            null,
            &dx.IID_ID3D12Resource,
            &rb_raw,
        ))) return false;
        self.readback_buf = @ptrCast(@alignCast(rb_raw.?));

        // Map readback buffer persistently
        var mapped: ?*anyopaque = null;
        if (dx.SUCCEEDED(self.readback_buf.Map(0, null, &mapped))) {
            self.readback_ptr = @ptrCast(mapped);
        }

        return true;
    }

    fn createCellBuffers(self: *GpuRenderer) bool {
        const cell_gpu_size = @sizeOf(GpuCellGpu);
        const buf_size = @as(u64, MAX_CELLS) * @as(u64, cell_gpu_size);

        // Upload buffer (CPU writable)
        const heap_upload = dx.D3D12_HEAP_PROPERTIES{ .Type = .UPLOAD };
        const buf_desc = dx.D3D12_RESOURCE_DESC{ .Dimension = .BUFFER, .Width = buf_size };

        var upload_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateCommittedResource(
            &heap_upload,
            .NONE,
            &buf_desc,
            .GENERIC_READ,
            null,
            &dx.IID_ID3D12Resource,
            &upload_raw,
        ))) return false;
        self.cell_buf_upload = @ptrCast(@alignCast(upload_raw.?));

        // Map it
        var mapped: ?*anyopaque = null;
        if (dx.SUCCEEDED(self.cell_buf_upload.Map(0, null, &mapped))) {
            self.cell_mapped = @ptrCast(mapped);
        }

        // GPU buffer (default heap, SRV)
        const heap_default = dx.D3D12_HEAP_PROPERTIES{ .Type = .DEFAULT };
        var gpu_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateCommittedResource(
            &heap_default,
            .NONE,
            &buf_desc,
            .COMMON,
            null,
            &dx.IID_ID3D12Resource,
            &gpu_raw,
        ))) return false;
        self.cell_buf_gpu = @ptrCast(@alignCast(gpu_raw.?));

        // Create SRV for cell buffer (slot 0 in srv_heap)
        const srv_desc = dx.D3D12_SHADER_RESOURCE_VIEW_DESC{
            .Format = .UNKNOWN,
            .ViewDimension = .TEXTURE2D, // Actually BUFFER, but we use raw
            .MipLevels = @intCast(MAX_CELLS),
        };
        _ = srv_desc; // TODO: StructuredBuffer SRV requires BUFFER dimension
        // For now we'll use the upload buffer directly via root SRV

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
        if (!dx.SUCCEEDED(self.device.CreateCommittedResource(
            &heap_default,
            .NONE,
            &tex_desc,
            .COPY_DEST,
            null,
            &dx.IID_ID3D12Resource,
            &tex_raw,
        ))) return false;
        self.atlas_texture = @ptrCast(@alignCast(tex_raw.?));

        // Upload buffer for atlas data
        const row_pitch = ((ATLAS_SIZE + 255) / 256) * 256;
        const upload_size = @as(u64, row_pitch) * @as(u64, ATLAS_SIZE);
        const buf_desc = dx.D3D12_RESOURCE_DESC{ .Dimension = .BUFFER, .Width = upload_size };
        const heap_upload = dx.D3D12_HEAP_PROPERTIES{ .Type = .UPLOAD };

        var upload_raw: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.device.CreateCommittedResource(
            &heap_upload,
            .NONE,
            &buf_desc,
            .GENERIC_READ,
            null,
            &dx.IID_ID3D12Resource,
            &upload_raw,
        ))) return false;
        self.atlas_upload = @ptrCast(@alignCast(upload_raw.?));

        // Create SRV for atlas texture (slot 1 in srv_heap)
        const srv = dx.D3D12_SHADER_RESOURCE_VIEW_DESC{
            .Format = .R8_UNORM,
            .ViewDimension = .TEXTURE2D,
        };
        var handle = self.srv_heap.GetCPUDescriptorHandleForHeapStart();
        handle.ptr += self.srv_inc; // slot 1
        self.device.CreateShaderResourceView(@ptrCast(self.atlas_texture), &srv, handle);

        return true;
    }

    fn uploadAtlas(self: *GpuRenderer) void {
        if (!self.atlas_dirty) return;

        // Map upload buffer
        var mapped: ?*anyopaque = null;
        if (!dx.SUCCEEDED(self.atlas_upload.Map(0, null, &mapped))) return;
        const dst: [*]u8 = @ptrCast(mapped orelse return);

        // Copy R8 bitmap with row pitch alignment
        const row_pitch = ((ATLAS_SIZE + 255) / 256) * 256;
        var row: u32 = 0;
        while (row < ATLAS_SIZE) : (row += 1) {
            const src_off = row * ATLAS_SIZE;
            const dst_off = row * row_pitch;
            @memcpy(dst[dst_off .. dst_off + ATLAS_SIZE], self.atlas_bitmap[src_off .. src_off + ATLAS_SIZE]);
        }
        self.atlas_upload.Unmap(0, null);

        // Record copy command (atlas_upload → atlas_texture)
        _ = self.cmd_alloc.Reset();
        _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);

        const src_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.atlas_upload),
            .Type = 1, // PLACED_FOOTPRINT
            .u = .{
                .PlacedFootprint = .{
                    .Format = .R8_UNORM,
                    .Width = ATLAS_SIZE,
                    .Height = ATLAS_SIZE,
                    .RowPitch = row_pitch,
                },
            },
        };
        const dst_loc = dx.D3D12_TEXTURE_COPY_LOCATION{
            .pResource = @ptrCast(self.atlas_texture),
            .Type = 0, // SUBRESOURCE_INDEX
            .u = .{ .SubresourceIndex = 0 },
        };
        self.cmd_list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, null);

        // Transition atlas to shader resource
        const barrier = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.atlas_texture), .COPY_DEST, .PIXEL_SHADER_RESOURCE),
        };
        self.cmd_list.ResourceBarrier(1, &barrier);

        _ = self.cmd_list.Close();
        const lists = [_]*anyopaque{@ptrCast(self.cmd_list)};
        self.cmd_queue.ExecuteCommandLists(1, &lists);
        self.waitForGpu();

        self.atlas_dirty = false;
    }

    // ---- Frame rendering ----

    /// Render terminal cells to offscreen buffer. Returns RGBA pixel pointer.
    pub fn renderFrame(
        self: *GpuRenderer,
        cells: [*]const GpuCellData,
        cell_count: u32,
        cell_width: f32,
        cell_height: f32,
    ) ?[*]const u8 {
        const count = @min(cell_count, MAX_CELLS);
        if (count == 0) return self.readback_ptr;

        // Upload atlas if dirty
        if (self.atlas_dirty) {
            // Transition atlas back to COPY_DEST for upload
            _ = self.cmd_alloc.Reset();
            _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);
            const b = [_]dx.D3D12_RESOURCE_BARRIER{
                dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.atlas_texture), .PIXEL_SHADER_RESOURCE, .COPY_DEST),
            };
            self.cmd_list.ResourceBarrier(1, &b);
            _ = self.cmd_list.Close();
            const l = [_]*anyopaque{@ptrCast(self.cmd_list)};
            self.cmd_queue.ExecuteCommandLists(1, &l);
            self.waitForGpu();
            self.uploadAtlas();
        }

        // Convert cells to GPU format and upload
        if (self.cell_mapped) |mapped| {
            const gpu_cells: [*]GpuCellGpu = @ptrCast(@alignCast(mapped));
            const inv_atlas_w = 1.0 / @as(f32, @floatFromInt(ATLAS_SIZE));
            const inv_atlas_h = 1.0 / @as(f32, @floatFromInt(ATLAS_SIZE));

            var i: u32 = 0;
            while (i < count) : (i += 1) {
                const c = cells[i];
                // Ensure glyph is in atlas
                const rect = self.rasterizeGlyph(c.codepoint) orelse AtlasRect{ .x = 0, .y = 0, .w = 0, .h = 0 };

                gpu_cells[i] = .{
                    .pos = .{
                        @as(f32, @floatFromInt(c.col)) * cell_width,
                        @as(f32, @floatFromInt(c.row)) * cell_height,
                    },
                    .uv_origin = .{
                        @as(f32, @floatFromInt(rect.x)) * inv_atlas_w,
                        @as(f32, @floatFromInt(rect.y)) * inv_atlas_h,
                    },
                    .uv_size = .{
                        @as(f32, @floatFromInt(rect.w)) * inv_atlas_w,
                        @as(f32, @floatFromInt(rect.h)) * inv_atlas_h,
                    },
                    .fg = unpackRgba(c.fg_rgba),
                    .bg = unpackRgba(c.bg_rgba),
                };
            }

            // Re-upload atlas if new glyphs were rasterized
            if (self.atlas_dirty) {
                _ = self.cmd_alloc.Reset();
                _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);
                const b2 = [_]dx.D3D12_RESOURCE_BARRIER{
                    dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.atlas_texture), .PIXEL_SHADER_RESOURCE, .COPY_DEST),
                };
                self.cmd_list.ResourceBarrier(1, &b2);
                _ = self.cmd_list.Close();
                const l2 = [_]*anyopaque{@ptrCast(self.cmd_list)};
                self.cmd_queue.ExecuteCommandLists(1, &l2);
                self.waitForGpu();
                self.uploadAtlas();
            }
        }

        // Record draw commands
        _ = self.cmd_alloc.Reset();
        _ = self.cmd_list.Reset(@ptrCast(self.cmd_alloc), null);

        // Barrier: RT → RENDER_TARGET
        const b_rt = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.rt_texture), .COPY_SOURCE, .RENDER_TARGET),
        };
        self.cmd_list.ResourceBarrier(1, &b_rt);

        // Clear
        const rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        const clear_color = [4]f32{ 0.118, 0.118, 0.180, 1.0 };
        self.cmd_list.ClearRenderTargetView(rtv, &clear_color, 0, null);

        // Set render target
        self.cmd_list.OMSetRenderTargets(1, @ptrCast(&rtv), dx.FALSE, null);

        // Viewport & scissor
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

        // Set pipeline
        self.cmd_list.SetGraphicsRootSignature(self.root_sig);
        self.cmd_list.SetPipelineState(self.pso);
        self.cmd_list.IASetPrimitiveTopology(.TRIANGLELIST);

        // Set descriptor heap (SRV for atlas)
        const heaps = [_]*anyopaque{@ptrCast(self.srv_heap)};
        self.cmd_list.SetDescriptorHeaps(1, &heaps);
        self.cmd_list.SetGraphicsRootDescriptorTable(0, self.srv_heap.GetGPUDescriptorHandleForHeapStart());

        // Root constants (viewport_size, cell_size, atlas_inv_size, pad)
        const constants = [8]f32{
            @floatFromInt(self.width),
            @floatFromInt(self.height),
            cell_width,
            cell_height,
            1.0 / @as(f32, @floatFromInt(ATLAS_SIZE)),
            1.0 / @as(f32, @floatFromInt(ATLAS_SIZE)),
            0,
            0,
        };
        self.cmd_list.SetGraphicsRoot32BitConstants(1, 8, &constants, 0);

        // Draw instanced (6 verts per cell, count instances)
        self.cmd_list.DrawInstanced(6, count, 0, 0);

        // Barrier: RT → COPY_SOURCE
        const b_copy = [_]dx.D3D12_RESOURCE_BARRIER{
            dx.D3D12_RESOURCE_BARRIER.transition(@ptrCast(self.rt_texture), .RENDER_TARGET, .COPY_SOURCE),
        };
        self.cmd_list.ResourceBarrier(1, &b_copy);

        // Copy RT → readback
        self.cmd_list.CopyResource(@ptrCast(self.readback_buf), @ptrCast(self.rt_texture));

        // Close & execute
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
// C API exports
// ================================================================

var gpu_gpa: ?std.heap.GeneralPurposeAllocator(.{}) = null;

export fn ghostty_gpu_renderer_new(width: u32, height: u32, font_size: f32) ?*GpuRenderer {
    if (gpu_gpa == null) gpu_gpa = std.heap.GeneralPurposeAllocator(.{}){};
    return GpuRenderer.init(gpu_gpa.?.allocator(), width, height, font_size);
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
    cell_width: f32,
    cell_height: f32,
) ?[*]const u8 {
    if (r) |v| {
        if (cells) |c| return v.renderFrame(c, cell_count, cell_width, cell_height);
    }
    return null;
}

export fn ghostty_gpu_renderer_pixel_stride(r: ?*const GpuRenderer) u32 {
    if (r) |v| return ((v.width * 4 + 255) / 256) * 256;
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
