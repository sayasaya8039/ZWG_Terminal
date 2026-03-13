//! Minimal DX12 / DXGI / D3DCompiler / GDI COM bindings for GPU terminal rendering.
//! Only the methods actually used by gpu_renderer.zig are typed; the rest are vtable slots.

const std = @import("std");

// ================================================================
// Windows primitive types
// ================================================================

pub const HRESULT = i32;
pub const HANDLE = *anyopaque;
pub const BOOL = i32;
pub const LPCWSTR = [*:0]const u16;
pub const TRUE: BOOL = 1;
pub const FALSE: BOOL = 0;
pub const INFINITE: u32 = 0xFFFFFFFF;
pub const S_OK: HRESULT = 0;

pub inline fn SUCCEEDED(hr: HRESULT) bool {
    return hr >= 0;
}

pub const GUID = extern struct {
    Data1: u32,
    Data2: u16,
    Data3: u16,
    Data4: [8]u8,
};

pub const RECT = extern struct {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
};

// ================================================================
// DXGI types
// ================================================================

pub const DXGI_FORMAT = enum(u32) {
    UNKNOWN = 0,
    R8G8B8A8_UNORM = 28,
    B8G8R8A8_UNORM = 87,
    R8_UNORM = 61,
    D32_FLOAT = 40,
    R32G32_FLOAT = 16,
    R32G32B32A32_FLOAT = 2,
    _,
};

pub const DXGI_SAMPLE_DESC = extern struct {
    Count: u32 = 1,
    Quality: u32 = 0,
};

// ================================================================
// D3D12 enums (as u32 for extern struct compat)
// ================================================================

pub const D3D_FEATURE_LEVEL = enum(u32) {
    @"11_0" = 0xb000,
    @"12_0" = 0xc000,
    _,
};

pub const D3D12_COMMAND_LIST_TYPE = enum(u32) {
    DIRECT = 0,
    BUNDLE = 1,
    COMPUTE = 2,
    COPY = 3,
    _,
};

pub const D3D12_COMMAND_QUEUE_FLAGS = enum(u32) {
    NONE = 0,
    _,
};

pub const D3D12_DESCRIPTOR_HEAP_TYPE = enum(u32) {
    CBV_SRV_UAV = 0,
    SAMPLER = 1,
    RTV = 2,
    DSV = 3,
    _,
};

pub const D3D12_DESCRIPTOR_HEAP_FLAGS = enum(u32) {
    NONE = 0,
    SHADER_VISIBLE = 1,
    _,
};

pub const D3D12_HEAP_TYPE = enum(u32) {
    DEFAULT = 1,
    UPLOAD = 2,
    READBACK = 3,
    _,
};

pub const D3D12_CPU_PAGE_PROPERTY = enum(u32) {
    UNKNOWN = 0,
    _,
};

pub const D3D12_MEMORY_POOL = enum(u32) {
    UNKNOWN = 0,
    _,
};

pub const D3D12_RESOURCE_DIMENSION = enum(u32) {
    UNKNOWN = 0,
    BUFFER = 1,
    TEXTURE1D = 2,
    TEXTURE2D = 3,
    TEXTURE3D = 4,
    _,
};

pub const D3D12_TEXTURE_LAYOUT = enum(u32) {
    UNKNOWN = 0,
    ROW_MAJOR = 1,
    _,
};

pub const D3D12_RESOURCE_FLAGS = enum(u32) {
    NONE = 0,
    ALLOW_RENDER_TARGET = 0x1,
    ALLOW_DEPTH_STENCIL = 0x2,
    ALLOW_UNORDERED_ACCESS = 0x4,
    _,
};

pub const D3D12_RESOURCE_STATES = enum(u32) {
    COMMON = 0,
    RENDER_TARGET = 0x4,
    COPY_DEST = 0x400,
    COPY_SOURCE = 0x800,
    GENERIC_READ = 0x1 | 0x2 | 0x40 | 0x80 | 0x200 | 0x800,
    PIXEL_SHADER_RESOURCE = 0x80,
    _,
};

pub const D3D12_HEAP_FLAGS = enum(u32) {
    NONE = 0,
    _,
};

pub const D3D12_FENCE_FLAGS = enum(u32) {
    NONE = 0,
    _,
};

pub const D3D12_RESOURCE_BARRIER_TYPE = enum(u32) {
    TRANSITION = 0,
    _,
};

pub const D3D12_RESOURCE_BARRIER_FLAGS = enum(u32) {
    NONE = 0,
    _,
};

pub const D3D12_PRIMITIVE_TOPOLOGY = enum(u32) {
    TRIANGLELIST = 4,
    _,
};

pub const D3D12_INPUT_CLASSIFICATION = enum(u32) {
    PER_VERTEX_DATA = 0,
    PER_INSTANCE_DATA = 1,
    _,
};

pub const D3D12_FILL_MODE = enum(u32) {
    SOLID = 3,
    _,
};

pub const D3D12_CULL_MODE = enum(u32) {
    NONE = 1,
    _,
};

pub const D3D12_BLEND = enum(u32) {
    ONE = 2,
    SRC_ALPHA = 5,
    INV_SRC_ALPHA = 6,
    _,
};

pub const D3D12_BLEND_OP = enum(u32) {
    ADD = 1,
    _,
};

