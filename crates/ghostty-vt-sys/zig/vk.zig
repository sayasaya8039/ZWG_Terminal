//! Minimal Vulkan bindings — dynamic function loading via vulkan-1.dll.
//! Only types and functions used by vulkan_renderer.zig are defined.

const std = @import("std");
const log = std.log.scoped(.vk);

// ================================================================
// Primitive types
// ================================================================

pub const VkResult = i32;
pub const VK_SUCCESS: VkResult = 0;
pub const VK_SUBOPTIMAL_KHR: VkResult = 1000001003;
pub const VK_ERROR_OUT_OF_DATE_KHR: VkResult = -1000001004;
pub const VK_FORMAT_B8G8R8A8_UNORM: i32 = 44;
pub const VK_FORMAT_R8_UNORM: i32 = 9;
pub const VK_FORMAT_R8G8B8A8_UNORM: i32 = 37;
pub const VK_IMAGE_LAYOUT_UNDEFINED: i32 = 0;
pub const VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL: i32 = 2;
pub const VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL: i32 = 5;
pub const VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL: i32 = 6;
pub const VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL: i32 = 7;
pub const VK_IMAGE_LAYOUT_PRESENT_SRC_KHR: i32 = 1000001002;
pub const VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT: u32 = 0x10;
pub const VK_IMAGE_USAGE_TRANSFER_SRC_BIT: u32 = 0x01;
pub const VK_IMAGE_USAGE_TRANSFER_DST_BIT: u32 = 0x02;
pub const VK_IMAGE_USAGE_SAMPLED_BIT: u32 = 0x04;
pub const VK_BUFFER_USAGE_STORAGE_BUFFER_BIT: u32 = 0x20;
pub const VK_BUFFER_USAGE_TRANSFER_SRC_BIT: u32 = 0x01;
pub const VK_BUFFER_USAGE_TRANSFER_DST_BIT: u32 = 0x02;
pub const VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT: u32 = 0x02;
pub const VK_MEMORY_PROPERTY_HOST_COHERENT_BIT: u32 = 0x04;
pub const VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT: u32 = 0x01;
pub const VK_SHARING_MODE_EXCLUSIVE: i32 = 0;
pub const VK_SAMPLE_COUNT_1_BIT: u32 = 0x01;
pub const VK_IMAGE_TILING_OPTIMAL: i32 = 0;
pub const VK_IMAGE_TILING_LINEAR: i32 = 1;
pub const VK_IMAGE_TYPE_2D: i32 = 1;
pub const VK_IMAGE_VIEW_TYPE_2D: i32 = 1;
pub const VK_COMPONENT_SWIZZLE_IDENTITY: i32 = 0;
pub const VK_IMAGE_ASPECT_COLOR_BIT: u32 = 0x01;
pub const VK_ATTACHMENT_LOAD_OP_CLEAR: i32 = 1;
pub const VK_ATTACHMENT_STORE_OP_STORE: i32 = 0;
pub const VK_SUBPASS_EXTERNAL: u32 = 0xFFFFFFFF;
pub const VK_PIPELINE_BIND_POINT_GRAPHICS: i32 = 0;
pub const VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT: u32 = 0x400;
pub const VK_PIPELINE_STAGE_TRANSFER_BIT: u32 = 0x1000;
pub const VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT: u32 = 0x01;
pub const VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT: u32 = 0x2000;
pub const VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT: u32 = 0x100;
pub const VK_ACCESS_TRANSFER_READ_BIT: u32 = 0x800;
pub const VK_ACCESS_TRANSFER_WRITE_BIT: u32 = 0x1000;
pub const VK_ACCESS_SHADER_READ_BIT: u32 = 0x20;
pub const VK_DESCRIPTOR_TYPE_STORAGE_BUFFER: i32 = 7;
pub const VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER: i32 = 1;
pub const VK_SHADER_STAGE_VERTEX_BIT: u32 = 0x01;
pub const VK_SHADER_STAGE_FRAGMENT_BIT: u32 = 0x10;
pub const VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST: i32 = 3;
pub const VK_POLYGON_MODE_FILL: i32 = 0;
pub const VK_CULL_MODE_NONE: u32 = 0;
pub const VK_FRONT_FACE_CLOCKWISE: i32 = 1;
pub const VK_DYNAMIC_STATE_VIEWPORT: i32 = 0;
pub const VK_DYNAMIC_STATE_SCISSOR: i32 = 1;
pub const VK_FILTER_NEAREST: i32 = 0;
pub const VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE: i32 = 2;
pub const VK_BORDER_COLOR_FLOAT_OPAQUE_BLACK: i32 = 2;
pub const VK_COMMAND_BUFFER_LEVEL_PRIMARY: i32 = 0;
pub const VK_FENCE_CREATE_SIGNALED_BIT: u32 = 0x01;
pub const VK_PRESENT_MODE_FIFO_KHR: i32 = 2;
pub const VK_PRESENT_MODE_MAILBOX_KHR: i32 = 3;
pub const VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR: u32 = 0x01;
pub const VK_COLOR_SPACE_SRGB_NONLINEAR_KHR: i32 = 0;
pub const VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT: u32 = 0x02;
pub const VK_QUEUE_GRAPHICS_BIT: u32 = 0x01;
pub const VK_API_VERSION_1_0: u32 = (1 << 22) | (0 << 12) | 0;

