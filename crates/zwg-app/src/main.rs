//! ZWG Terminal — Ghostty-powered Windows terminal emulator
//! Built with Zig + GPUI + ConPTY

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app;
mod config;
mod shell;
mod snippets;
mod split;
mod terminal;

use gpui::*;

struct Assets {
    base: PathBuf,
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(Cow::Owned(data)))
            .map_err(|err| err.into())
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(|err| err.into())
    }
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
        ToggleSnippetPalette,
    ]
);

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("zwg=info,gpui=warn"),
    )
    .init();

    log::info!("ZWG Terminal v{} starting", env!("CARGO_PKG_VERSION"));

    let app = Application::new().with_assets(Assets {
        base: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources"),
    });
    app.run(|cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // Keybindings
        cx.bind_keys([
            KeyBinding::new("ctrl-shift-t", NewTab, None),
            KeyBinding::new("ctrl-shift-w", CloseTab, None),
            KeyBinding::new("ctrl-shift-d", SplitRight, None),
            KeyBinding::new("ctrl-shift-e", SplitDown, None),
            KeyBinding::new("ctrl-shift-x", ClosePane, None),
            KeyBinding::new("ctrl-tab", FocusNext, None),
            KeyBinding::new("ctrl-shift-tab", FocusPrev, None),
            KeyBinding::new("ctrl-shift-v", ToggleSnippetPalette, None),
            KeyBinding::new("ctrl-shift-q", Quit, None),
        ]);

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
                ..Default::default()
            }),
            ..Default::default()
        };

        cx.open_window(opts, |_window, cx| {
            cx.new(|cx| app::RootView::new(state.clone(), cx))
        })
        .ok();
    });
}