pub const D3D12_LOGIC_OP = enum(u32) {
    NOOP = 5,
    _,
};

pub const D3D12_COLOR_WRITE_ENABLE = enum(u8) {
    ALL = 0xF,
    _,
};

pub const D3D12_ROOT_SIGNATURE_FLAGS = enum(u32) {
    ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT = 0x1,
    _,
};

pub const D3D_ROOT_SIGNATURE_VERSION = enum(u32) {
    @"1_0" = 1,
    _,
};

pub const D3D12_ROOT_PARAMETER_TYPE = enum(u32) {
    DESCRIPTOR_TABLE = 0,
    @"32BIT_CONSTANTS" = 1,
    CBV = 2,
    SRV = 3,
    UAV = 4,
    _,
};

pub const D3D12_SHADER_VISIBILITY = enum(u32) {
    ALL = 0,
    VERTEX = 1,
    PIXEL = 5,
    _,
};

pub const D3D12_DESCRIPTOR_RANGE_TYPE = enum(u32) {
    SRV = 0,
    UAV = 1,
    CBV = 2,
    SAMPLER = 3,
    _,
};

pub const D3D12_FILTER = enum(u32) {
    MIN_MAG_MIP_POINT = 0,
    MIN_MAG_MIP_LINEAR = 0x15,
    _,
};

pub const D3D12_TEXTURE_ADDRESS_MODE = enum(u32) {
    CLAMP = 3,
    _,
};

pub const D3D12_COMPARISON_FUNC = enum(u32) {
    NEVER = 1,
    _,
};

pub const D3D12_STATIC_BORDER_COLOR = enum(u32) {
    TRANSPARENT_BLACK = 0,
    _,
};

pub const D3D12_SRV_DIMENSION = enum(u32) {
    TEXTURE2D = 4,
    _,
};

pub const RESOURCE_BARRIER_ALL_SUBRESOURCES: u32 = 0xFFFFFFFF;

// ================================================================
// D3D12 structs
// ================================================================

pub const D3D12_COMMAND_QUEUE_DESC = extern struct {
    Type: D3D12_COMMAND_LIST_TYPE = .DIRECT,
    Priority: i32 = 0,
    Flags: D3D12_COMMAND_QUEUE_FLAGS = .NONE,
    NodeMask: u32 = 0,
};

pub const D3D12_DESCRIPTOR_HEAP_DESC = extern struct {
    Type: D3D12_DESCRIPTOR_HEAP_TYPE,
    NumDescriptors: u32,
    Flags: D3D12_DESCRIPTOR_HEAP_FLAGS = .NONE,
    NodeMask: u32 = 0,
};

pub const D3D12_CPU_DESCRIPTOR_HANDLE = extern struct {
    ptr: usize = 0,
};

pub const D3D12_GPU_DESCRIPTOR_HANDLE = extern struct {
    ptr: u64 = 0,
};

pub const D3D12_HEAP_PROPERTIES = extern struct {
    Type: D3D12_HEAP_TYPE,
    CPUPageProperty: D3D12_CPU_PAGE_PROPERTY = .UNKNOWN,
    MemoryPoolPreference: D3D12_MEMORY_POOL = .UNKNOWN,
    CreationNodeMask: u32 = 0,
    VisibleNodeMask: u32 = 0,
};

pub const D3D12_RESOURCE_DESC = extern struct {
    Dimension: D3D12_RESOURCE_DIMENSION = .BUFFER,
    Alignment: u64 = 0,
    Width: u64 = 0,
    Height: u32 = 1,
    DepthOrArraySize: u16 = 1,
    MipLevels: u16 = 1,
    Format: DXGI_FORMAT = .UNKNOWN,
    SampleDesc: DXGI_SAMPLE_DESC = .{},
    Layout: D3D12_TEXTURE_LAYOUT = .UNKNOWN,
    Flags: D3D12_RESOURCE_FLAGS = .NONE,
};

pub const D3D12_CLEAR_VALUE = extern struct {
    Format: DXGI_FORMAT,
    Color: [4]f32,
};

pub const D3D12_RESOURCE_TRANSITION_BARRIER = extern struct {
    pResource: ?*anyopaque = null,
    Subresource: u32 = RESOURCE_BARRIER_ALL_SUBRESOURCES,
    StateBefore: D3D12_RESOURCE_STATES = .COMMON,
    StateAfter: D3D12_RESOURCE_STATES = .COMMON,
};

pub const D3D12_RESOURCE_BARRIER = extern struct {
    Type: D3D12_RESOURCE_BARRIER_TYPE = .TRANSITION,
    Flags: D3D12_RESOURCE_BARRIER_FLAGS = .NONE,
    Transition: D3D12_RESOURCE_TRANSITION_BARRIER = .{},

    pub fn transition(resource: ?*anyopaque, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) D3D12_RESOURCE_BARRIER {
        return .{
            .Type = .TRANSITION,
            .Flags = .NONE,
            .Transition = .{
                .pResource = resource,
                .Subresource = RESOURCE_BARRIER_ALL_SUBRESOURCES,
                .StateBefore = before,
                .StateAfter = after,
            },
        };
    }
};

pub const D3D12_VIEWPORT = extern struct {
    TopLeftX: f32 = 0,
    TopLeftY: f32 = 0,
    Width: f32,
    Height: f32,
    MinDepth: f32 = 0,
    MaxDepth: f32 = 1,
};

