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
const dx = @import("dx12.zig"); // GDI/DirectWrite types

const Allocator = std.mem.Allocator;
const log = std.log.scoped(.vulkan_renderer);

const ATLAS_SIZE: u32 = 2048;
const MAX_CELLS: u32 = 400 * 120;
const GLYPH_PAD: u32 = 1;
const GLYPH_MAP_CAP: usize = 4096;

const GlyphSlot = struct {
    key: u32, // codepoint
    atlas_index: u32,
    occupied: bool,
};

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
    rt_view: vk.VkImageView,
    width: u32,
    height: u32,

    // Pipeline objects
    render_pass: vk.VkRenderPass,
    framebuffer: vk.VkFramebuffer,
    desc_set_layout: vk.VkDescriptorSetLayout,
    desc_pool: vk.VkDescriptorPool,
    desc_set: vk.VkDescriptorSet,
    pipeline_layout: vk.VkPipelineLayout,
    pipeline: vk.VkPipeline,

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

    // Atlas GPU resources
    atlas_image: vk.VkImage,
    atlas_mem: vk.VkDeviceMemory,
    atlas_view: vk.VkImageView,
    atlas_sampler: vk.VkSampler,
    atlas_upload_buf: vk.VkBuffer,
    atlas_upload_mem: vk.VkDeviceMemory,
    atlas_upload_mapped: ?[*]u8,

    // GDI/DirectWrite (same as gpu_renderer.zig)
    gdi_dc: ?*anyopaque, // HDC
    gdi_font: ?*anyopaque, // HFONT
    dwrite_factory: ?*dx.IDWriteFactory,
    dwrite_gdi_interop: ?*dx.IDWriteGdiInterop,
    dwrite_font_face: ?*dx.IDWriteFontFace,
    glyph_baseline: f32,
    atlas_cols: u32,
    atlas_rows: u32,
    atlas_slot_count: u32,
    atlas_dirty_words: []u64,
    glyph_map: []GlyphSlot,

    // State flags
    initialized: bool,

    // ---- Initialization ----

    pub fn init(alloc: Allocator, width: u32, height: u32, font_size: f32) ?*VulkanRenderer {
        log.info("initializing Vulkan GPU renderer {}x{} font_size={d:.1}", .{ width, height, font_size });

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

        // Atlas GPU resources init
        self.atlas_image = 0;
        self.atlas_mem = 0;
        self.atlas_view = 0;
        self.atlas_sampler = 0;
        self.atlas_upload_buf = 0;
        self.atlas_upload_mem = 0;
        self.atlas_upload_mapped = null;

        // GDI/DirectWrite init
        self.gdi_dc = null;
        self.gdi_font = null;
        self.dwrite_factory = null;
        self.dwrite_gdi_interop = null;
        self.dwrite_font_face = null;
        self.glyph_baseline = 0;
        self.atlas_cols = 0;
        self.atlas_rows = 0;
        self.atlas_slot_count = 0;
        self.atlas_dirty_words = &.{};
        self.glyph_map = &.{};
        self.glyph_cell_w = 0;
        self.glyph_cell_h = 0;

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

        // Create render target image + view
        if (!self.createRenderTarget()) {
            self.destroyReadbackBuffer();
            self.destroyCellBuffer();
            self.destroyCommandResources();
            self.funcs.destroyDevice(device, null);
            self.funcs.destroyInstance(instance, null);
            alloc.free(self.atlas_bitmap);
            alloc.destroy(self);
            return null;
        }

        // Create render pass + pipeline + descriptors
        if (!self.createPipeline()) {
            self.destroyRenderTarget();
            self.destroyReadbackBuffer();
            self.destroyCellBuffer();
            self.destroyCommandResources();
            self.funcs.destroyDevice(device, null);
            self.funcs.destroyInstance(instance, null);
            alloc.free(self.atlas_bitmap);
            alloc.destroy(self);
            return null;
        }

        // Create atlas GPU resources
        if (!self.createAtlasImage()) {
            log.warn("atlas image creation failed — text rendering disabled", .{});
        } else {
            self.updateAtlasDescriptor();
        }

        // Initialize GDI/DirectWrite for glyph rasterization
        if (!self.initGdi(font_size)) {
            log.warn("GDI/DirectWrite init failed — text rendering disabled", .{});
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

    // ---- Render target image ----

    fn createRenderTarget(self: *VulkanRenderer) bool {
        const img_ci = vk.VkImageCreateInfo{
            .extent = .{ .width = self.width, .height = self.height, .depth = 1 },
            .format = vk.VK_FORMAT_R8G8B8A8_UNORM,
            .usage = vk.VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | vk.VK_IMAGE_USAGE_TRANSFER_SRC_BIT,
        };
        var image: vk.VkImage = 0;
        if (self.funcs.createImage(self.device, &img_ci, null, &image) != vk.VK_SUCCESS) {
            log.err("vkCreateImage failed for render target", .{});
            setVkInitError(20, -1);
            return false;
        }
        self.rt_image = image;

        var reqs: vk.VkMemoryRequirements = .{};
        self.funcs.getImageMemoryRequirements(self.device, image, &reqs);
        const type_idx = vk.findMemoryType(&self.mem_props, reqs.memoryTypeBits, vk.VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) orelse {
            log.err("no device-local memory for render target", .{});
            setVkInitError(21, -1);
            return false;
        };
        const alloc_info = vk.VkMemoryAllocateInfo{
            .allocationSize = reqs.size,
            .memoryTypeIndex = type_idx,
        };
        var mem: vk.VkDeviceMemory = 0;
        if (self.funcs.allocateMemory(self.device, &alloc_info, null, &mem) != vk.VK_SUCCESS) {
            log.err("vkAllocateMemory failed for render target", .{});
            setVkInitError(22, -1);
            return false;
        }
        self.rt_mem = mem;
        if (self.funcs.bindImageMemory(self.device, image, mem, 0) != vk.VK_SUCCESS) return false;

        const view_ci = vk.VkImageViewCreateInfo{
            .image = image,
            .format = vk.VK_FORMAT_R8G8B8A8_UNORM,
        };
        var view: vk.VkImageView = 0;
        if (self.funcs.createImageView(self.device, &view_ci, null, &view) != vk.VK_SUCCESS) {
            log.err("vkCreateImageView failed", .{});
            setVkInitError(23, -1);
            return false;
        }
        self.rt_view = view;
        return true;
    }

    fn destroyRenderTarget(self: *VulkanRenderer) void {
        if (self.rt_view != 0) self.funcs.destroyImageView(self.device, self.rt_view, null);
        if (self.rt_image != 0) self.funcs.destroyImage(self.device, self.rt_image, null);
        if (self.rt_mem != 0) self.funcs.freeMemory(self.device, self.rt_mem, null);
    }

    // ---- Pipeline ----

    fn createPipeline(self: *VulkanRenderer) bool {
        // Render pass
        const attachment = vk.VkAttachmentDescription{
            .format = vk.VK_FORMAT_R8G8B8A8_UNORM,
        };
        const color_ref = vk.VkAttachmentReference{};
        const subpass = vk.VkSubpassDescription{
            .colorAttachmentCount = 1,
            .pColorAttachments = @ptrCast(&color_ref),
        };
        const dependency = vk.VkSubpassDependency{};
        const rp_ci = vk.VkRenderPassCreateInfo{
            .attachmentCount = 1,
            .pAttachments = @ptrCast(&attachment),
            .subpassCount = 1,
            .pSubpasses = @ptrCast(&subpass),
            .dependencyCount = 1,
            .pDependencies = @ptrCast(&dependency),
        };
        var rp: vk.VkRenderPass = 0;
        if (self.funcs.createRenderPass(self.device, &rp_ci, null, &rp) != vk.VK_SUCCESS) {
            log.err("vkCreateRenderPass failed", .{});
            setVkInitError(30, -1);
            return false;
        }
        self.render_pass = rp;

        // Framebuffer
        const fb_ci = vk.VkFramebufferCreateInfo{
            .renderPass = rp,
            .attachmentCount = 1,
            .pAttachments = @ptrCast(&self.rt_view),
            .width = self.width,
            .height = self.height,
        };
        var fb: vk.VkFramebuffer = 0;
        if (self.funcs.createFramebuffer(self.device, &fb_ci, null, &fb) != vk.VK_SUCCESS) {
            log.err("vkCreateFramebuffer failed", .{});
            setVkInitError(31, -1);
            return false;
        }
        self.framebuffer = fb;

        // Descriptor set layout (binding 0 = cell storage, binding 1 = atlas sampler)
        const bindings = [_]vk.VkDescriptorSetLayoutBinding{
            .{ .binding = 0, .descriptorType = vk.VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, .descriptorCount = 1, .stageFlags = vk.VK_SHADER_STAGE_VERTEX_BIT },
            .{ .binding = 1, .descriptorType = vk.VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, .descriptorCount = 1, .stageFlags = vk.VK_SHADER_STAGE_FRAGMENT_BIT },
        };
        const dsl_ci = vk.VkDescriptorSetLayoutCreateInfo{
            .bindingCount = 2,
            .pBindings = &bindings,
        };
        var dsl: vk.VkDescriptorSetLayout = 0;
        if (self.funcs.createDescriptorSetLayout(self.device, &dsl_ci, null, &dsl) != vk.VK_SUCCESS) {
            log.err("vkCreateDescriptorSetLayout failed", .{});
            setVkInitError(32, -1);
            return false;
        }
        self.desc_set_layout = dsl;

        // Push constant range (48 bytes: viewport/cell/atlas params)
        const pc_range = vk.VkPushConstantRange{
            .stageFlags = vk.VK_SHADER_STAGE_VERTEX_BIT | vk.VK_SHADER_STAGE_FRAGMENT_BIT,
            .offset = 0,
            .size = 48,
        };
        const pl_ci = vk.VkPipelineLayoutCreateInfo{
            .setLayoutCount = 1,
            .pSetLayouts = @ptrCast(&dsl),
            .pushConstantRangeCount = 1,
            .pPushConstantRanges = @ptrCast(&pc_range),
        };
        var pl: vk.VkPipelineLayout = 0;
        if (self.funcs.createPipelineLayout(self.device, &pl_ci, null, &pl) != vk.VK_SUCCESS) {
            log.err("vkCreatePipelineLayout failed", .{});
            setVkInitError(33, -1);
            return false;
        }
        self.pipeline_layout = pl;

        // Shader modules (SPIR-V embedded at compile time)
        const vert_spv = @embedFile("shaders/terminal.vert.spv");
        const frag_spv = @embedFile("shaders/terminal.frag.spv");

        const vert_ci = vk.VkShaderModuleCreateInfo{
            .codeSize = vert_spv.len,
            .pCode = @ptrCast(@alignCast(vert_spv.ptr)),
        };
        var vert_mod: vk.VkShaderModule = 0;
        if (self.funcs.createShaderModule(self.device, &vert_ci, null, &vert_mod) != vk.VK_SUCCESS) {
            log.err("vkCreateShaderModule failed (vertex)", .{});
            setVkInitError(34, -1);
            return false;
        }

        const frag_ci = vk.VkShaderModuleCreateInfo{
            .codeSize = frag_spv.len,
            .pCode = @ptrCast(@alignCast(frag_spv.ptr)),
        };
        var frag_mod: vk.VkShaderModule = 0;
        if (self.funcs.createShaderModule(self.device, &frag_ci, null, &frag_mod) != vk.VK_SUCCESS) {
            log.err("vkCreateShaderModule failed (fragment)", .{});
            setVkInitError(35, -1);
            self.funcs.destroyShaderModule(self.device, vert_mod, null);
            return false;
        }

        // Graphics pipeline
        const stages = [_]vk.VkPipelineShaderStageCreateInfo{
            .{ .stage = vk.VK_SHADER_STAGE_VERTEX_BIT, .module = vert_mod },
            .{ .stage = vk.VK_SHADER_STAGE_FRAGMENT_BIT, .module = frag_mod },
        };
        const vert_input = vk.VkPipelineVertexInputStateCreateInfo{};
        const input_assembly = vk.VkPipelineInputAssemblyStateCreateInfo{};
        const viewport_state = vk.VkPipelineViewportStateCreateInfo{};
        const rasterization = vk.VkPipelineRasterizationStateCreateInfo{};
        const multisample = vk.VkPipelineMultisampleStateCreateInfo{};
        const blend_attach = vk.VkPipelineColorBlendAttachmentState{};
        const color_blend = vk.VkPipelineColorBlendStateCreateInfo{
            .attachmentCount = 1,
            .pAttachments = @ptrCast(&blend_attach),
        };
        const dyn_states = [_]i32{ vk.VK_DYNAMIC_STATE_VIEWPORT, vk.VK_DYNAMIC_STATE_SCISSOR };
        const dynamic = vk.VkPipelineDynamicStateCreateInfo{
            .dynamicStateCount = 2,
            .pDynamicStates = &dyn_states,
        };
        const gp_ci = [_]vk.VkGraphicsPipelineCreateInfo{.{
            .stageCount = 2,
            .pStages = &stages,
            .pVertexInputState = &vert_input,
            .pInputAssemblyState = &input_assembly,
            .pViewportState = &viewport_state,
            .pRasterizationState = &rasterization,
            .pMultisampleState = &multisample,
            .pColorBlendState = &color_blend,
            .pDynamicState = &dynamic,
            .layout = pl,
            .renderPass = rp,
        }};
        var pipeline: vk.VkPipeline = 0;
        if (self.funcs.createGraphicsPipelines(self.device, 0, 1, &gp_ci, null, &pipeline) != vk.VK_SUCCESS) {
            log.err("vkCreateGraphicsPipelines failed", .{});
            setVkInitError(36, -1);
            self.funcs.destroyShaderModule(self.device, vert_mod, null);
            self.funcs.destroyShaderModule(self.device, frag_mod, null);
            return false;
        }
        self.pipeline = pipeline;

        // Shader modules no longer needed
        self.funcs.destroyShaderModule(self.device, vert_mod, null);
        self.funcs.destroyShaderModule(self.device, frag_mod, null);

        // Descriptor pool + set
        const pool_sizes = [_]vk.VkDescriptorPoolSize{
            .{ .type_ = vk.VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, .descriptorCount = 1 },
            .{ .type_ = vk.VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, .descriptorCount = 1 },
        };
        const dp_ci = vk.VkDescriptorPoolCreateInfo{
            .maxSets = 1,
            .poolSizeCount = 2,
            .pPoolSizes = &pool_sizes,
        };
        var dp: vk.VkDescriptorPool = 0;
        if (self.funcs.createDescriptorPool(self.device, &dp_ci, null, &dp) != vk.VK_SUCCESS) {
            log.err("vkCreateDescriptorPool failed", .{});
            setVkInitError(37, -1);
            return false;
        }
        self.desc_pool = dp;

        const ds_ai = vk.VkDescriptorSetAllocateInfo{
            .descriptorPool = dp,
            .descriptorSetCount = 1,
            .pSetLayouts = @ptrCast(&dsl),
        };
        var ds: vk.VkDescriptorSet = 0;
        if (self.funcs.allocateDescriptorSets(self.device, &ds_ai, &ds) != vk.VK_SUCCESS) {
            log.err("vkAllocateDescriptorSets failed", .{});
            setVkInitError(38, -1);
            return false;
        }
        self.desc_set = ds;

        // Write descriptor (cell buffer → binding 0)
        const buf_info = vk.VkDescriptorBufferInfo{ .buffer = self.cell_buf };
        const write = [_]vk.VkWriteDescriptorSet{.{
            .dstSet = ds,
            .dstBinding = 0,
            .descriptorCount = 1,
            .descriptorType = vk.VK_DESCRIPTOR_TYPE_STORAGE_BUFFER,
            .pBufferInfo = &buf_info,
        }};
        self.funcs.updateDescriptorSets(self.device, 1, &write, 0, null);

        log.info("Vulkan pipeline created: render_pass + framebuffer + pipeline + descriptors", .{});
        return true;
    }

    fn destroyPipeline(self: *VulkanRenderer) void {
        if (self.desc_pool != 0) self.funcs.destroyDescriptorPool(self.device, self.desc_pool, null);
        if (self.pipeline != 0) self.funcs.destroyPipeline(self.device, self.pipeline, null);
        if (self.pipeline_layout != 0) self.funcs.destroyPipelineLayout(self.device, self.pipeline_layout, null);
        if (self.desc_set_layout != 0) self.funcs.destroyDescriptorSetLayout(self.device, self.desc_set_layout, null);
        if (self.framebuffer != 0) self.funcs.destroyFramebuffer(self.device, self.framebuffer, null);
        if (self.render_pass != 0) self.funcs.destroyRenderPass(self.device, self.render_pass, null);
    }

    // ---- Atlas GPU resources ----

    fn createAtlasImage(self: *VulkanRenderer) bool {
        // R8_UNORM 2048x2048 image (SAMPLED + TRANSFER_DST)
        const img_ci = vk.VkImageCreateInfo{
            .extent = .{ .width = ATLAS_SIZE, .height = ATLAS_SIZE, .depth = 1 },
            .format = vk.VK_FORMAT_R8_UNORM,
            .usage = vk.VK_IMAGE_USAGE_SAMPLED_BIT | vk.VK_IMAGE_USAGE_TRANSFER_DST_BIT,
        };
        var image: vk.VkImage = 0;
        if (self.funcs.createImage(self.device, &img_ci, null, &image) != vk.VK_SUCCESS) {
            log.err("failed to create atlas image", .{});
            return false;
        }
        self.atlas_image = image;

        var reqs: vk.VkMemoryRequirements = .{};
        self.funcs.getImageMemoryRequirements(self.device, image, &reqs);
        const type_idx = vk.findMemoryType(&self.mem_props, reqs.memoryTypeBits, vk.VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) orelse return false;
        const alloc_info = vk.VkMemoryAllocateInfo{ .allocationSize = reqs.size, .memoryTypeIndex = type_idx };
        var mem: vk.VkDeviceMemory = 0;
        if (self.funcs.allocateMemory(self.device, &alloc_info, null, &mem) != vk.VK_SUCCESS) return false;
        self.atlas_mem = mem;
        if (self.funcs.bindImageMemory(self.device, image, mem, 0) != vk.VK_SUCCESS) return false;

        // Image view
        const view_ci = vk.VkImageViewCreateInfo{
            .image = image,
            .format = vk.VK_FORMAT_R8_UNORM,
        };
        var view: vk.VkImageView = 0;
        if (self.funcs.createImageView(self.device, &view_ci, null, &view) != vk.VK_SUCCESS) return false;
        self.atlas_view = view;

        // Sampler (NEAREST, CLAMP_TO_EDGE)
        const sampler_ci = vk.VkSamplerCreateInfo{};
        var sampler: vk.VkSampler = 0;
        if (self.funcs.createSampler(self.device, &sampler_ci, null, &sampler) != vk.VK_SUCCESS) return false;
        self.atlas_sampler = sampler;

        // Staging buffer (4MB = ATLAS_SIZE * ATLAS_SIZE)
        const staging_size: vk.VkDeviceSize = ATLAS_SIZE * ATLAS_SIZE;
        var sbuf: vk.VkBuffer = 0;
        var smem: vk.VkDeviceMemory = 0;
        if (!self.createBuffer(staging_size, vk.VK_BUFFER_USAGE_TRANSFER_SRC_BIT, vk.VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | vk.VK_MEMORY_PROPERTY_HOST_COHERENT_BIT, &sbuf, &smem)) return false;
        self.atlas_upload_buf = sbuf;
        self.atlas_upload_mem = smem;
        var mapped: ?*anyopaque = null;
        if (self.funcs.mapMemory(self.device, smem, 0, staging_size, 0, &mapped) != vk.VK_SUCCESS) return false;
        self.atlas_upload_mapped = @ptrCast(mapped);

        log.info("atlas image created {}x{} R8_UNORM + staging buffer", .{ ATLAS_SIZE, ATLAS_SIZE });
        return true;
    }

    fn destroyAtlasImage(self: *VulkanRenderer) void {
        if (self.atlas_sampler != 0) self.funcs.destroySampler(self.device, self.atlas_sampler, null);
        if (self.atlas_view != 0) self.funcs.destroyImageView(self.device, self.atlas_view, null);
        if (self.atlas_image != 0) self.funcs.destroyImage(self.device, self.atlas_image, null);
        if (self.atlas_mem != 0) self.funcs.freeMemory(self.device, self.atlas_mem, null);
        if (self.atlas_upload_buf != 0) self.funcs.destroyBuffer(self.device, self.atlas_upload_buf, null);
        if (self.atlas_upload_mem != 0) self.funcs.freeMemory(self.device, self.atlas_upload_mem, null);
    }

    fn updateAtlasDescriptor(self: *VulkanRenderer) void {
        const img_info = vk.VkDescriptorImageInfo{
            .sampler = self.atlas_sampler,
            .imageView = self.atlas_view,
            .imageLayout = vk.VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
        };
        const write = [_]vk.VkWriteDescriptorSet{.{
            .dstSet = self.desc_set,
            .dstBinding = 1,
            .descriptorCount = 1,
            .descriptorType = vk.VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
            .pImageInfo = &img_info,
        }};
        self.funcs.updateDescriptorSets(self.device, 1, &write, 0, null);
    }

    // ---- DirectWrite glyph rasterization ----

    fn initGdi(self: *VulkanRenderer, font_size: f32) bool {
        self.gdi_dc = dx.CreateCompatibleDC(null);
        if (self.gdi_dc == null) {
            log.err("initGdi: CreateCompatibleDC failed", .{});
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
            log.err("initGdi: glyph cell width {} invalid for atlas {}", .{ self.glyph_cell_w, ATLAS_SIZE });
            _ = dx.DeleteObject(self.gdi_font);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }

        var factory_raw: ?*anyopaque = null;
        const factory_hr = dx.DWriteCreateFactory(.SHARED, &dx.IID_IDWriteFactory, &factory_raw);
        if (!dx.SUCCEEDED(factory_hr) or factory_raw == null) {
            log.err("initGdi: DWriteCreateFactory failed hr=0x{x}", .{@as(u32, @bitCast(factory_hr))});
            _ = dx.DeleteObject(self.gdi_font);
            _ = dx.DeleteDC(self.gdi_dc);
            return false;
        }
        self.dwrite_factory = @ptrCast(@alignCast(factory_raw.?));

        var interop_raw: ?*anyopaque = null;
        const interop_hr = self.dwrite_factory.?.GetGdiInterop(&interop_raw);
        if (!dx.SUCCEEDED(interop_hr) or interop_raw == null) {
            log.err("initGdi: GetGdiInterop failed hr=0x{x}", .{@as(u32, @bitCast(interop_hr))});
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
            log.err("initGdi: CreateFontFaceFromHdc failed hr=0x{x}", .{@as(u32, @bitCast(font_face_hr))});
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
            log.err("initGdi: GetGdiCompatibleMetrics failed hr=0x{x}", .{@as(u32, @bitCast(metrics_hr))});
            self.deinitGdi();
            return false;
        }

        const metrics_height = @as(u32, metrics.ascent) + @as(u32, metrics.descent);
        self.glyph_cell_h = @max(self.glyph_cell_h, @max(metrics_height, 1));
        self.glyph_baseline = @as(f32, @floatFromInt(@min(metrics.ascent, @as(u16, @intCast(self.glyph_cell_h)))));

        const slot_pitch_h = self.glyph_cell_h + GLYPH_PAD;
        if (slot_pitch_h == 0 or slot_pitch_h > ATLAS_SIZE) {
            log.err("initGdi: glyph cell height {} invalid for atlas {}", .{ self.glyph_cell_h, ATLAS_SIZE });
            self.deinitGdi();
            return false;
        }
        self.atlas_cols = ATLAS_SIZE / slot_pitch_w;
        self.atlas_rows = ATLAS_SIZE / slot_pitch_h;
        if (self.atlas_cols == 0 or self.atlas_rows == 0) {
            log.err("initGdi: atlas packing invalid for pitch {}x{}", .{ slot_pitch_w, slot_pitch_h });
            self.deinitGdi();
            return false;
        }
        self.atlas_slot_count = self.atlas_cols * self.atlas_rows;
        const atlas_dirty_word_count = @as(usize, @intCast((self.atlas_slot_count + 63) / 64));
        self.atlas_dirty_words = self.alloc.alloc(u64, atlas_dirty_word_count) catch {
            log.err("initGdi: failed to allocate atlas dirty bitset", .{});
            self.deinitGdi();
            return false;
        };
        @memset(self.atlas_dirty_words, 0);

        // Allocate glyph map
        self.glyph_map = self.alloc.alloc(GlyphSlot, GLYPH_MAP_CAP) catch {
            log.err("initGdi: failed to allocate glyph map", .{});
            self.deinitGdi();
            return false;
        };
        for (self.glyph_map) |*s| s.* = .{ .key = 0, .atlas_index = 0, .occupied = false };

        log.info(
            "DirectWrite glyph rasterizer ready glyph_cell={}x{} baseline={d:.2} atlas_cols={} atlas_rows={}",
            .{ self.glyph_cell_w, self.glyph_cell_h, self.glyph_baseline, self.atlas_cols, self.atlas_rows },
        );
        return true;
    }

    fn deinitGdi(self: *VulkanRenderer) void {
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
    fn rasterizeGlyph(self: *VulkanRenderer, codepoint: u32) ?u32 {
        // Check cache first
        if (self.lookupGlyph(codepoint)) |idx| return idx;

        const w = self.glyph_cell_w;
        const h = self.glyph_cell_h;
        const rast_slot_pitch_w = w + GLYPH_PAD;
        const rast_slot_pitch_h = h + GLYPH_PAD;
        if (self.atlas_cols == 0) return null;
        const total_slots = self.atlas_cols * self.atlas_rows;
        if (self.glyph_count >= total_slots) return null;
        const atlas_index = self.glyph_count;
        const dst_x = (atlas_index % self.atlas_cols) * rast_slot_pitch_w;
        const dst_y = (atlas_index / self.atlas_cols) * rast_slot_pitch_h;

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
        if (!self.hasAtlasDirtySlot(atlas_index)) self.markAtlasDirty(atlas_index);
        return atlas_index + 1;
    }

    fn markAtlasDirty(self: *VulkanRenderer, atlas_index: u32) void {
        if (atlas_index >= self.atlas_slot_count) return;
        if (!self.atlas_dirty) {
            self.atlas_dirty = true;
        }
        const word_index = atlas_index / 64;
        const bit_index = @as(u6, @intCast(atlas_index % 64));
        self.atlas_dirty_words[word_index] |= (@as(u64, 1) << bit_index);
    }

    fn hasAtlasDirtySlot(self: *const VulkanRenderer, atlas_index: u32) bool {
        if (!self.atlas_dirty or atlas_index >= self.atlas_slot_count) return false;
        const word_index = atlas_index / 64;
        const bit_index = @as(u6, @intCast(atlas_index % 64));
        return (self.atlas_dirty_words[word_index] & (@as(u64, 1) << bit_index)) != 0;
    }

    fn lookupGlyph(self: *const VulkanRenderer, cp: u32) ?u32 {
        if (self.glyph_map.len == 0) return null;
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

    fn insertGlyph(self: *VulkanRenderer, cp: u32, atlas_index: u32) void {
        if (self.glyph_map.len == 0) return;
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

    fn destroyReadbackBuffer(self: *VulkanRenderer) void {
        if (self.readback_buf != 0) self.funcs.destroyBuffer(self.device, self.readback_buf, null);
        if (self.readback_mem != 0) self.funcs.freeMemory(self.device, self.readback_mem, null);
    }

    // ---- Rendering ----

    /// Upload cell data and render to readback buffer.
    /// Returns pointer to CPU-readable RGBA pixels, or null on error.
    /// Push constants layout (matches GLSL push_constant block)
    const PushConstants = extern struct {
        viewport_size: [2]f32,
        cell_size: [2]f32,
        atlas_pitch_inv: [2]f32,
        atlas_glyph_inv: [2]f32,
        term_cols: f32,
        atlas_grid_cols: f32,
        _pad: [2]f32,
    };

    pub fn renderFrame(
        self: *VulkanRenderer,
        cells: [*]const gpu.GpuCellData,
        cell_count: u32,
        term_cols: u32,
        cell_width: f32,
        cell_height: f32,
    ) ?[*]const u8 {
        if (!self.initialized) return null;
        const count: u32 = @min(cell_count, MAX_CELLS);
        if (count == 0) return self.readback_mapped;

        // Upload cell data to host-visible storage buffer
        if (self.cell_mapped) |mapped| {
            const dst: [*]gpu.GpuCellData = @ptrCast(@alignCast(mapped));
            @memcpy(dst[0..count], cells[0..count]);
        }

        // Rasterize any new glyphs
        for (0..count) |i| {
            const cell = cells[i];
            if (cell.codepoint != 0) {
                _ = self.rasterizeGlyph(cell.codepoint);
            }
        }

        // Wait for previous frame's fence
        _ = self.funcs.waitForFences(self.device, 1, @ptrCast(&self.fence), vk.VK_TRUE, 1_000_000_000);
        _ = self.funcs.resetFences(self.device, 1, @ptrCast(&self.fence));

        // Record command buffer
        _ = self.funcs.resetCommandBuffer(self.cmd_buf, 0);
        const begin_info = vk.VkCommandBufferBeginInfo{};
        _ = self.funcs.beginCommandBuffer(self.cmd_buf, &begin_info);

        // Upload atlas if dirty
        if (self.atlas_dirty and self.atlas_image != 0) {
            if (self.atlas_upload_mapped) |upload_ptr| {
                @memcpy(upload_ptr[0 .. ATLAS_SIZE * ATLAS_SIZE], self.atlas_bitmap);
            }
            // Transition atlas image UNDEFINED → TRANSFER_DST
            const atlas_barrier_dst = [_]vk.VkImageMemoryBarrier{.{
                .dstAccessMask = vk.VK_ACCESS_TRANSFER_WRITE_BIT,
                .oldLayout = vk.VK_IMAGE_LAYOUT_UNDEFINED,
                .newLayout = vk.VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                .image = self.atlas_image,
            }};
            self.funcs.cmdPipelineBarrier(self.cmd_buf, vk.VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, vk.VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, null, 0, null, 1, &atlas_barrier_dst);

            const copy = [_]vk.VkBufferImageCopy{.{
                .bufferRowLength = ATLAS_SIZE,
                .bufferImageHeight = ATLAS_SIZE,
                .imageExtent = .{ .width = ATLAS_SIZE, .height = ATLAS_SIZE, .depth = 1 },
            }};
            self.funcs.cmdCopyBufferToImage(self.cmd_buf, self.atlas_upload_buf, self.atlas_image, vk.VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &copy);

            // Transition atlas TRANSFER_DST → SHADER_READ_ONLY
            const atlas_barrier_read = [_]vk.VkImageMemoryBarrier{.{
                .srcAccessMask = vk.VK_ACCESS_TRANSFER_WRITE_BIT,
                .dstAccessMask = vk.VK_ACCESS_SHADER_READ_BIT,
                .oldLayout = vk.VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                .newLayout = vk.VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
                .image = self.atlas_image,
            }};
            self.funcs.cmdPipelineBarrier(self.cmd_buf, vk.VK_PIPELINE_STAGE_TRANSFER_BIT, vk.VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT, 0, 0, null, 0, null, 1, &atlas_barrier_read);
            self.atlas_dirty = false;
        }

        // Begin render pass (clear to bg color)
        const bg = cells[0].bg_rgba;
        const clear_color = vk.VkClearValue{ .color = .{ .float32 = .{
            @as(f32, @floatFromInt((bg >> 16) & 0xFF)) / 255.0,
            @as(f32, @floatFromInt((bg >> 8) & 0xFF)) / 255.0,
            @as(f32, @floatFromInt(bg & 0xFF)) / 255.0,
            @as(f32, @floatFromInt((bg >> 24) & 0xFF)) / 255.0,
        } } };
        const rp_begin = vk.VkRenderPassBeginInfo{
            .renderPass = self.render_pass,
            .framebuffer = self.framebuffer,
            .renderArea = .{ .extent = .{ .width = self.width, .height = self.height } },
            .clearValueCount = 1,
            .pClearValues = @ptrCast(&clear_color),
        };
        self.funcs.cmdBeginRenderPass(self.cmd_buf, &rp_begin, 0); // INLINE

        // Bind pipeline + descriptor set
        self.funcs.cmdBindPipeline(self.cmd_buf, vk.VK_PIPELINE_BIND_POINT_GRAPHICS, self.pipeline);
        self.funcs.cmdBindDescriptorSets(
            self.cmd_buf,
            vk.VK_PIPELINE_BIND_POINT_GRAPHICS,
            self.pipeline_layout,
            0,
            1,
            @ptrCast(&self.desc_set),
            0,
            null,
        );

        // Push constants
        const slot_pitch_w = self.glyph_cell_w + GLYPH_PAD;
        const slot_pitch_h = self.glyph_cell_h + GLYPH_PAD;
        const pc = PushConstants{
            .viewport_size = .{ @floatFromInt(self.width), @floatFromInt(self.height) },
            .cell_size = .{ cell_width, cell_height },
            .atlas_pitch_inv = .{
                if (self.atlas_cols > 0) @as(f32, @floatFromInt(slot_pitch_w)) / @as(f32, @floatFromInt(ATLAS_SIZE)) else 0,
                if (self.atlas_rows > 0) @as(f32, @floatFromInt(slot_pitch_h)) / @as(f32, @floatFromInt(ATLAS_SIZE)) else 0,
            },
            .atlas_glyph_inv = .{
                if (self.glyph_cell_w > 0) @as(f32, @floatFromInt(self.glyph_cell_w)) / @as(f32, @floatFromInt(ATLAS_SIZE)) else 0,
                if (self.glyph_cell_h > 0) @as(f32, @floatFromInt(self.glyph_cell_h)) / @as(f32, @floatFromInt(ATLAS_SIZE)) else 0,
            },
            .term_cols = @floatFromInt(term_cols),
            .atlas_grid_cols = @floatFromInt(self.atlas_cols),
            ._pad = .{ 0, 0 },
        };
        self.funcs.cmdPushConstants(
            self.cmd_buf,
            self.pipeline_layout,
            vk.VK_SHADER_STAGE_VERTEX_BIT | vk.VK_SHADER_STAGE_FRAGMENT_BIT,
            0,
            @sizeOf(PushConstants),
            @ptrCast(&pc),
        );

        // Dynamic viewport + scissor
        const viewport = [_]vk.VkViewport{.{
            .width = @floatFromInt(self.width),
            .height = @floatFromInt(self.height),
        }};
        self.funcs.cmdSetViewport(self.cmd_buf, 0, 1, &viewport);
        const scissor = [_]vk.VkRect2D{.{
            .extent = .{ .width = self.width, .height = self.height },
        }};
        self.funcs.cmdSetScissor(self.cmd_buf, 0, 1, &scissor);

        // Instanced draw: 6 verts/cell × count instances
        self.funcs.cmdDraw(self.cmd_buf, 6, count, 0, 0);

        self.funcs.cmdEndRenderPass(self.cmd_buf);

        // Transition RT → TRANSFER_SRC, then copy to readback buffer
        const barrier = [_]vk.VkImageMemoryBarrier{.{
            .srcAccessMask = vk.VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
            .dstAccessMask = vk.VK_ACCESS_TRANSFER_READ_BIT,
            .oldLayout = vk.VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, // render pass finalLayout
            .newLayout = vk.VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
            .image = self.rt_image,
        }};
        self.funcs.cmdPipelineBarrier(
            self.cmd_buf,
            vk.VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
            vk.VK_PIPELINE_STAGE_TRANSFER_BIT,
            0,
            0, null,
            0, null,
            1, &barrier,
        );

        const copy_region = [_]vk.VkBufferImageCopy{.{
            .imageExtent = .{ .width = self.width, .height = self.height, .depth = 1 },
        }};
        self.funcs.cmdCopyImageToBuffer(
            self.cmd_buf,
            self.rt_image,
            vk.VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
            self.readback_buf,
            1,
            &copy_region,
        );

        _ = self.funcs.endCommandBuffer(self.cmd_buf);

        // Submit
        const submit = vk.VkSubmitInfo{
            .commandBufferCount = 1,
            .pCommandBuffers = @ptrCast(&self.cmd_buf),
        };
        _ = self.funcs.queueSubmit(self.queue, 1, &submit, self.fence);

        // Wait for completion (readback is synchronous in offscreen mode)
        _ = self.funcs.waitForFences(self.device, 1, @ptrCast(&self.fence), vk.VK_TRUE, 1_000_000_000);

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
        self.destroyPipeline();
        self.destroyAtlasImage();
        self.deinitGdi();
        self.destroyRenderTarget();
        self.destroyReadbackBuffer();
        self.destroyCellBuffer();
        self.destroyCommandResources();
        if (self.device != null) self.funcs.destroyDevice(self.device, null);
        if (self.instance != null) self.funcs.destroyInstance(self.instance, null);
        self.alloc.free(self.atlas_bitmap);
        if (self.glyph_map.len != 0) self.alloc.free(self.glyph_map);
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