// Null handles
pub const VK_NULL_HANDLE: u64 = 0;

// ================================================================
// Opaque handle types (non-dispatchable = u64, dispatchable = *opaque)
// ================================================================

pub const VkInstance = ?*anyopaque;
pub const VkPhysicalDevice = ?*anyopaque;
pub const VkDevice = ?*anyopaque;
pub const VkQueue = ?*anyopaque;
pub const VkCommandPool = u64;
pub const VkCommandBuffer = ?*anyopaque;
pub const VkFence = u64;
pub const VkSemaphore = u64;
pub const VkBuffer = u64;
pub const VkDeviceMemory = u64;
pub const VkImage = u64;
pub const VkImageView = u64;
pub const VkSampler = u64;
pub const VkRenderPass = u64;
pub const VkFramebuffer = u64;
pub const VkPipeline = u64;
pub const VkPipelineLayout = u64;
pub const VkPipelineCache = u64;
pub const VkDescriptorPool = u64;
pub const VkDescriptorSetLayout = u64;
pub const VkDescriptorSet = u64;
pub const VkShaderModule = u64;
pub const VkSurfaceKHR = u64;
pub const VkSwapchainKHR = u64;

pub const VkDeviceSize = u64;
pub const VkFlags = u32;
pub const VkBool32 = u32;

pub const VK_TRUE: VkBool32 = 1;
pub const VK_FALSE: VkBool32 = 0;
pub const VK_WHOLE_SIZE: VkDeviceSize = 0xFFFFFFFFFFFFFFFF;

// ================================================================
// Structures (only fields we use)
// ================================================================

pub const VkExtent2D = extern struct {
    width: u32 = 0,
    height: u32 = 0,
};

pub const VkExtent3D = extern struct {
    width: u32 = 0,
    height: u32 = 0,
    depth: u32 = 1,
};

pub const VkOffset2D = extern struct {
    x: i32 = 0,
    y: i32 = 0,
};

pub const VkOffset3D = extern struct {
    x: i32 = 0,
    y: i32 = 0,
    z: i32 = 0,
};

pub const VkRect2D = extern struct {
    offset: VkOffset2D = .{},
    extent: VkExtent2D = .{},
};

pub const VkViewport = extern struct {
    x: f32 = 0,
    y: f32 = 0,
    width: f32 = 0,
    height: f32 = 0,
    minDepth: f32 = 0,
    maxDepth: f32 = 1,
};

pub const VkClearValue = extern union {
    color: VkClearColorValue,
    depthStencil: extern struct { depth: f32, stencil: u32 },
};

pub const VkClearColorValue = extern union {
    float32: [4]f32,
    int32: [4]i32,
    uint32: [4]u32,
};

pub const VkApplicationInfo = extern struct {
    sType: i32 = 0, // VK_STRUCTURE_TYPE_APPLICATION_INFO
    pNext: ?*const anyopaque = null,
    pApplicationName: ?[*:0]const u8 = null,
    applicationVersion: u32 = 0,
    pEngineName: ?[*:0]const u8 = null,
    engineVersion: u32 = 0,
    apiVersion: u32 = VK_API_VERSION_1_0,
};

pub const VkInstanceCreateInfo = extern struct {
    sType: i32 = 1, // VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    pApplicationInfo: ?*const VkApplicationInfo = null,
    enabledLayerCount: u32 = 0,
    ppEnabledLayerNames: ?[*]const [*:0]const u8 = null,
    enabledExtensionCount: u32 = 0,
    ppEnabledExtensionNames: ?[*]const [*:0]const u8 = null,
};

pub const VkPhysicalDeviceMemoryProperties = extern struct {
    memoryTypeCount: u32 = 0,
    memoryTypes: [32]VkMemoryType = [_]VkMemoryType{.{}} ** 32,
    memoryHeapCount: u32 = 0,
    memoryHeaps: [16]VkMemoryHeap = [_]VkMemoryHeap{.{}} ** 16,
};

pub const VkMemoryType = extern struct {
    propertyFlags: u32 = 0,
    heapIndex: u32 = 0,
};