pub const D3D12_VERTEX_BUFFER_VIEW = extern struct {
    BufferLocation: u64,
    SizeInBytes: u32,
    StrideInBytes: u32,
};

pub const D3D12_INPUT_ELEMENT_DESC = extern struct {
    SemanticName: [*:0]const u8,
    SemanticIndex: u32 = 0,
    Format: DXGI_FORMAT,
    InputSlot: u32 = 0,
    AlignedByteOffset: u32,
    InputSlotClass: D3D12_INPUT_CLASSIFICATION = .PER_VERTEX_DATA,
    InstanceDataStepRate: u32 = 0,
};

pub const D3D12_SHADER_BYTECODE = extern struct {
    pShaderBytecode: ?*const anyopaque = null,
    BytecodeLength: usize = 0,
};

pub const D3D12_RASTERIZER_DESC = extern struct {
    FillMode: D3D12_FILL_MODE = .SOLID,
    CullMode: D3D12_CULL_MODE = .NONE,
    FrontCounterClockwise: BOOL = FALSE,
    DepthBias: i32 = 0,
    DepthBiasClamp: f32 = 0,
    SlopeScaledDepthBias: f32 = 0,
    DepthClipEnable: BOOL = TRUE,
    MultisampleEnable: BOOL = FALSE,
    AntialiasedLineEnable: BOOL = FALSE,
    ForcedSampleCount: u32 = 0,
    ConservativeRaster: u32 = 0, // D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF
};

pub const D3D12_RENDER_TARGET_BLEND_DESC = extern struct {
    BlendEnable: BOOL = TRUE,
    LogicOpEnable: BOOL = FALSE,
    SrcBlend: D3D12_BLEND = .SRC_ALPHA,
    DestBlend: D3D12_BLEND = .INV_SRC_ALPHA,
    BlendOp: D3D12_BLEND_OP = .ADD,
    SrcBlendAlpha: D3D12_BLEND = .ONE,
    DestBlendAlpha: D3D12_BLEND = .INV_SRC_ALPHA,
    BlendOpAlpha: D3D12_BLEND_OP = .ADD,
    LogicOp: D3D12_LOGIC_OP = .NOOP,
    RenderTargetWriteMask: u8 = @intFromEnum(D3D12_COLOR_WRITE_ENABLE.ALL),
};

pub const D3D12_BLEND_DESC = extern struct {
    AlphaToCoverageEnable: BOOL = FALSE,
    IndependentBlendEnable: BOOL = FALSE,
    RenderTarget: [8]D3D12_RENDER_TARGET_BLEND_DESC = .{D3D12_RENDER_TARGET_BLEND_DESC{}} ** 8,
};

pub const D3D12_DEPTH_STENCIL_DESC = extern struct {
    DepthEnable: BOOL = FALSE,
    DepthWriteMask: u32 = 0,
    DepthFunc: u32 = 0,
    StencilEnable: BOOL = FALSE,
    StencilReadMask: u8 = 0xFF,
    StencilWriteMask: u8 = 0xFF,
    FrontFace: [4]u32 = .{ 0, 0, 0, 0 },
    BackFace: [4]u32 = .{ 0, 0, 0, 0 },
};

pub const D3D12_INPUT_LAYOUT_DESC = extern struct {
    pInputElementDescs: ?[*]const D3D12_INPUT_ELEMENT_DESC = null,
    NumElements: u32 = 0,
};

pub const D3D12_GRAPHICS_PIPELINE_STATE_DESC = extern struct {
    pRootSignature: ?*anyopaque = null,
    VS: D3D12_SHADER_BYTECODE = .{},
    PS: D3D12_SHADER_BYTECODE = .{},
    DS: D3D12_SHADER_BYTECODE = .{},
    HS: D3D12_SHADER_BYTECODE = .{},
    GS: D3D12_SHADER_BYTECODE = .{},
    StreamOutput: [5]usize = .{0} ** 5,
    BlendState: D3D12_BLEND_DESC = .{},
    SampleMask: u32 = 0xFFFFFFFF,
    RasterizerState: D3D12_RASTERIZER_DESC = .{},
    DepthStencilState: D3D12_DEPTH_STENCIL_DESC = .{},
    InputLayout: D3D12_INPUT_LAYOUT_DESC = .{},
    IBStripCutValue: u32 = 0,
    PrimitiveTopologyType: u32 = 4, // D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE
    NumRenderTargets: u32 = 1,
    RTVFormats: [8]DXGI_FORMAT = .{.UNKNOWN} ** 8,
    DSVFormat: DXGI_FORMAT = .UNKNOWN,
    SampleDesc: DXGI_SAMPLE_DESC = .{},
    NodeMask: u32 = 0,
    CachedPSO: [2]usize = .{0} ** 2,
    Flags: u32 = 0,
};

pub const D3D12_DESCRIPTOR_RANGE = extern struct {
    RangeType: D3D12_DESCRIPTOR_RANGE_TYPE,
    NumDescriptors: u32,
    BaseShaderRegister: u32,
    RegisterSpace: u32 = 0,
    OffsetInDescriptorsFromTableStart: u32 = 0xFFFFFFFF, // APPEND
};

