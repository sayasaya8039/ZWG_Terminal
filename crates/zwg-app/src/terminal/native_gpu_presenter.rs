#[cfg(target_os = "windows")]
use anyhow::{Context, Result};
#[cfg(target_os = "windows")]
use gpui::{Bounds, Pixels};
#[cfg(target_os = "windows")]
use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        Graphics::{
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
            SetWindowPos, ShowWindow, WS_CHILD, WS_CLIPSIBLINGS, WS_DISABLED, WS_VISIBLE,
        },
    },
    core::{Interface, w},
};

#[cfg(target_os = "windows")]
const BUFFER_COUNT: usize = 2;

#[cfg(target_os = "windows")]
pub(crate) struct NativeGpuPresenter {
    hwnd: HWND,
    device: ID3D12Device,
    swap_chain: IDXGISwapChain3,
    rtv_heap: ID3D12DescriptorHeap,
    back_buffers: [Option<ID3D12Resource>; BUFFER_COUNT],
    rtv_stride: usize,
    width: u32,
    height: u32,
}

#[cfg(target_os = "windows")]
unsafe impl Send for NativeGpuPresenter {}

#[cfg(target_os = "windows")]
impl NativeGpuPresenter {
    pub(crate) fn new(
        parent_hwnd: HWND,
        bounds: Bounds<Pixels>,
        device_ptr: *mut core::ffi::c_void,
        queue_ptr: *mut core::ffi::c_void,
    ) -> Result<Self> {
        let hwnd = create_child_window(parent_hwnd, bounds)?;
        let device =
            clone_interface::<ID3D12Device>(device_ptr).context("borrowing D3D12 device")?;
        let command_queue = clone_interface::<ID3D12CommandQueue>(queue_ptr)
            .context("borrowing D3D12 command queue")?;
        let swap_chain = create_swap_chain(&command_queue, hwnd, bounds)?;
        let rtv_heap = create_rtv_heap(&device)?;
        let rtv_stride = unsafe {
            device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize
        };
        let back_buffers = create_back_buffers(&device, &swap_chain, &rtv_heap, rtv_stride)?;

        Ok(Self {
            hwnd,
            device,
            swap_chain,
            rtv_heap,
            back_buffers,
            rtv_stride,
            width: pixels_to_u32(bounds.size.width),
            height: pixels_to_u32(bounds.size.height),
        })
    }

    pub(crate) fn sync_bounds(&mut self, parent_hwnd: HWND, bounds: Bounds<Pixels>) -> Result<()> {
        let width = pixels_to_u32(bounds.size.width);
        let height = pixels_to_u32(bounds.size.height);
        let rect = bounds_to_rect(bounds);
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        }
        .ok()
        .context("moving native GPU child window")?;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        };
        if width != self.width || height != self.height {
            self.resize_swap_chain(parent_hwnd, width, height)?;
        }
        Ok(())
    }

    pub(crate) fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        };
    }

    pub(crate) fn current_back_buffer_ptr(&self) -> Option<*mut core::ffi::c_void> {
        let back_index = unsafe { self.swap_chain.GetCurrentBackBufferIndex() as usize };
        self.back_buffers[back_index]
            .as_ref()
            .map(|buffer| buffer.as_raw() as *mut core::ffi::c_void)
    }

    pub(crate) fn present(&mut self) -> Result<()> {
        unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)) }
            .ok()
            .context("presenting native GPU swapchain")?;
        Ok(())
    }

    fn resize_swap_chain(&mut self, parent_hwnd: HWND, width: u32, height: u32) -> Result<()> {
        let _ = parent_hwnd;
        self.back_buffers.fill(None);
        unsafe {
            self.swap_chain.ResizeBuffers(
                BUFFER_COUNT as u32,
                width,
                height,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )
        }
        .context("resizing native GPU swapchain")?;
        self.back_buffers = create_back_buffers(
            &self.device,
            &self.swap_chain,
            &self.rtv_heap,
            self.rtv_stride,
        )?;
        self.width = width;
        self.height = height;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl Drop for NativeGpuPresenter {
    fn drop(&mut self) {
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn clone_interface<I: Interface>(ptr: *mut core::ffi::c_void) -> Option<I> {
    unsafe { I::from_raw_borrowed(&ptr).map(|iface| iface.clone()) }
}

#[cfg(target_os = "windows")]
fn create_child_window(parent_hwnd: HWND, bounds: Bounds<Pixels>) -> Result<HWND> {
    let rect = bounds_to_rect(bounds);
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            w!("STATIC"),
            None,
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_DISABLED,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            Some(parent_hwnd),
            None,
            None,
            None,
        )
    }
    .context("creating native GPU child window")?;
    Ok(hwnd)
}

#[cfg(target_os = "windows")]
fn create_swap_chain(
    command_queue: &ID3D12CommandQueue,
    hwnd: HWND,
    bounds: Bounds<Pixels>,
) -> Result<IDXGISwapChain3> {
    let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }
        .context("creating DXGI factory for native presenter")?;
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: pixels_to_u32(bounds.size.width),
        Height: pixels_to_u32(bounds.size.height),
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: BUFFER_COUNT as u32,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: 0,
    };
    let swap_chain =
        unsafe { factory.CreateSwapChainForHwnd(command_queue, hwnd, &desc, None, None) }
            .context("creating native presenter swapchain")?;
    unsafe { factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER) }.ok();
    swap_chain
        .cast()
        .context("casting native presenter swapchain")
}

#[cfg(target_os = "windows")]
fn create_rtv_heap(device: &ID3D12Device) -> Result<ID3D12DescriptorHeap> {
    let desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
        NumDescriptors: BUFFER_COUNT as u32,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
        NodeMask: 0,
    };
    unsafe { device.CreateDescriptorHeap(&desc) }.context("creating native presenter RTV heap")
}

#[cfg(target_os = "windows")]
fn create_back_buffers(
    device: &ID3D12Device,
    swap_chain: &IDXGISwapChain3,
    rtv_heap: &ID3D12DescriptorHeap,
    rtv_stride: usize,
) -> Result<[Option<ID3D12Resource>; BUFFER_COUNT]> {
    let mut buffers: [Option<ID3D12Resource>; BUFFER_COUNT] = Default::default();
    let base = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
    for (index, slot) in buffers.iter_mut().enumerate() {
        let buffer: ID3D12Resource = unsafe { swap_chain.GetBuffer(index as u32) }
            .context("fetching swapchain back buffer")?;
        let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: base.ptr + index * rtv_stride,
        };
        unsafe { device.CreateRenderTargetView(&buffer, None, handle) };
        *slot = Some(buffer);
    }
    Ok(buffers)
}

#[cfg(target_os = "windows")]
fn bounds_to_rect(bounds: Bounds<Pixels>) -> RECT {
    let left: f32 = bounds.origin.x.into();
    let top: f32 = bounds.origin.y.into();
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    RECT {
        left: left.floor() as i32,
        top: top.floor() as i32,
        right: (left + width).ceil() as i32,
        bottom: (top + height).ceil() as i32,
    }
}

#[cfg(target_os = "windows")]
fn pixels_to_u32(value: Pixels) -> u32 {
    let value: f32 = value.into();
    value.max(1.0).ceil() as u32
}
