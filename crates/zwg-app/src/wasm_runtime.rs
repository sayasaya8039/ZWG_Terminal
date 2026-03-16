//! Embedded WASM runtime bootstrap.
//!
//! The module is intentionally sandboxed: it exposes no host imports, touches no
//! files, and has no network access. It acts as a stable capability probe so the
//! app always ships with a verified WASM execution path alongside Zig + DX12.

use anyhow::{Context, Result, bail};
use wasmi::{Engine, Instance, Linker, Module, Store, TypedFunc};

pub const WASM_ABI_VERSION: i32 = 1;
pub const CAPABILITY_ZIG_FFI: i32 = 0b001;
pub const CAPABILITY_DX12_RENDERER: i32 = 0b010;
pub const CAPABILITY_GPUI_HOST: i32 = 0b100;
pub const REQUIRED_CAPABILITIES: i32 =
    CAPABILITY_ZIG_FFI | CAPABILITY_DX12_RENDERER | CAPABILITY_GPUI_HOST;

const EMBEDDED_RUNTIME_WAT: &str = r#"
(module
  (func (export "zwg_abi_version") (result i32)
    i32.const 1)
  (func (export "zwg_capabilities") (result i32)
    i32.const 7)
)
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmRuntimeStatus {
    pub abi_version: i32,
    pub capabilities: i32,
}

impl WasmRuntimeStatus {
    pub fn has_capability(&self, capability: i32) -> bool {
        self.capabilities & capability == capability
    }

    pub fn capability_summary(&self) -> &'static str {
        "zig-ffi, dx12-renderer, gpui-host"
    }
}

pub fn initialize() -> Result<WasmRuntimeStatus> {
    let engine = Engine::default();
    let module = Module::new(&engine, EMBEDDED_RUNTIME_WAT)
        .context("failed to compile embedded WAT runtime")?;
    let mut store = Store::new(&engine, ());
    let linker = Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .context("failed to instantiate embedded WASM runtime")?;

    runtime_status(&mut store, &instance)
}

fn runtime_status(store: &mut Store<()>, instance: &Instance) -> Result<WasmRuntimeStatus> {
    let abi = typed_func::<(), i32>(store, instance, "zwg_abi_version")?
        .call(&mut *store, ())
        .context("failed to call zwg_abi_version")?;
    let capabilities = typed_func::<(), i32>(store, instance, "zwg_capabilities")?
        .call(&mut *store, ())
        .context("failed to call zwg_capabilities")?;

    if abi != WASM_ABI_VERSION {
        bail!(
            "unsupported embedded WASM ABI: expected {}, got {}",
            WASM_ABI_VERSION,
            abi
        );
    }
    if capabilities & REQUIRED_CAPABILITIES != REQUIRED_CAPABILITIES {
        bail!(
            "embedded WASM runtime missing required capabilities: expected 0x{:X}, got 0x{:X}",
            REQUIRED_CAPABILITIES,
            capabilities
        );
    }

    Ok(WasmRuntimeStatus {
        abi_version: abi,
        capabilities,
    })
}

fn typed_func<Params, Results>(
    store: &Store<()>,
    instance: &Instance,
    name: &str,
) -> Result<TypedFunc<Params, Results>>
where
    Params: wasmi::WasmParams,
    Results: wasmi::WasmResults,
{
    instance
        .get_typed_func(store, name)
        .with_context(|| format!("missing WASM export: {name}"))
}

#[cfg(test)]
mod tests {
    use super::{
        CAPABILITY_DX12_RENDERER, CAPABILITY_GPUI_HOST, CAPABILITY_ZIG_FFI, WASM_ABI_VERSION,
        initialize,
    };

    #[test]
    fn embedded_runtime_reports_required_capabilities() {
        let status = initialize().expect("embedded runtime should initialize");
        assert_eq!(status.abi_version, WASM_ABI_VERSION);
        assert!(status.has_capability(CAPABILITY_ZIG_FFI));
        assert!(status.has_capability(CAPABILITY_DX12_RENDERER));
        assert!(status.has_capability(CAPABILITY_GPUI_HOST));
    }

    #[test]
    fn embedded_runtime_summary_is_stable() {
        let status = initialize().expect("embedded runtime should initialize");
        assert_eq!(
            status.capability_summary(),
            "zig-ffi, dx12-renderer, gpui-host"
        );
    }
}