pub const D3D12_ROOT_DESCRIPTOR_TABLE = extern struct {
    NumDescriptorRanges: u32,
    pDescriptorRanges: ?[*]const D3D12_DESCRIPTOR_RANGE,
};

pub const D3D12_ROOT_CONSTANTS = extern struct {
    ShaderRegister: u32,
    RegisterSpace: u32 = 0,
    Num32BitValues: u32,
};

pub const D3D12_ROOT_PARAMETER = extern struct {
    ParameterType: D3D12_ROOT_PARAMETER_TYPE,
    u: extern union {
        DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE,
        Constants: D3D12_ROOT_CONSTANTS,
        Descriptor: extern struct { ShaderRegister: u32, RegisterSpace: u32 },
    },
    ShaderVisibility: D3D12_SHADER_VISIBILITY = .ALL,
};

pub const D3D12_STATIC_SAMPLER_DESC = extern struct {
    Filter: D3D12_FILTER = .MIN_MAG_MIP_POINT,
    AddressU: D3D12_TEXTURE_ADDRESS_MODE = .CLAMP,
    AddressV: D3D12_TEXTURE_ADDRESS_MODE = .CLAMP,
    AddressW: D3D12_TEXTURE_ADDRESS_MODE = .CLAMP,
    MipLODBias: f32 = 0,
    MaxAnisotropy: u32 = 0,
    ComparisonFunc: D3D12_COMPARISON_FUNC = .NEVER,
    BorderColor: D3D12_STATIC_BORDER_COLOR = .TRANSPARENT_BLACK,
    MinLOD: f32 = 0,
    MaxLOD: f32 = 3.402823466e+38, // D3D12_FLOAT32_MAX
    ShaderRegister: u32 = 0,
    RegisterSpace: u32 = 0,
    ShaderVisibility: D3D12_SHADER_VISIBILITY = .PIXEL,
};

pub const D3D12_ROOT_SIGNATURE_DESC = extern struct {
    NumParameters: u32 = 0,
    pParameters: ?[*]const D3D12_ROOT_PARAMETER = null,
    NumStaticSamplers: u32 = 0,
    pStaticSamplers: ?[*]const D3D12_STATIC_SAMPLER_DESC = null,
    Flags: D3D12_ROOT_SIGNATURE_FLAGS = .ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
};

pub const D3D12_SHADER_RESOURCE_VIEW_DESC = extern struct {
    Format: DXGI_FORMAT,
    ViewDimension: D3D12_SRV_DIMENSION,
    Shader4ComponentMapping: u32 = 0x00001688, // D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING
    // Texture2D union member
    MostDetailedMip: u32 = 0,
    MipLevels: u32 = 1,
    PlaneSlice: u32 = 0,
    ResourceMinLODClamp: f32 = 0,
};

pub const D3D12_PLACED_SUBRESOURCE_FOOTPRINT = extern struct {
    Offset: u64 = 0,
    Format: DXGI_FORMAT = .UNKNOWN,
    Width: u32 = 0,
    Height: u32 = 0,
    Depth: u32 = 1,
    RowPitch: u32 = 0,
};

pub const D3D12_TEXTURE_COPY_LOCATION = extern struct {
    pResource: ?*anyopaque = null,
    Type: u32 = 0, // 0=SUBRESOURCE_INDEX, 1=PLACED_FOOTPRINT
    u: extern union {
        PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
        SubresourceIndex: u32,
    } = .{ .SubresourceIndex = 0 },
};

pub const D3D12_BOX = extern struct {
    left: u32 = 0,
    top: u32 = 0,
    front: u32 = 0,
    right: u32,
    bottom: u32,
    back: u32 = 1,
};

// ================================================================
// COM interface helper: vtable index → typed function pointer
// ================================================================

fn VtFn(comptime Self: type, comptime Fn: type) type {
    _ = Self;
    return Fn;
}

fn vtCall(ptr: ?*anyopaque, comptime idx: usize, comptime F: type) F {
    const base: [*]const usize = @ptrCast(@alignCast(ptr));
    return @ptrFromInt(base[idx]);
}

// ================================================================
// COM interfaces
// ================================================================

/// ID3DBlob (ID3D10Blob)
pub const ID3DBlob = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *ID3DBlob) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*ID3DBlob) callconv(.c) u32)(self);
    }
    pub fn GetBufferPointer(self: *ID3DBlob) ?*anyopaque {
        return vtCall(self.lpVtbl, 3, *const fn (*ID3DBlob) callconv(.c) ?*anyopaque)(self);
    }
    pub fn GetBufferSize(self: *ID3DBlob) usize {
        return vtCall(self.lpVtbl, 4, *const fn (*ID3DBlob) callconv(.c) usize)(self);
    }
};