pub const VkMemoryHeap = extern struct {
    size: VkDeviceSize = 0,
    flags: u32 = 0,
};

pub const VkQueueFamilyProperties = extern struct {
    queueFlags: u32 = 0,
    queueCount: u32 = 0,
    timestampValidBits: u32 = 0,
    minImageTransferGranularity: VkExtent3D = .{},
};

pub const VkDeviceQueueCreateInfo = extern struct {
    sType: i32 = 2, // VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    queueFamilyIndex: u32 = 0,
    queueCount: u32 = 1,
    pQueuePriorities: ?[*]const f32 = null,
};

pub const VkDeviceCreateInfo = extern struct {
    sType: i32 = 3, // VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    queueCreateInfoCount: u32 = 0,
    pQueueCreateInfos: ?[*]const VkDeviceQueueCreateInfo = null,
    enabledLayerCount: u32 = 0,
    ppEnabledLayerNames: ?[*]const [*:0]const u8 = null,
    enabledExtensionCount: u32 = 0,
    ppEnabledExtensionNames: ?[*]const [*:0]const u8 = null,
    pEnabledFeatures: ?*const anyopaque = null,
};

pub const VkMemoryAllocateInfo = extern struct {
    sType: i32 = 5, // VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO
    pNext: ?*const anyopaque = null,
    allocationSize: VkDeviceSize = 0,
    memoryTypeIndex: u32 = 0,
};

pub const VkBufferCreateInfo = extern struct {
    sType: i32 = 12, // VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    size: VkDeviceSize = 0,
    usage: u32 = 0,
    sharingMode: i32 = VK_SHARING_MODE_EXCLUSIVE,
    queueFamilyIndexCount: u32 = 0,
    pQueueFamilyIndices: ?[*]const u32 = null,
};

pub const VkMemoryRequirements = extern struct {
    size: VkDeviceSize = 0,
    alignment: VkDeviceSize = 0,
    memoryTypeBits: u32 = 0,
};

pub const VkFenceCreateInfo = extern struct {
    sType: i32 = 8, // VK_STRUCTURE_TYPE_FENCE_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
};

pub const VkCommandPoolCreateInfo = extern struct {
    sType: i32 = 39, // VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    queueFamilyIndex: u32 = 0,
};

pub const VkCommandBufferAllocateInfo = extern struct {
    sType: i32 = 40, // VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO
    pNext: ?*const anyopaque = null,
    commandPool: VkCommandPool = 0,
    level: i32 = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
    commandBufferCount: u32 = 1,
};

pub const VkCommandBufferBeginInfo = extern struct {
    sType: i32 = 42, // VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    pInheritanceInfo: ?*const anyopaque = null,
};

pub const VkSubmitInfo = extern struct {
    sType: i32 = 4, // VK_STRUCTURE_TYPE_SUBMIT_INFO
    pNext: ?*const anyopaque = null,
    waitSemaphoreCount: u32 = 0,
    pWaitSemaphores: ?[*]const VkSemaphore = null,
    pWaitDstStageMask: ?[*]const u32 = null,
    commandBufferCount: u32 = 0,
    pCommandBuffers: ?[*]const VkCommandBuffer = null,
    signalSemaphoreCount: u32 = 0,
    pSignalSemaphores: ?[*]const VkSemaphore = null,
};

pub const VkImageCreateInfo = extern struct {
    sType: i32 = 14, // VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    imageType: i32 = VK_IMAGE_TYPE_2D,
    format: i32 = VK_FORMAT_R8G8B8A8_UNORM,
    extent: VkExtent3D = .{},
    mipLevels: u32 = 1,
    arrayLayers: u32 = 1,
    samples: u32 = VK_SAMPLE_COUNT_1_BIT,
    tiling: i32 = VK_IMAGE_TILING_OPTIMAL,
    usage: u32 = 0,
    sharingMode: i32 = VK_SHARING_MODE_EXCLUSIVE,
    queueFamilyIndexCount: u32 = 0,
    pQueueFamilyIndices: ?[*]const u32 = null,
    initialLayout: i32 = VK_IMAGE_LAYOUT_UNDEFINED,
};

pub const VkImageViewCreateInfo = extern struct {
    sType: i32 = 15, // VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    image: VkImage = 0,
    viewType: i32 = VK_IMAGE_VIEW_TYPE_2D,
    format: i32 = VK_FORMAT_R8G8B8A8_UNORM,
    components: VkComponentMapping = .{},
    subresourceRange: VkImageSubresourceRange = .{},
};

