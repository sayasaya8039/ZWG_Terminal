use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceKind {
    Npu,
    Gpu,
    Cpu,
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Npu => write!(f, "NPU"),
            Self::Gpu => write!(f, "GPU"),
            Self::Cpu => write!(f, "CPU"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceleratorDevice {
    pub kind: DeviceKind,
    pub device_id: u32,
    pub name: String,
}

impl fmt::Display for AcceleratorDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} #{}: {}", self.kind, self.device_id, self.name)
    }
}

/// Discover available DirectML devices and return them in priority order:
/// NPU > GPU > CPU fallback.
///
/// Uses DXGI adapter enumeration via the `windows` crate to detect
/// GPU/NPU hardware. CPU is always appended as the final fallback.
pub fn discover_devices() -> Result<Vec<AcceleratorDevice>> {
    let mut devices = Vec::new();

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1,
            DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE,
        };

        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let mut adapter_index: u32 = 0;

        loop {
            let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Ok(a) => a,
                Err(_) => break,
            };

            let desc: DXGI_ADAPTER_DESC1 = unsafe { adapter.GetDesc1()? };
            let flags = DXGI_ADAPTER_FLAG(desc.Flags as i32);

            // Skip software adapters (e.g. WARP)
            if flags.contains(DXGI_ADAPTER_FLAG_SOFTWARE) {
                adapter_index += 1;
                continue;
            }

            let name = String::from_utf16_lossy(
                &desc.Description[..desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len())],
            );

            let kind = classify_device(&name);
            devices.push(AcceleratorDevice {
                kind,
                device_id: adapter_index,
                name,
            });

            adapter_index += 1;
        }
    }

    // Sort: NPU first, then GPU, then CPU
    devices.sort_by_key(|d| match d.kind {
        DeviceKind::Npu => 0,
        DeviceKind::Gpu => 1,
        DeviceKind::Cpu => 2,
    });

    // Always add CPU as final fallback
    devices.push(AcceleratorDevice {
        kind: DeviceKind::Cpu,
        device_id: 0,
        name: "CPU (fallback)".to_string(),
    });

    log::info!("Discovered {} accelerator device(s):", devices.len());
    for dev in &devices {
        log::info!("  {dev}");
    }

    Ok(devices)
}

/// Classify a DXGI adapter by its description string.
/// Covers known NPU branding from Intel, Qualcomm, and AMD.
fn classify_device(name: &str) -> DeviceKind {
    let lower = name.to_ascii_lowercase();
    const NPU_KEYWORDS: &[&str] = &[
        "npu",
        "neural",
        "myriad",
        "ai accelerator",
        "ai boost",
        "hexagon",
        "xdna",
    ];
    if NPU_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        DeviceKind::Npu
    } else {
        DeviceKind::Gpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_gpu_adapter() {
        assert_eq!(classify_device("NVIDIA GeForce RTX 4090"), DeviceKind::Gpu);
        assert_eq!(classify_device("AMD Radeon RX 7900 XTX"), DeviceKind::Gpu);
        assert_eq!(classify_device("Intel(R) Arc(TM) A770"), DeviceKind::Gpu);
    }

    #[test]
    fn classify_npu_adapter() {
        assert_eq!(classify_device("Intel(R) AI Boost NPU"), DeviceKind::Npu);
        assert_eq!(classify_device("Qualcomm Neural Processing Unit"), DeviceKind::Npu);
        assert_eq!(classify_device("Qualcomm(R) AI Accelerator"), DeviceKind::Npu);
        assert_eq!(classify_device("AMD XDNA Driver"), DeviceKind::Npu);
        assert_eq!(classify_device("Intel(R) AI Boost"), DeviceKind::Npu);
    }

    #[test]
    fn cpu_always_appended() {
        let devices = discover_devices().unwrap();
        assert!(devices.last().map_or(false, |d| d.kind == DeviceKind::Cpu));
    }
}