/// ID3D12Device (vtable: IUnknown 0-2, ID3D12Object 3-6, Device 7-43)
pub const ID3D12Device = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *ID3D12Device) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*ID3D12Device) callconv(.c) u32)(self);
    }
    pub fn CreateCommandQueue(self: *ID3D12Device, desc: *const D3D12_COMMAND_QUEUE_DESC, riid: *const GUID, out: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 8, *const fn (*ID3D12Device, *const D3D12_COMMAND_QUEUE_DESC, *const GUID, *?*anyopaque) callconv(.c) HRESULT)(self, desc, riid, out);
    }
    pub fn CreateCommandAllocator(self: *ID3D12Device, typ: D3D12_COMMAND_LIST_TYPE, riid: *const GUID, out: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 9, *const fn (*ID3D12Device, D3D12_COMMAND_LIST_TYPE, *const GUID, *?*anyopaque) callconv(.c) HRESULT)(self, typ, riid, out);
    }
    pub fn CreateGraphicsPipelineState(self: *ID3D12Device, desc: *const D3D12_GRAPHICS_PIPELINE_STATE_DESC, riid: *const GUID, out: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 10, *const fn (*ID3D12Device, *const D3D12_GRAPHICS_PIPELINE_STATE_DESC, *const GUID, *?*anyopaque) callconv(.c) HRESULT)(self, desc, riid, out);
    }
    pub fn CreateCommandList(self: *ID3D12Device, node_mask: u32, typ: D3D12_COMMAND_LIST_TYPE, alloc: *anyopaque, initial_pso: ?*anyopaque, riid: *const GUID, out: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 12, *const fn (*ID3D12Device, u32, D3D12_COMMAND_LIST_TYPE, *anyopaque, ?*anyopaque, *const GUID, *?*anyopaque) callconv(.c) HRESULT)(self, node_mask, typ, alloc, initial_pso, riid, out);
    }
    pub fn CreateDescriptorHeap(self: *ID3D12Device, desc: *const D3D12_DESCRIPTOR_HEAP_DESC, riid: *const GUID, out: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 14, *const fn (*ID3D12Device, *const D3D12_DESCRIPTOR_HEAP_DESC, *const GUID, *?*anyopaque) callconv(.c) HRESULT)(self, desc, riid, out);
    }
    pub fn GetDescriptorHandleIncrementSize(self: *ID3D12Device, typ: D3D12_DESCRIPTOR_HEAP_TYPE) u32 {
        return vtCall(self.lpVtbl, 15, *const fn (*ID3D12Device, D3D12_DESCRIPTOR_HEAP_TYPE) callconv(.c) u32)(self, typ);
    }
    pub fn CreateRootSignature(self: *ID3D12Device, node_mask: u32, blob: ?*const anyopaque, blob_len: usize, riid: *const GUID, out: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 16, *const fn (*ID3D12Device, u32, ?*const anyopaque, usize, *const GUID, *?*anyopaque) callconv(.c) HRESULT)(self, node_mask, blob, blob_len, riid, out);
    }
    pub fn CreateShaderResourceView(self: *ID3D12Device, resource: ?*anyopaque, desc: ?*const D3D12_SHADER_RESOURCE_VIEW_DESC, handle: D3D12_CPU_DESCRIPTOR_HANDLE) void {
        vtCall(self.lpVtbl, 18, *const fn (*ID3D12Device, ?*anyopaque, ?*const D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_CPU_DESCRIPTOR_HANDLE) callconv(.c) void)(self, resource, desc, handle);
    }
    pub fn CreateRenderTargetView(self: *ID3D12Device, resource: ?*anyopaque, desc: ?*anyopaque, handle: D3D12_CPU_DESCRIPTOR_HANDLE) void {
        vtCall(self.lpVtbl, 20, *const fn (*ID3D12Device, ?*anyopaque, ?*anyopaque, D3D12_CPU_DESCRIPTOR_HANDLE) callconv(.c) void)(self, resource, desc, handle);
    }
    pub fn CreateCommittedResource(self: *ID3D12Device, heap_props: *const D3D12_HEAP_PROPERTIES, heap_flags: D3D12_HEAP_FLAGS, desc: *const D3D12_RESOURCE_DESC, initial_state: D3D12_RESOURCE_STATES, clear_value: ?*const D3D12_CLEAR_VALUE, riid: *const GUID, out: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 27, *const fn (*ID3D12Device, *const D3D12_HEAP_PROPERTIES, D3D12_HEAP_FLAGS, *const D3D12_RESOURCE_DESC, D3D12_RESOURCE_STATES, ?*const D3D12_CLEAR_VALUE, *const GUID, *?*anyopaque) callconv(.c) HRESULT)(self, heap_props, heap_flags, desc, initial_state, clear_value, riid, out);
    }
    pub fn CreateFence(self: *ID3D12Device, initial: u64, flags: D3D12_FENCE_FLAGS, riid: *const GUID, out: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 36, *const fn (*ID3D12Device, u64, D3D12_FENCE_FLAGS, *const GUID, *?*anyopaque) callconv(.c) HRESULT)(self, initial, flags, riid, out);
    }
};

/// ID3D12CommandQueue (vtable: IUnknown 0-2, Object 3-6, DeviceChild 7, Queue 8+)
pub const ID3D12CommandQueue = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *ID3D12CommandQueue) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*ID3D12CommandQueue) callconv(.c) u32)(self);
    }
    pub fn ExecuteCommandLists(self: *ID3D12CommandQueue, num: u32, lists: [*]const *anyopaque) void {
        vtCall(self.lpVtbl, 10, *const fn (*ID3D12CommandQueue, u32, [*]const *anyopaque) callconv(.c) void)(self, num, lists);
    }
    pub fn Signal(self: *ID3D12CommandQueue, fence: *anyopaque, value: u64) HRESULT {
        return vtCall(self.lpVtbl, 14, *const fn (*ID3D12CommandQueue, *anyopaque, u64) callconv(.c) HRESULT)(self, fence, value);
    }
};

