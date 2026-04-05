//! Vulkan GPU terminal renderer — offscreen cell rendering with glyph atlas.
//!
//! Architecture (mirrors DX12 gpu_renderer.zig):
//!   1. DirectWrite rasterises glyphs → R8 bitmap atlas
//!   2. Atlas uploaded to VkImage (R8_UNORM)
//!   3. Per-frame: cell data → VkBuffer (storage) → instanced draw (6 verts/cell)
//!   4. Render target → readback buffer → CPU-accessible RGBA pixels
//!   5. Rust/GPUI displays the pixel buffer as an image
//!
//! All Vulkan operations happen in this file; Rust never touches Vulkan directly.
//! Same FFI interface as DX12 GpuRenderer — Rust calls ghostty_vulkan_renderer_*.

const std = @import("std");
const vk = @import("vk.zig");
const gpu = @import("gpu_renderer.zig"); // reuse GpuCellData type

const Allocator = std.mem.Allocator;
const log = std.log.scoped(.vulkan_renderer);

const ATLAS_SIZE: u32 = 2048;
const MAX_CELLS: u32 = 400 * 120;

// ================================================================
// VulkanRenderer
// ================================================================

pub const VulkanRenderer = struct {
    alloc: Allocator,
    funcs: vk.VkFuncs,

    // Core objects
    instance: vk.VkInstance,
    physical_device: vk.VkPhysicalDevice,
    device: vk.VkDevice,
    queue: vk.VkQueue,
    queue_family: u32,
    mem_props: vk.VkPhysicalDeviceMemoryProperties,

    // Command recording
    cmd_pool: vk.VkCommandPool,
    cmd_buf: vk.VkCommandBuffer,
    fence: vk.VkFence,

    // Cell data (host-visible storage buffer)
    cell_buf: vk.VkBuffer,
    cell_mem: vk.VkDeviceMemory,
    cell_mapped: ?[*]u8,

    // Offscreen render target (RGBA8)
    rt_image: vk.VkImage,
    rt_mem: vk.VkDeviceMemory,
    width: u32,
    height: u32,

    // Readback buffer
    readback_buf: vk.VkBuffer,
    readback_mem: vk.VkDeviceMemory,
    readback_mapped: ?[*]u8,

    // Glyph atlas
    glyph_count: u32,
    atlas_bitmap: []u8,
    atlas_dirty: bool,

    // GDI/DirectWrite (reuse from DX12 renderer — same rasterization)
    glyph_cell_w: u32,
    glyph_cell_h: u32,

    // State flags
    initialized: bool,

    // ---- Initialization ----

    pub fn init(alloc: Allocator, width: u32, height: u32, font_size: f32) ?*VulkanRenderer {
        _ = font_size; // TODO: use for glyph rasterization
        log.info("initializing Vulkan GPU renderer {}x{}", .{ width, height });

        // Load Vulkan loader
        const gipa = vk.loadLoader() orelse {
            log.err("vulkan-1.dll not available — Vulkan backend disabled", .{});
            setVkInitError(1, -1);
            return null;
        };

        const self = alloc.create(VulkanRenderer) catch {
            log.err("failed to allocate VulkanRenderer", .{});
            setVkInitError(2, -1);
            return null;
        };
        self.* = undefined;
        self.alloc = alloc;
        self.width = width;
        self.height = height;
        self.initialized = false;
        self.glyph_count = 0;
        self.atlas_dirty = false;
        self.cell_mapped = null;
        self.readback_mapped = null;

        // Atlas bitmap (CPU)
        self.atlas_bitmap = alloc.alloc(u8, ATLAS_SIZE * ATLAS_SIZE) catch {
            log.err("failed to allocate atlas bitmap", .{});
            setVkInitError(4, -1);
            alloc.destroy(self);
            return null;
        };
        @memset(self.atlas_bitmap, 0);

        // Create Vulkan instance
        const global_funcs = vk.VkFuncs.loadGlobal(gipa);
        var app_info = vk.VkApplicationInfo{
            .pApplicationName = "ZWG Terminal",
            .applicationVersion = 1,
            .pEngineName = "ZWG",
            .engineVersion = 1,
            .apiVersion = vk.VK_API_VERSION_1_0,
        };
        var inst_ci = vk.VkInstanceCreateInfo{
            .pApplicationInfo = &app_info,
        };
        var instance: vk.VkInstance = null;
        const inst_result = global_funcs.createInstance(&inst_ci, null, &instance);
        if (inst_result != vk.VK_SUCCESS) {
            log.err("vkCreateInstance failed: {}", .{inst_result});
            setVkInitError(5, inst_result);
            alloc.free(self.atlas_bitmap);

            alloc.destroy(self);
            return null;
        }
        self.instance = instance;
        self.funcs = vk.VkFuncs.loadInstance(gipa, instance);

        // Enumerate physical devices
        var gpu_count: u32 = 0;
        _ = self.funcs.enumeratePhysicalDevices(instance, &gpu_count, null);
        if (gpu_count == 0) {
            log.err("no Vulkan-capable GPU found", .{});
            setVkInitError(6, -1);
            self.funcs.destroyInstance(instance, null);
            alloc.free(self.atlas_bitmap);

            alloc.destroy(self);
            return null;
        }

        var phys_devices: [8]vk.VkPhysicalDevice = .{null} ** 8;
        var count: u32 = @min(gpu_count, 8);
        _ = self.funcs.enumeratePhysicalDevices(instance, &count, &phys_devices);
        self.physical_device = phys_devices[0]; // Use first GPU

        // Find graphics queue family
        var qf_count: u32 = 0;
        self.funcs.getPhysicalDeviceQueueFamilyProperties(self.physical_device, &qf_count, null);
        var qf_props: [16]vk.VkQueueFamilyProperties = [_]vk.VkQueueFamilyProperties{.{}} ** 16;
        var qf_actual: u32 = @min(qf_count, 16);
        self.funcs.getPhysicalDeviceQueueFamilyProperties(self.physical_device, &qf_actual, &qf_props);

        var graphics_family: ?u32 = null;
        for (0..qf_actual) |i| {
            if ((qf_props[i].queueFlags & vk.VK_QUEUE_GRAPHICS_BIT) != 0) {
                graphics_family = @intCast(i);
                break;
            }
        }
        self.queue_family = graphics_family orelse {
            log.err("no graphics queue family found", .{});
            setVkInitError(7, -1);
            self.funcs.destroyInstance(instance, null);
            alloc.free(self.atlas_bitmap);

            alloc.destroy(self);
            return null;
        };

        // Create logical device
        const queue_priorities = [_]f32{1.0};
        const queue_ci = vk.VkDeviceQueueCreateInfo{
            .queueFamilyIndex = self.queue_family,
            .queueCount = 1,
            .pQueuePriorities = &queue_priorities,
        };
        const dev_ci = vk.VkDeviceCreateInfo{
            .queueCreateInfoCount = 1,
            .pQueueCreateInfos = @ptrCast(&queue_ci),
        };
        var device: vk.VkDevice = null;
        const dev_result = self.funcs.createDevice(self.physical_device, &dev_ci, null, &device);
        if (dev_result != vk.VK_SUCCESS) {
            log.err("vkCreateDevice failed: {}", .{dev_result});
            setVkInitError(8, dev_result);
            self.funcs.destroyInstance(instance, null);
            alloc.free(self.atlas_bitmap);

            alloc.destroy(self);
            return null;
        }
        self.device = device;

        // Get queue
        var queue: vk.VkQueue = null;
        self.funcs.getDeviceQueue(device, self.queue_family, 0, &queue);
        self.queue = queue;

        // Get memory properties
        self.mem_props = .{};
        self.funcs.getPhysicalDeviceMemoryProperties(self.physical_device, &self.mem_props);

        // Create command pool + buffer
        if (!self.createCommandResources()) {
            self.funcs.destroyDevice(device, null);
            self.funcs.destroyInstance(instance, null);
            alloc.free(self.atlas_bitmap);

            alloc.destroy(self);
            return null;
        }

        // Create cell upload buffer
        if (!self.createCellBuffer()) {
            self.destroyCommandResources();
            self.funcs.destroyDevice(device, null);
            self.funcs.destroyInstance(instance, null);
            alloc.free(self.atlas_bitmap);

            alloc.destroy(self);
            return null;
        }

        // Create readback buffer
        if (!self.createReadbackBuffer()) {
            self.destroyCellBuffer();
            self.destroyCommandResources();
            self.funcs.destroyDevice(device, null);
            self.funcs.destroyInstance(instance, null);
            alloc.free(self.atlas_bitmap);

            alloc.destroy(self);
            return null;
        }

        self.initialized = true;
        log.info("Vulkan GPU renderer initialized successfully", .{});
        return self;
    }

    fn createCommandResources(self: *VulkanRenderer) bool {
        const pool_ci = vk.VkCommandPoolCreateInfo{
            .flags = vk.VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
            .queueFamilyIndex = self.queue_family,
        };
        var pool: vk.VkCommandPool = 0;
        if (self.funcs.createCommandPool(self.device, &pool_ci, null, &pool) != vk.VK_SUCCESS) {
            log.err("vkCreateCommandPool failed", .{});
            setVkInitError(10, -1);
            return false;
        }
        self.cmd_pool = pool;

        const alloc_info = vk.VkCommandBufferAllocateInfo{
            .commandPool = pool,
            .commandBufferCount = 1,
        };
        var cmd: vk.VkCommandBuffer = null;
        if (self.funcs.allocateCommandBuffers(self.device, &alloc_info, &cmd) != vk.VK_SUCCESS) {
            log.err("vkAllocateCommandBuffers failed", .{});
            setVkInitError(11, -1);
            return false;
        }
        self.cmd_buf = cmd;

        const fence_ci = vk.VkFenceCreateInfo{
            .flags = vk.VK_FENCE_CREATE_SIGNALED_BIT,
        };
        var fence: vk.VkFence = 0;
        if (self.funcs.createFence(self.device, &fence_ci, null, &fence) != vk.VK_SUCCESS) {
            log.err("vkCreateFence failed", .{});
            setVkInitError(12, -1);
            return false;
        }
        self.fence = fence;
        return true;
    }

    fn destroyCommandResources(self: *VulkanRenderer) void {
        if (self.fence != 0) self.funcs.destroyFence(self.device, self.fence, null);
        if (self.cmd_pool != 0) self.funcs.destroyCommandPool(self.device, self.cmd_pool, null);
    }

    fn createBuffer(
        self: *VulkanRenderer,
        size: vk.VkDeviceSize,
        usage: u32,
        mem_flags: u32,
        buf: *vk.VkBuffer,
        mem: *vk.VkDeviceMemory,
    ) bool {
        const buf_ci = vk.VkBufferCreateInfo{
            .size = size,
            .usage = usage,
        };
        if (self.funcs.createBuffer(self.device, &buf_ci, null, buf) != vk.VK_SUCCESS) return false;

        var reqs: vk.VkMemoryRequirements = .{};
        self.funcs.getBufferMemoryRequirements(self.device, buf.*, &reqs);

        const type_idx = vk.findMemoryType(&self.mem_props, reqs.memoryTypeBits, mem_flags) orelse return false;
        const alloc_info = vk.VkMemoryAllocateInfo{
            .allocationSize = reqs.size,
            .memoryTypeIndex = type_idx,
        };
        if (self.funcs.allocateMemory(self.device, &alloc_info, null, mem) != vk.VK_SUCCESS) return false;
        if (self.funcs.bindBufferMemory(self.device, buf.*, mem.*, 0) != vk.VK_SUCCESS) return false;
        return true;
    }

    fn createCellBuffer(self: *VulkanRenderer) bool {
        const size = @as(vk.VkDeviceSize, MAX_CELLS) * @sizeOf(gpu.GpuCellData);
        var buf: vk.VkBuffer = 0;
        var mem: vk.VkDeviceMemory = 0;
        if (!self.createBuffer(
            size,
            vk.VK_BUFFER_USAGE_STORAGE_BUFFER_BIT | vk.VK_BUFFER_USAGE_TRANSFER_DST_BIT,
            vk.VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | vk.VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
            &buf,
            &mem,
        )) {
            log.err("failed to create cell buffer", .{});
            setVkInitError(13, -1);
            return false;
        }
        self.cell_buf = buf;
        self.cell_mem = mem;

        // Persistent map
        var mapped: ?*anyopaque = null;
        if (self.funcs.mapMemory(self.device, mem, 0, size, 0, &mapped) != vk.VK_SUCCESS) {
            log.err("failed to map cell buffer", .{});
            setVkInitError(14, -1);
            return false;
        }
        self.cell_mapped = @ptrCast(mapped);
        return true;
    }

    fn destroyCellBuffer(self: *VulkanRenderer) void {
        if (self.cell_buf != 0) self.funcs.destroyBuffer(self.device, self.cell_buf, null);
        if (self.cell_mem != 0) self.funcs.freeMemory(self.device, self.cell_mem, null);
    }

    fn createReadbackBuffer(self: *VulkanRenderer) bool {
        const size = @as(vk.VkDeviceSize, self.width) * self.height * 4;
        var buf: vk.VkBuffer = 0;
        var mem: vk.VkDeviceMemory = 0;
        if (!self.createBuffer(
            size,
            vk.VK_BUFFER_USAGE_TRANSFER_DST_BIT,
            vk.VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | vk.VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
            &buf,
            &mem,
        )) {
            log.err("failed to create readback buffer", .{});
            setVkInitError(15, -1);
            return false;
        }
        self.readback_buf = buf;
        self.readback_mem = mem;

        var mapped: ?*anyopaque = null;
        if (self.funcs.mapMemory(self.device, mem, 0, size, 0, &mapped) != vk.VK_SUCCESS) {
            log.err("failed to map readback buffer", .{});
            setVkInitError(16, -1);
            return false;
        }
        self.readback_mapped = @ptrCast(mapped);
        return true;
    }

    fn destroyReadbackBuffer(self: *VulkanRenderer) void {
        if (self.readback_buf != 0) self.funcs.destroyBuffer(self.device, self.readback_buf, null);
        if (self.readback_mem != 0) self.funcs.freeMemory(self.device, self.readback_mem, null);
    }

    // ---- Rendering ----

    /// Upload cell data and render to readback buffer.
    /// Returns pointer to CPU-readable RGBA pixels, or null on error.
    pub fn renderFrame(
        self: *VulkanRenderer,
        cells: [*]const gpu.GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) ?[*]const u8 {
        // TODO: use for pipeline uniform upload
        _ = term_cols;
        _ = cell_width;
        _ = cell_height;
        if (!self.initialized) return null;
        const count: u32 = @min(cell_count, MAX_CELLS);

        // Upload cell data
        if (self.cell_mapped) |mapped| {
            const dst: [*]gpu.GpuCellData = @ptrCast(@alignCast(mapped));
            @memcpy(dst[0..count], cells[0..count]);
        }

        // TODO: Record command buffer with instanced draw
        // For now, clear readback to bg color from first cell
        if (self.readback_mapped) |rb| {
            const pixel_count = self.width * self.height;
            const bg = if (count > 0) cells[0].bg_rgba else 0xFF000000;
            const r: u8 = @intCast((bg >> 16) & 0xFF);
            const g: u8 = @intCast((bg >> 8) & 0xFF);
            const b: u8 = @intCast(bg & 0xFF);
            const a: u8 = @intCast((bg >> 24) & 0xFF);
            for (0..pixel_count) |i| {
                rb[i * 4 + 0] = r;
                rb[i * 4 + 1] = g;
                rb[i * 4 + 2] = b;
                rb[i * 4 + 3] = a;
            }
        }

        return self.readback_mapped;
    }

    pub fn resize(self: *VulkanRenderer, width: u32, height: u32) bool {
        if (!self.initialized) return false;
        _ = self.funcs.deviceWaitIdle(self.device);

        // Recreate readback buffer
        self.destroyReadbackBuffer();
        self.width = width;
        self.height = height;
        return self.createReadbackBuffer();
    }

    pub fn pixelStride(self: *const VulkanRenderer) u32 {
        return self.width * 4;
    }

    // ---- Cleanup ----

    pub fn deinit(self: *VulkanRenderer) void {
        if (self.initialized) {
            _ = self.funcs.deviceWaitIdle(self.device);
        }
        self.destroyReadbackBuffer();
        self.destroyCellBuffer();
        self.destroyCommandResources();
        if (self.device != null) self.funcs.destroyDevice(self.device, null);
        if (self.instance != null) self.funcs.destroyInstance(self.instance, null);
        self.alloc.free(self.atlas_bitmap);
        self.alloc.destroy(self);
    }
};

