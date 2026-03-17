//! ZWG Terminal — Ghostty-powered Windows terminal emulator
//! Built with Zig + GPUI + ConPTY

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Result;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod ai;
mod app;
mod config;
mod shell;
mod split;
mod terminal;
mod wasm_runtime;

use gpui::*;

struct Assets {
    base: PathBuf,
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let normalized_path = normalize_asset_path(path);
        let absolute_path = self.base.join(&*normalized_path);
        match fs::read(&absolute_path) {
            Ok(data) => Ok(Some(Cow::Owned(data))),
            Err(err) if err.kind() == ErrorKind::NotFound => {
                if let Some(bytes) = embedded_ui_asset(&normalized_path) {
                    log::warn!(
                        "Asset missing on disk: {} ; fallback to embedded asset",
                        normalized_path
                    );
                    return Ok(Some(Cow::Borrowed(bytes)));
                }

                log::warn!("Asset missing: {}", absolute_path.display());
                Ok(None)
            }
            Err(err) => Err(err.into()),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let normalized_path = normalize_asset_path(path);
        let absolute_path = self.base.join(&*normalized_path);
        match fs::read_dir(&absolute_path) {
            Ok(entries) => Ok(entries
                .filter_map(|entry| {
                    entry
                        .ok()
                        .and_then(|entry| entry.file_name().into_string().ok())
                        .map(SharedString::from)
                })
                .collect()),
            Err(err) if err.kind() == ErrorKind::NotFound => {
                if normalized_path == "ui" {
                    return Ok(EMBEDDED_UI_ASSETS
                        .iter()
                        .map(|(name, _)| SharedString::from(*name))
                        .collect());
                }
                log::warn!("Asset directory missing: {}", absolute_path.display());
                Ok(Vec::new())
            }
            Err(err) => Err(err.into()),
        }
    }
}

fn normalize_asset_path(path: &str) -> Cow<'_, str> {
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

static EMBEDDED_UI_ASSETS: &[(&str, &[u8])] = &[
    (
        "ui/chevron-down.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/chevron-down.svg"
        )),
    ),
    (
        "ui/copy.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/copy.svg"
        )),
    ),
    (
        "ui/edit.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/edit.svg"
        )),
    ),
    (
        "ui/plus.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/plus.svg"
        )),
    ),
    (
        "ui/search.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/search.svg"
        )),
    ),
    (
        "ui/settings-advanced.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/settings-advanced.svg"
        )),
    ),
    (
        "ui/settings-appearance.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/settings-appearance.svg"
        )),
    ),
    (
        "ui/settings-general.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/settings-general.svg"
        )),
    ),
    (
        "ui/settings-key.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/settings-key.svg"
        )),
    ),
    (
        "ui/settings-notifications.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/settings-notifications.svg"
        )),
    ),
    (
        "ui/settings-privacy.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/settings-privacy.svg"
        )),
    ),
    (
        "ui/settings-terminal.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/settings-terminal.svg"
        )),
    ),
    (
        "ui/settings.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/settings.svg"
        )),
    ),
    (
        "ui/snippet-palette.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/snippet-palette.svg"
        )),
    ),
    (
        "ui/star-filled.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/star-filled.svg"
        )),
    ),
    (
        "ui/star.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/star.svg"
        )),
    ),
    (
        "ui/trash.svg",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/ui/trash.svg"
        )),
    ),
];

fn embedded_ui_asset(path: &str) -> Option<&'static [u8]> {
    EMBEDDED_UI_ASSETS
        .iter()
        .find_map(|(name, data)| (*name == path).then_some(*data))
}

fn collect_resource_candidates(anchor: &Path, max_depth: usize, out: &mut Vec<PathBuf>) {
    let mut current = Some(anchor);
    for _ in 0..=max_depth {
        let Some(dir) = current else {
            break;
        };
        out.push(dir.join("resources"));
        current = dir.parent();
    }
}

fn push_unique(candidates: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, candidate: PathBuf) {
    if seen.insert(candidate.clone()) {
        candidates.push(candidate);
    }
}

fn resource_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    let max_depth = 6;

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let mut discovered = Vec::new();
            collect_resource_candidates(exe_dir, max_depth, &mut discovered);
            for candidate in discovered {
                push_unique(&mut candidates, &mut seen, candidate);
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut discovered = Vec::new();
        collect_resource_candidates(&cwd, max_depth, &mut discovered);
        for candidate in discovered {
            push_unique(&mut candidates, &mut seen, candidate);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in manifest_dir
        .ancestors()
        .take(max_depth + 1)
        .map(|ancestor| ancestor.join("resources"))
    {
        push_unique(&mut candidates, &mut seen, candidate);
    }

    candidates
}

fn is_resource_root(path: &Path) -> bool {
    path.join("ui").is_dir() && path.join("icons").is_dir()
}

fn locate_resources_path() -> PathBuf {
    let candidates = resource_dir_candidates();
    for candidate in &candidates {
        log::debug!("Checking resource candidate: {}", candidate.display());
    }

    for candidate in candidates {
        if candidate.is_dir() && is_resource_root(&candidate) {
            log::info!("Using resources from {}", candidate.display());
            return candidate;
        }
    }

    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources");
    log::warn!(
        "resources path not found, fallback to build-time path: {}",
        fallback.display()
    );
    fallback
}

actions!(
    zwg,
    [
        Quit,
        NewTab,
        CloseTab,
        SplitRight,
        SplitDown,
        ClosePane,
        FocusNext,
        FocusPrev,
        OpenSettings,
    ]
);

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("zwg=info,gpui=warn"),
    )
    .init();

    if std::env::var("ZWG_IME_TRACE")
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        })
        .unwrap_or(false)
    {
        log::info!("ZWG_IME_TRACE=1 enabled");
    }

    log::info!("ZWG Terminal v{} starting", env!("CARGO_PKG_VERSION"));
    let wasm_runtime = wasm_runtime::initialize().expect("embedded WASM runtime init failed");
    log::info!(
        "WASM runtime ready: abi={} capabilities=0x{:X} ({})",
        wasm_runtime.abi_version,
        wasm_runtime.capabilities,
        wasm_runtime.capability_summary()
    );
    let resources_path = locate_resources_path();

    let app = Application::new().with_assets(Assets {
        base: resources_path,
    });
    app.run(|cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // Load saved window state (position + size)
        let window_state = config::WindowState::load();

        let app_state = app::AppState::new(cx);
        let state = cx.new(|_cx| app_state);

        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point {
                    x: px(window_state.x),
                    y: px(window_state.y),
                },
                size: Size {
                    width: px(window_state.width),
                    height: px(window_state.height),
                },
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("ZWG Terminal".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        cx.open_window(opts, |window, cx| {
            let root = cx.new(|cx| app::RootView::new(state.clone(), cx));
            // Auto-focus terminal so the user can type immediately at startup
            root.update(cx, |view, cx| {
                view.focus_active_terminal(window, cx);
            });
            root
        })
        .ok();
    });
}