pub const VkComponentMapping = extern struct {
    r: i32 = VK_COMPONENT_SWIZZLE_IDENTITY,
    g: i32 = VK_COMPONENT_SWIZZLE_IDENTITY,
    b: i32 = VK_COMPONENT_SWIZZLE_IDENTITY,
    a: i32 = VK_COMPONENT_SWIZZLE_IDENTITY,
};

pub const VkImageSubresourceRange = extern struct {
    aspectMask: u32 = VK_IMAGE_ASPECT_COLOR_BIT,
    baseMipLevel: u32 = 0,
    levelCount: u32 = 1,
    baseArrayLayer: u32 = 0,
    layerCount: u32 = 1,
};

pub const VkAttachmentDescription = extern struct {
    flags: u32 = 0,
    format: i32 = VK_FORMAT_R8G8B8A8_UNORM,
    samples: u32 = VK_SAMPLE_COUNT_1_BIT,
    loadOp: i32 = VK_ATTACHMENT_LOAD_OP_CLEAR,
    storeOp: i32 = VK_ATTACHMENT_STORE_OP_STORE,
    stencilLoadOp: i32 = 2, // DONT_CARE
    stencilStoreOp: i32 = 1, // DONT_CARE
    initialLayout: i32 = VK_IMAGE_LAYOUT_UNDEFINED,
    finalLayout: i32 = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
};

pub const VkAttachmentReference = extern struct {
    attachment: u32 = 0,
    layout: i32 = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
};

pub const VkSubpassDescription = extern struct {
    flags: u32 = 0,
    pipelineBindPoint: i32 = VK_PIPELINE_BIND_POINT_GRAPHICS,
    inputAttachmentCount: u32 = 0,
    pInputAttachments: ?[*]const VkAttachmentReference = null,
    colorAttachmentCount: u32 = 0,
    pColorAttachments: ?[*]const VkAttachmentReference = null,
    pResolveAttachments: ?[*]const VkAttachmentReference = null,
    pDepthStencilAttachment: ?*const VkAttachmentReference = null,
    preserveAttachmentCount: u32 = 0,
    pPreserveAttachments: ?[*]const u32 = null,
};

pub const VkSubpassDependency = extern struct {
    srcSubpass: u32 = VK_SUBPASS_EXTERNAL,
    dstSubpass: u32 = 0,
    srcStageMask: u32 = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
    dstStageMask: u32 = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
    srcAccessMask: u32 = 0,
    dstAccessMask: u32 = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
    dependencyFlags: u32 = 0,
};

pub const VkRenderPassCreateInfo = extern struct {
    sType: i32 = 38, // VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    attachmentCount: u32 = 0,
    pAttachments: ?[*]const VkAttachmentDescription = null,
    subpassCount: u32 = 0,
    pSubpasses: ?[*]const VkSubpassDescription = null,
    dependencyCount: u32 = 0,
    pDependencies: ?[*]const VkSubpassDependency = null,
};

pub const VkFramebufferCreateInfo = extern struct {
    sType: i32 = 37, // VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    renderPass: VkRenderPass = 0,
    attachmentCount: u32 = 0,
    pAttachments: ?[*]const VkImageView = null,
    width: u32 = 0,
    height: u32 = 0,
    layers: u32 = 1,
};

pub const VkShaderModuleCreateInfo = extern struct {
    sType: i32 = 16, // VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    codeSize: usize = 0,
    pCode: ?[*]const u32 = null,
};

pub const VkPushConstantRange = extern struct {
    stageFlags: u32 = 0,
    offset: u32 = 0,
    size: u32 = 0,
};

pub const VkDescriptorSetLayoutBinding = extern struct {
    binding: u32 = 0,
    descriptorType: i32 = 0,
    descriptorCount: u32 = 1,
    stageFlags: u32 = 0,
    pImmutableSamplers: ?[*]const VkSampler = null,
};

pub const VkDescriptorSetLayoutCreateInfo = extern struct {
    sType: i32 = 32, // VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    bindingCount: u32 = 0,
    pBindings: ?[*]const VkDescriptorSetLayoutBinding = null,
};

pub const VkPipelineLayoutCreateInfo = extern struct {
    sType: i32 = 30, // VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    setLayoutCount: u32 = 0,
    pSetLayouts: ?[*]const VkDescriptorSetLayout = null,
    pushConstantRangeCount: u32 = 0,
    pPushConstantRanges: ?[*]const VkPushConstantRange = null,
};

pub const VkPipelineShaderStageCreateInfo = extern struct {
    sType: i32 = 18, // VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    stage: u32 = 0,
    module: VkShaderModule = 0,
    pName: [*:0]const u8 = "main",
    pSpecializationInfo: ?*const anyopaque = null,
};