/// ID3D12CommandAllocator (vtable: IUnknown 0-2, Object 3-6, DeviceChild 7, Alloc 8)
pub const ID3D12CommandAllocator = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *ID3D12CommandAllocator) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*ID3D12CommandAllocator) callconv(.c) u32)(self);
    }
    pub fn Reset(self: *ID3D12CommandAllocator) HRESULT {
        return vtCall(self.lpVtbl, 8, *const fn (*ID3D12CommandAllocator) callconv(.c) HRESULT)(self);
    }
};

/// ID3D12GraphicsCommandList (60 methods)
pub const ID3D12GraphicsCommandList = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *@This()) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*@This()) callconv(.c) u32)(self);
    }
    pub fn Close(self: *@This()) HRESULT {
        return vtCall(self.lpVtbl, 9, *const fn (*@This()) callconv(.c) HRESULT)(self);
    }
    pub fn Reset(self: *@This(), alloc: *anyopaque, pso: ?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 10, *const fn (*@This(), *anyopaque, ?*anyopaque) callconv(.c) HRESULT)(self, alloc, pso);
    }
    pub fn DrawInstanced(self: *@This(), vertex_count: u32, instance_count: u32, start_vertex: u32, start_instance: u32) void {
        vtCall(self.lpVtbl, 12, *const fn (*@This(), u32, u32, u32, u32) callconv(.c) void)(self, vertex_count, instance_count, start_vertex, start_instance);
    }
    pub fn CopyTextureRegion(self: *@This(), dst: *const D3D12_TEXTURE_COPY_LOCATION, dx: u32, dy: u32, dz: u32, src: *const D3D12_TEXTURE_COPY_LOCATION, src_box: ?*const D3D12_BOX) void {
        vtCall(self.lpVtbl, 16, *const fn (*@This(), *const D3D12_TEXTURE_COPY_LOCATION, u32, u32, u32, *const D3D12_TEXTURE_COPY_LOCATION, ?*const D3D12_BOX) callconv(.c) void)(self, dst, dx, dy, dz, src, src_box);
    }
    pub fn CopyResource(self: *@This(), dst: *anyopaque, src: *anyopaque) void {
        vtCall(self.lpVtbl, 17, *const fn (*@This(), *anyopaque, *anyopaque) callconv(.c) void)(self, dst, src);
    }
    pub fn IASetPrimitiveTopology(self: *@This(), topology: D3D12_PRIMITIVE_TOPOLOGY) void {
        vtCall(self.lpVtbl, 20, *const fn (*@This(), D3D12_PRIMITIVE_TOPOLOGY) callconv(.c) void)(self, topology);
    }
    pub fn RSSetViewports(self: *@This(), num: u32, viewports: [*]const D3D12_VIEWPORT) void {
        vtCall(self.lpVtbl, 21, *const fn (*@This(), u32, [*]const D3D12_VIEWPORT) callconv(.c) void)(self, num, viewports);
    }
    pub fn RSSetScissorRects(self: *@This(), num: u32, rects: [*]const RECT) void {
        vtCall(self.lpVtbl, 22, *const fn (*@This(), u32, [*]const RECT) callconv(.c) void)(self, num, rects);
    }
    pub fn SetPipelineState(self: *@This(), pso: *anyopaque) void {
        vtCall(self.lpVtbl, 25, *const fn (*@This(), *anyopaque) callconv(.c) void)(self, pso);
    }
    pub fn ResourceBarrier(self: *@This(), num: u32, barriers: [*]const D3D12_RESOURCE_BARRIER) void {
        vtCall(self.lpVtbl, 26, *const fn (*@This(), u32, [*]const D3D12_RESOURCE_BARRIER) callconv(.c) void)(self, num, barriers);
    }
    pub fn SetDescriptorHeaps(self: *@This(), num: u32, heaps: [*]const *anyopaque) void {
        vtCall(self.lpVtbl, 28, *const fn (*@This(), u32, [*]const *anyopaque) callconv(.c) void)(self, num, heaps);
    }
    pub fn SetGraphicsRootSignature(self: *@This(), sig: *anyopaque) void {
        vtCall(self.lpVtbl, 30, *const fn (*@This(), *anyopaque) callconv(.c) void)(self, sig);
    }
    pub fn SetGraphicsRootDescriptorTable(self: *@This(), index: u32, base_descriptor: D3D12_GPU_DESCRIPTOR_HANDLE) void {
        vtCall(self.lpVtbl, 32, *const fn (*@This(), u32, D3D12_GPU_DESCRIPTOR_HANDLE) callconv(.c) void)(self, index, base_descriptor);
    }
    pub fn SetGraphicsRoot32BitConstants(self: *@This(), index: u32, num: u32, data: ?*const anyopaque, offset: u32) void {
        vtCall(self.lpVtbl, 36, *const fn (*@This(), u32, u32, ?*const anyopaque, u32) callconv(.c) void)(self, index, num, data, offset);
    }
    pub fn IASetVertexBuffers(self: *@This(), start_slot: u32, num: u32, views: [*]const D3D12_VERTEX_BUFFER_VIEW) void {
        vtCall(self.lpVtbl, 44, *const fn (*@This(), u32, u32, [*]const D3D12_VERTEX_BUFFER_VIEW) callconv(.c) void)(self, start_slot, num, views);
    }
    pub fn OMSetRenderTargets(self: *@This(), num_rtvs: u32, rtv_handles: ?[*]const D3D12_CPU_DESCRIPTOR_HANDLE, single_handle: BOOL, dsv: ?*const D3D12_CPU_DESCRIPTOR_HANDLE) void {
        vtCall(self.lpVtbl, 46, *const fn (*@This(), u32, ?[*]const D3D12_CPU_DESCRIPTOR_HANDLE, BOOL, ?*const D3D12_CPU_DESCRIPTOR_HANDLE) callconv(.c) void)(self, num_rtvs, rtv_handles, single_handle, dsv);
    }
    pub fn ClearRenderTargetView(self: *@This(), handle: D3D12_CPU_DESCRIPTOR_HANDLE, color: *const [4]f32, num_rects: u32, rects: ?[*]const RECT) void {
        vtCall(self.lpVtbl, 48, *const fn (*@This(), D3D12_CPU_DESCRIPTOR_HANDLE, *const [4]f32, u32, ?[*]const RECT) callconv(.c) void)(self, handle, color, num_rects, rects);
    }
};