// ================================================================
// Error tracking (mirrors DX12 pattern)
// ================================================================

var vk_last_init_stage: u32 = 0;
var vk_last_init_hr: i32 = 0;

fn setVkInitError(stage: u32, hr: i32) void {
    vk_last_init_stage = stage;
    vk_last_init_hr = hr;
}

fn clearVkInitError() void {
    vk_last_init_stage = 0;
    vk_last_init_hr = 0;
}

// ================================================================
// FFI exports (C ABI — same interface as DX12 ghostty_gpu_renderer_*)
// ================================================================

var vk_gpa: ?std.heap.GeneralPurposeAllocator(.{}) = null;

export fn ghostty_vulkan_renderer_new(width: u32, height: u32, font_size: f32) ?*VulkanRenderer {
    clearVkInitError();
    if (vk_gpa == null) vk_gpa = std.heap.GeneralPurposeAllocator(.{}){};
    return VulkanRenderer.init(vk_gpa.?.allocator(), width, height, font_size);
}

export fn ghostty_vulkan_renderer_last_init_error(stage_out: *u32, hr_out: *i32) callconv(.c) void {
    stage_out.* = vk_last_init_stage;
    hr_out.* = vk_last_init_hr;
}

export fn ghostty_vulkan_renderer_free(r: ?*VulkanRenderer) void {
    if (r) |v| v.deinit();
}

export fn ghostty_vulkan_renderer_resize(r: ?*VulkanRenderer, width: u32, height: u32) u8 {
    if (r) |v| return @intFromBool(v.resize(width, height));
    return 0;
}

export fn ghostty_vulkan_renderer_render(
    r: ?*VulkanRenderer,
    cells: ?[*]const gpu.GpuCellData,
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

export fn ghostty_vulkan_renderer_width(r: ?*const VulkanRenderer) u32 {
    if (r) |v| return v.width;
    return 0;
}

export fn ghostty_vulkan_renderer_height(r: ?*const VulkanRenderer) u32 {
    if (r) |v| return v.height;
    return 0;
}

export fn ghostty_vulkan_renderer_pixel_stride(r: ?*const VulkanRenderer) u32 {
    if (r) |v| return v.pixelStride();
    return 0;
}