pub const VkPipelineVertexInputStateCreateInfo = extern struct {
    sType: i32 = 19,
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    vertexBindingDescriptionCount: u32 = 0,
    pVertexBindingDescriptions: ?*const anyopaque = null,
    vertexAttributeDescriptionCount: u32 = 0,
    pVertexAttributeDescriptions: ?*const anyopaque = null,
};

pub const VkPipelineInputAssemblyStateCreateInfo = extern struct {
    sType: i32 = 20,
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    topology: i32 = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
    primitiveRestartEnable: VkBool32 = VK_FALSE,
};

pub const VkPipelineViewportStateCreateInfo = extern struct {
    sType: i32 = 22,
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    viewportCount: u32 = 1,
    pViewports: ?*const VkViewport = null,
    scissorCount: u32 = 1,
    pScissors: ?*const VkRect2D = null,
};

pub const VkPipelineRasterizationStateCreateInfo = extern struct {
    sType: i32 = 23,
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    depthClampEnable: VkBool32 = VK_FALSE,
    rasterizerDiscardEnable: VkBool32 = VK_FALSE,
    polygonMode: i32 = VK_POLYGON_MODE_FILL,
    cullMode: u32 = VK_CULL_MODE_NONE,
    frontFace: i32 = VK_FRONT_FACE_CLOCKWISE,
    depthBiasEnable: VkBool32 = VK_FALSE,
    depthBiasConstantFactor: f32 = 0,
    depthBiasClamp: f32 = 0,
    depthBiasSlopeFactor: f32 = 0,
    lineWidth: f32 = 1.0,
};

pub const VkPipelineMultisampleStateCreateInfo = extern struct {
    sType: i32 = 24,
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    rasterizationSamples: u32 = VK_SAMPLE_COUNT_1_BIT,
    sampleShadingEnable: VkBool32 = VK_FALSE,
    minSampleShading: f32 = 1.0,
    pSampleMask: ?*const u32 = null,
    alphaToCoverageEnable: VkBool32 = VK_FALSE,
    alphaToOneEnable: VkBool32 = VK_FALSE,
};

pub const VkPipelineColorBlendAttachmentState = extern struct {
    blendEnable: VkBool32 = VK_FALSE,
    srcColorBlendFactor: i32 = 0,
    dstColorBlendFactor: i32 = 0,
    colorBlendOp: i32 = 0,
    srcAlphaBlendFactor: i32 = 0,
    dstAlphaBlendFactor: i32 = 0,
    alphaBlendOp: i32 = 0,
    colorWriteMask: u32 = 0x0F, // RGBA
};

pub const VkPipelineColorBlendStateCreateInfo = extern struct {
    sType: i32 = 26,
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    logicOpEnable: VkBool32 = VK_FALSE,
    logicOp: i32 = 0,
    attachmentCount: u32 = 0,
    pAttachments: ?[*]const VkPipelineColorBlendAttachmentState = null,
    blendConstants: [4]f32 = .{ 0, 0, 0, 0 },
};

pub const VkPipelineDynamicStateCreateInfo = extern struct {
    sType: i32 = 27,
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    dynamicStateCount: u32 = 0,
    pDynamicStates: ?[*]const i32 = null,
};

pub const VkGraphicsPipelineCreateInfo = extern struct {
    sType: i32 = 28,
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    stageCount: u32 = 0,
    pStages: ?[*]const VkPipelineShaderStageCreateInfo = null,
    pVertexInputState: ?*const VkPipelineVertexInputStateCreateInfo = null,
    pInputAssemblyState: ?*const VkPipelineInputAssemblyStateCreateInfo = null,
    pTessellationState: ?*const anyopaque = null,
    pViewportState: ?*const VkPipelineViewportStateCreateInfo = null,
    pRasterizationState: ?*const VkPipelineRasterizationStateCreateInfo = null,
    pMultisampleState: ?*const VkPipelineMultisampleStateCreateInfo = null,
    pDepthStencilState: ?*const anyopaque = null,
    pColorBlendState: ?*const VkPipelineColorBlendStateCreateInfo = null,
    pDynamicState: ?*const VkPipelineDynamicStateCreateInfo = null,
    layout: VkPipelineLayout = 0,
    renderPass: VkRenderPass = 0,
    subpass: u32 = 0,
    basePipelineHandle: VkPipeline = 0,
    basePipelineIndex: i32 = -1,
};

pub const VkDescriptorPoolSize = extern struct {
    type_: i32 = 0,
    descriptorCount: u32 = 0,
};

pub const VkDescriptorPoolCreateInfo = extern struct {
    sType: i32 = 33, // VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO
    pNext: ?*const anyopaque = null,
    flags: u32 = 0,
    maxSets: u32 = 0,
    poolSizeCount: u32 = 0,
    pPoolSizes: ?[*]const VkDescriptorPoolSize = null,
};