/// ID3D12Fence
pub const ID3D12Fence = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *@This()) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*@This()) callconv(.c) u32)(self);
    }
    pub fn GetCompletedValue(self: *@This()) u64 {
        return vtCall(self.lpVtbl, 8, *const fn (*@This()) callconv(.c) u64)(self);
    }
    pub fn SetEventOnCompletion(self: *@This(), value: u64, event: HANDLE) HRESULT {
        return vtCall(self.lpVtbl, 9, *const fn (*@This(), u64, HANDLE) callconv(.c) HRESULT)(self, value, event);
    }
};

/// ID3D12Resource
pub const ID3D12Resource = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *@This()) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*@This()) callconv(.c) u32)(self);
    }
    pub fn Map(self: *@This(), subresource: u32, read_range: ?*const anyopaque, out_ptr: *?*anyopaque) HRESULT {
        return vtCall(self.lpVtbl, 8, *const fn (*@This(), u32, ?*const anyopaque, *?*anyopaque) callconv(.c) HRESULT)(self, subresource, read_range, out_ptr);
    }
    pub fn Unmap(self: *@This(), subresource: u32, written_range: ?*const anyopaque) void {
        vtCall(self.lpVtbl, 9, *const fn (*@This(), u32, ?*const anyopaque) callconv(.c) void)(self, subresource, written_range);
    }
    pub fn GetGPUVirtualAddress(self: *@This()) u64 {
        return vtCall(self.lpVtbl, 11, *const fn (*@This()) callconv(.c) u64)(self);
    }
};

/// ID3D12DescriptorHeap
pub const ID3D12DescriptorHeap = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *@This()) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*@This()) callconv(.c) u32)(self);
    }
    /// Returns D3D12_CPU_DESCRIPTOR_HANDLE (8 bytes, fits in RAX on x64)
    pub fn GetCPUDescriptorHandleForHeapStart(self: *@This()) D3D12_CPU_DESCRIPTOR_HANDLE {
        return vtCall(self.lpVtbl, 9, *const fn (*@This()) callconv(.c) D3D12_CPU_DESCRIPTOR_HANDLE)(self);
    }
    pub fn GetGPUDescriptorHandleForHeapStart(self: *@This()) D3D12_GPU_DESCRIPTOR_HANDLE {
        return vtCall(self.lpVtbl, 10, *const fn (*@This()) callconv(.c) D3D12_GPU_DESCRIPTOR_HANDLE)(self);
    }
};

/// Generic Release-only COM wrapper (for RootSignature, PipelineState, etc.)
pub const IUnknown = extern struct {
    lpVtbl: ?*anyopaque,

    pub fn Release(self: *@This()) u32 {
        return vtCall(self.lpVtbl, 2, *const fn (*@This()) callconv(.c) u32)(self);
    }
};

// ================================================================
// Free functions (resolved by linker from d3d12.lib etc.)
// ================================================================

pub extern fn D3D12CreateDevice(
    pAdapter: ?*anyopaque,
    MinimumFeatureLevel: D3D_FEATURE_LEVEL,
    riid: *const GUID,
    ppDevice: *?*anyopaque,
) callconv(.c) HRESULT;

pub extern fn D3D12SerializeRootSignature(
    pRootSignature: *const D3D12_ROOT_SIGNATURE_DESC,
    Version: D3D_ROOT_SIGNATURE_VERSION,
    ppBlob: *?*ID3DBlob,
    ppErrorBlob: *?*ID3DBlob,
) callconv(.c) HRESULT;

pub extern fn D3DCompile(
    pSrcData: [*]const u8,
    SrcDataSize: usize,
    pSourceName: ?[*:0]const u8,
    pDefines: ?*anyopaque,
    pInclude: ?*anyopaque,
    pEntrypoint: [*:0]const u8,
    pTarget: [*:0]const u8,
    Flags1: u32,
    Flags2: u32,
    ppCode: *?*ID3DBlob,
    ppErrorMsgs: *?*ID3DBlob,
) callconv(.c) HRESULT;

// ================================================================
// Windows kernel32 / GDI32 functions
// ================================================================