pub const VkDescriptorSetAllocateInfo = extern struct {
    sType: i32 = 34, // VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO
    pNext: ?*const anyopaque = null,
    descriptorPool: VkDescriptorPool = 0,
    descriptorSetCount: u32 = 0,
    pSetLayouts: ?[*]const VkDescriptorSetLayout = null,
};

pub const VkDescriptorBufferInfo = extern struct {
    buffer: VkBuffer = 0,
    offset: VkDeviceSize = 0,
    range: VkDeviceSize = VK_WHOLE_SIZE,
};

pub const VkWriteDescriptorSet = extern struct {
    sType: i32 = 35, // VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET
    pNext: ?*const anyopaque = null,
    dstSet: VkDescriptorSet = 0,
    dstBinding: u32 = 0,
    dstArrayElement: u32 = 0,
    descriptorCount: u32 = 0,
    descriptorType: i32 = 0,
    pImageInfo: ?*const anyopaque = null,
    pBufferInfo: ?*const VkDescriptorBufferInfo = null,
    pTexelBufferView: ?*const anyopaque = null,
};

pub const VkRenderPassBeginInfo = extern struct {
    sType: i32 = 43, // VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO
    pNext: ?*const anyopaque = null,
    renderPass: VkRenderPass = 0,
    framebuffer: VkFramebuffer = 0,
    renderArea: VkRect2D = .{},
    clearValueCount: u32 = 0,
    pClearValues: ?[*]const VkClearValue = null,
};

pub const VkImageMemoryBarrier = extern struct {
    sType: i32 = 45, // VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER
    pNext: ?*const anyopaque = null,
    srcAccessMask: u32 = 0,
    dstAccessMask: u32 = 0,
    oldLayout: i32 = VK_IMAGE_LAYOUT_UNDEFINED,
    newLayout: i32 = VK_IMAGE_LAYOUT_UNDEFINED,
    srcQueueFamilyIndex: u32 = 0xFFFFFFFF,
    dstQueueFamilyIndex: u32 = 0xFFFFFFFF,
    image: VkImage = 0,
    subresourceRange: VkImageSubresourceRange = .{},
};

pub const VkBufferImageCopy = extern struct {
    bufferOffset: VkDeviceSize = 0,
    bufferRowLength: u32 = 0,
    bufferImageHeight: u32 = 0,
    imageSubresource: VkImageSubresourceLayers = .{},
    imageOffset: VkOffset3D = .{},
    imageExtent: VkExtent3D = .{},
};

pub const VkImageSubresourceLayers = extern struct {
    aspectMask: u32 = VK_IMAGE_ASPECT_COLOR_BIT,
    mipLevel: u32 = 0,
    baseArrayLayer: u32 = 0,
    layerCount: u32 = 1,
};

// ================================================================
// Dynamic function loading
// ================================================================

pub const PFN_vkVoidFunction = ?*const fn () callconv(.c) void;
pub const PFN_vkGetInstanceProcAddr = *const fn (VkInstance, [*:0]const u8) callconv(.c) PFN_vkVoidFunction;

/// Load Vulkan entry point from vulkan-1.dll (Windows) or libvulkan.so (Linux).
/// Returns null if Vulkan is not available.
pub fn loadLoader() ?PFN_vkGetInstanceProcAddr {
    var lib = std.DynLib.open("vulkan-1.dll") catch return null;
    const sym = lib.lookup(PFN_vkGetInstanceProcAddr, "vkGetInstanceProcAddr") orelse return null;
    // intentionally leak DynLib — lives for process lifetime
    return sym;
}

/// Vulkan function table — loaded dynamically from instance/device.
pub const VkFuncs = struct {
    // Instance-level
    createInstance: *const fn (*const VkInstanceCreateInfo, ?*const anyopaque, *VkInstance) callconv(.c) VkResult = undefined,
    destroyInstance: *const fn (VkInstance, ?*const anyopaque) callconv(.c) void = undefined,
    enumeratePhysicalDevices: *const fn (VkInstance, *u32, ?[*]VkPhysicalDevice) callconv(.c) VkResult = undefined,
    getPhysicalDeviceQueueFamilyProperties: *const fn (VkPhysicalDevice, *u32, ?[*]VkQueueFamilyProperties) callconv(.c) void = undefined,
    getPhysicalDeviceMemoryProperties: *const fn (VkPhysicalDevice, *VkPhysicalDeviceMemoryProperties) callconv(.c) void = undefined,
    createDevice: *const fn (VkPhysicalDevice, *const VkDeviceCreateInfo, ?*const anyopaque, *VkDevice) callconv(.c) VkResult = undefined,

    // Device-level
    destroyDevice: *const fn (VkDevice, ?*const anyopaque) callconv(.c) void = undefined,
    getDeviceQueue: *const fn (VkDevice, u32, u32, *VkQueue) callconv(.c) void = undefined,
    createCommandPool: *const fn (VkDevice, *const VkCommandPoolCreateInfo, ?*const anyopaque, *VkCommandPool) callconv(.c) VkResult = undefined,
    destroyCommandPool: *const fn (VkDevice, VkCommandPool, ?*const anyopaque) callconv(.c) void = undefined,
    allocateCommandBuffers: *const fn (VkDevice, *const VkCommandBufferAllocateInfo, *VkCommandBuffer) callconv(.c) VkResult = undefined,
    beginCommandBuffer: *const fn (VkCommandBuffer, *const VkCommandBufferBeginInfo) callconv(.c) VkResult = undefined,
    endCommandBuffer: *const fn (VkCommandBuffer) callconv(.c) VkResult = undefined,
    resetCommandBuffer: *const fn (VkCommandBuffer, u32) callconv(.c) VkResult = undefined,
    queueSubmit: *const fn (VkQueue, u32, *const VkSubmitInfo, VkFence) callconv(.c) VkResult = undefined,
    queueWaitIdle: *const fn (VkQueue) callconv(.c) VkResult = undefined,
    createFence: *const fn (VkDevice, *const VkFenceCreateInfo, ?*const anyopaque, *VkFence) callconv(.c) VkResult = undefined,
    destroyFence: *const fn (VkDevice, VkFence, ?*const anyopaque) callconv(.c) void = undefined,
    waitForFences: *const fn (VkDevice, u32, *const VkFence, VkBool32, u64) callconv(.c) VkResult = undefined,
    resetFences: *const fn (VkDevice, u32, *const VkFence) callconv(.c) VkResult = undefined,
    createBuffer: *const fn (VkDevice, *const VkBufferCreateInfo, ?*const anyopaque, *VkBuffer) callconv(.c) VkResult = undefined,
    destroyBuffer: *const fn (VkDevice, VkBuffer, ?*const anyopaque) callconv(.c) void = undefined,
    getBufferMemoryRequirements: *const fn (VkDevice, VkBuffer, *VkMemoryRequirements) callconv(.c) void = undefined,
    allocateMemory: *const fn (VkDevice, *const VkMemoryAllocateInfo, ?*const anyopaque, *VkDeviceMemory) callconv(.c) VkResult = undefined,
    freeMemory: *const fn (VkDevice, VkDeviceMemory, ?*const anyopaque) callconv(.c) void = undefined,
    bindBufferMemory: *const fn (VkDevice, VkBuffer, VkDeviceMemory, VkDeviceSize) callconv(.c) VkResult = undefined,
    mapMemory: *const fn (VkDevice, VkDeviceMemory, VkDeviceSize, VkDeviceSize, u32, *?*anyopaque) callconv(.c) VkResult = undefined,
    deviceWaitIdle: *const fn (VkDevice) callconv(.c) VkResult = undefined,
    // Image
    createImage: *const fn (VkDevice, *const VkImageCreateInfo, ?*const anyopaque, *VkImage) callconv(.c) VkResult = undefined,
    destroyImage: *const fn (VkDevice, VkImage, ?*const anyopaque) callconv(.c) void = undefined,
    getImageMemoryRequirements: *const fn (VkDevice, VkImage, *VkMemoryRequirements) callconv(.c) void = undefined,
    bindImageMemory: *const fn (VkDevice, VkImage, VkDeviceMemory, VkDeviceSize) callconv(.c) VkResult = undefined,
    createImageView: *const fn (VkDevice, *const VkImageViewCreateInfo, ?*const anyopaque, *VkImageView) callconv(.c) VkResult = undefined,
    destroyImageView: *const fn (VkDevice, VkImageView, ?*const anyopaque) callconv(.c) void = undefined,
    // Render pass + framebuffer
    createRenderPass: *const fn (VkDevice, *const VkRenderPassCreateInfo, ?*const anyopaque, *VkRenderPass) callconv(.c) VkResult = undefined,
    destroyRenderPass: *const fn (VkDevice, VkRenderPass, ?*const anyopaque) callconv(.c) void = undefined,
    createFramebuffer: *const fn (VkDevice, *const VkFramebufferCreateInfo, ?*const anyopaque, *VkFramebuffer) callconv(.c) VkResult = undefined,
    destroyFramebuffer: *const fn (VkDevice, VkFramebuffer, ?*const anyopaque) callconv(.c) void = undefined,
    // Shader + pipeline
    createShaderModule: *const fn (VkDevice, *const VkShaderModuleCreateInfo, ?*const anyopaque, *VkShaderModule) callconv(.c) VkResult = undefined,
    destroyShaderModule: *const fn (VkDevice, VkShaderModule, ?*const anyopaque) callconv(.c) void = undefined,
    createPipelineLayout: *const fn (VkDevice, *const VkPipelineLayoutCreateInfo, ?*const anyopaque, *VkPipelineLayout) callconv(.c) VkResult = undefined,
    destroyPipelineLayout: *const fn (VkDevice, VkPipelineLayout, ?*const anyopaque) callconv(.c) void = undefined,
    createGraphicsPipelines: *const fn (VkDevice, VkPipelineCache, u32, [*]const VkGraphicsPipelineCreateInfo, ?*const anyopaque, *VkPipeline) callconv(.c) VkResult = undefined,
    destroyPipeline: *const fn (VkDevice, VkPipeline, ?*const anyopaque) callconv(.c) void = undefined,
    // Descriptors
    createDescriptorSetLayout: *const fn (VkDevice, *const VkDescriptorSetLayoutCreateInfo, ?*const anyopaque, *VkDescriptorSetLayout) callconv(.c) VkResult = undefined,
    destroyDescriptorSetLayout: *const fn (VkDevice, VkDescriptorSetLayout, ?*const anyopaque) callconv(.c) void = undefined,
    createDescriptorPool: *const fn (VkDevice, *const VkDescriptorPoolCreateInfo, ?*const anyopaque, *VkDescriptorPool) callconv(.c) VkResult = undefined,
    destroyDescriptorPool: *const fn (VkDevice, VkDescriptorPool, ?*const anyopaque) callconv(.c) void = undefined,
    allocateDescriptorSets: *const fn (VkDevice, *const VkDescriptorSetAllocateInfo, *VkDescriptorSet) callconv(.c) VkResult = undefined,
    updateDescriptorSets: *const fn (VkDevice, u32, [*]const VkWriteDescriptorSet, u32, ?*const anyopaque) callconv(.c) void = undefined,
    // Command recording
    cmdBeginRenderPass: *const fn (VkCommandBuffer, *const VkRenderPassBeginInfo, i32) callconv(.c) void = undefined,
    cmdEndRenderPass: *const fn (VkCommandBuffer) callconv(.c) void = undefined,
    cmdBindPipeline: *const fn (VkCommandBuffer, i32, VkPipeline) callconv(.c) void = undefined,
    cmdBindDescriptorSets: *const fn (VkCommandBuffer, i32, VkPipelineLayout, u32, u32, [*]const VkDescriptorSet, u32, ?[*]const u32) callconv(.c) void = undefined,
    cmdDraw: *const fn (VkCommandBuffer, u32, u32, u32, u32) callconv(.c) void = undefined,
    cmdSetViewport: *const fn (VkCommandBuffer, u32, u32, [*]const VkViewport) callconv(.c) void = undefined,
    cmdSetScissor: *const fn (VkCommandBuffer, u32, u32, [*]const VkRect2D) callconv(.c) void = undefined,
    cmdPipelineBarrier: *const fn (VkCommandBuffer, u32, u32, u32, u32, ?*const anyopaque, u32, ?*const anyopaque, u32, ?[*]const VkImageMemoryBarrier) callconv(.c) void = undefined,
    cmdCopyImageToBuffer: *const fn (VkCommandBuffer, VkImage, i32, VkBuffer, u32, [*]const VkBufferImageCopy) callconv(.c) void = undefined,
    cmdPushConstants: *const fn (VkCommandBuffer, VkPipelineLayout, u32, u32, u32, *const anyopaque) callconv(.c) void = undefined,

    pub fn loadInstance(gipa: PFN_vkGetInstanceProcAddr, instance: VkInstance) VkFuncs {
        var self: VkFuncs = .{};
        inline for (@typeInfo(VkFuncs).@"struct".fields) |field| {
            if (field.name[0] != '_') {
                const name = @as([*:0]const u8, @ptrCast(field.name.ptr));
                if (gipa(instance, name)) |fp| {
                    @field(self, field.name) = @ptrCast(fp);
                }
            }
        }
        return self;
    }

    pub fn loadGlobal(gipa: PFN_vkGetInstanceProcAddr) VkFuncs {
        return loadInstance(gipa, null);
    }
};

/// Find a memory type index matching `type_bits` and `properties`.
pub fn findMemoryType(
    mem_props: *const VkPhysicalDeviceMemoryProperties,
    type_bits: u32,
    properties: u32,
) ?u32 {
    var bits = type_bits;
    for (0..mem_props.memoryTypeCount) |i| {
        if ((bits & 1) == 1 and (mem_props.memoryTypes[i].propertyFlags & properties) == properties) {
            return @intCast(i);
        }
        bits >>= 1;
    }
    return null;
}