pub extern "kernel32" fn CreateEventW(
    lpEventAttributes: ?*anyopaque,
    bManualReset: BOOL,
    bInitialState: BOOL,
    lpName: ?LPCWSTR,
) callconv(.c) ?HANDLE;

pub extern "kernel32" fn WaitForSingleObject(
    hHandle: HANDLE,
    dwMilliseconds: u32,
) callconv(.c) u32;

pub extern "kernel32" fn CloseHandle(
    hObject: HANDLE,
) callconv(.c) BOOL;

// GDI32 for glyph rasterization
pub const HDC = ?*anyopaque;
pub const HBITMAP = ?*anyopaque;
pub const HFONT = ?*anyopaque;
pub const HGDIOBJ = ?*anyopaque;

pub const BITMAPINFOHEADER = extern struct {
    biSize: u32 = 40,
    biWidth: i32 = 0,
    biHeight: i32 = 0,
    biPlanes: u16 = 1,
    biBitCount: u16 = 32,
    biCompression: u32 = 0, // BI_RGB
    biSizeImage: u32 = 0,
    biXPelsPerMeter: i32 = 0,
    biYPelsPerMeter: i32 = 0,
    biClrUsed: u32 = 0,
    biClrImportant: u32 = 0,
};

pub const BITMAPINFO = extern struct {
    bmiHeader: BITMAPINFOHEADER = .{},
    bmiColors: [1]u32 = .{0},
};

pub const SIZE = extern struct {
    cx: i32 = 0,
    cy: i32 = 0,
};

pub extern "gdi32" fn CreateCompatibleDC(hdc: HDC) callconv(.c) HDC;
pub extern "gdi32" fn DeleteDC(hdc: HDC) callconv(.c) BOOL;
pub extern "gdi32" fn CreateDIBSection(hdc: HDC, pbmi: *const BITMAPINFO, usage: u32, ppvBits: *?*anyopaque, hSection: ?HANDLE, offset: u32) callconv(.c) HBITMAP;
pub extern "gdi32" fn DeleteObject(ho: HGDIOBJ) callconv(.c) BOOL;
pub extern "gdi32" fn SelectObject(hdc: HDC, h: HGDIOBJ) callconv(.c) HGDIOBJ;
pub extern "gdi32" fn CreateFontW(
    cHeight: i32,
    cWidth: i32,
    cEscapement: i32,
    cOrientation: i32,
    cWeight: i32,
    bItalic: u32,
    bUnderline: u32,
    bStrikeOut: u32,
    iCharSet: u32,
    iOutPrecision: u32,
    iClipPrecision: u32,
    iQuality: u32,
    iPitchAndFamily: u32,
    pszFaceName: LPCWSTR,
) callconv(.c) HFONT;
pub extern "gdi32" fn SetBkMode(hdc: HDC, mode: i32) callconv(.c) i32;
pub extern "gdi32" fn SetTextColor(hdc: HDC, color: u32) callconv(.c) u32;
pub extern "gdi32" fn SetBkColor(hdc: HDC, color: u32) callconv(.c) u32;
pub extern "gdi32" fn GetTextExtentPoint32W(hdc: HDC, lpString: [*]const u16, c: i32, psizl: *SIZE) callconv(.c) BOOL;
pub extern "gdi32" fn TextOutW(hdc: HDC, x: i32, y: i32, lpString: [*]const u16, c: i32) callconv(.c) BOOL;

// ================================================================
// IID constants
// ================================================================

pub const IID_ID3D12Device = GUID{ .Data1 = 0x189819f1, .Data2 = 0x1db6, .Data3 = 0x4b57, .Data4 = .{ 0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7 } };
pub const IID_ID3D12CommandQueue = GUID{ .Data1 = 0x0ec870a6, .Data2 = 0x5d7e, .Data3 = 0x4c22, .Data4 = .{ 0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed } };
pub const IID_ID3D12CommandAllocator = GUID{ .Data1 = 0x6102dee4, .Data2 = 0xaf59, .Data3 = 0x4b09, .Data4 = .{ 0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24 } };
pub const IID_ID3D12GraphicsCommandList = GUID{ .Data1 = 0x5b160d0f, .Data2 = 0xac1b, .Data3 = 0x4185, .Data4 = .{ 0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55 } };
pub const IID_ID3D12Fence = GUID{ .Data1 = 0x0a753dcf, .Data2 = 0xc4d8, .Data3 = 0x4b91, .Data4 = .{ 0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76 } };
pub const IID_ID3D12DescriptorHeap = GUID{ .Data1 = 0x8efb471d, .Data2 = 0x616c, .Data3 = 0x4f49, .Data4 = .{ 0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51 } };
pub const IID_ID3D12RootSignature = GUID{ .Data1 = 0xc54a6b66, .Data2 = 0x72df, .Data3 = 0x4ee8, .Data4 = .{ 0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14 } };
pub const IID_ID3D12PipelineState = GUID{ .Data1 = 0x765a30f3, .Data2 = 0xf624, .Data3 = 0x4c6f, .Data4 = .{ 0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45 } };
pub const IID_ID3D12Resource = GUID{ .Data1 = 0x696442be, .Data2 = 0xa72e, .Data3 = 0x4059, .Data4 = .{ 0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad } };
