//! ZWG Terminal — Ghostty-powered Windows terminal emulator
//! Built with Zig + GPUI + ConPTY

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app;
mod config;
mod shell;
mod split;
mod terminal;

use std::sync::{Arc, Mutex};

use gpui::*;

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
    ]
);

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("zwg=info,gpui=warn"),
    )
    .init();

    log::info!("ZWG Terminal v{} starting", env!("CARGO_PKG_VERSION"));

    let app = Application::new();
    app.run(|cx: &mut App| {
        // Global actions
        cx.on_action(|_: &Quit, cx| cx.quit());

        // Keybindings
        cx.bind_keys([
            KeyBinding::new("ctrl-shift-t", NewTab, None),
            KeyBinding::new("ctrl-shift-w", CloseTab, None),
            KeyBinding::new("ctrl-shift-d", SplitRight, None),
            KeyBinding::new("ctrl-shift-e", SplitDown, None),
            KeyBinding::new("ctrl-shift-x", ClosePane, None),
            KeyBinding::new("ctrl-tab", FocusNext, None),
            KeyBinding::new("ctrl-shift-tab", FocusPrev, None),
            KeyBinding::new("ctrl-shift-q", Quit, None),
        ]);

        // Load saved window state
        let window_state = config::WindowState::load();
        let last_bounds: Arc<Mutex<Option<config::WindowState>>> = Arc::new(Mutex::new(None));
        let bounds_for_quit = last_bounds.clone();

        // Save window state on app quit (covers X button, Ctrl+Shift+Q, etc.)
        // Must keep Subscription alive for the duration of the app
        let _quit_sub = cx.on_app_quit({
            move |_cx| {
                let bounds_ref = bounds_for_quit.clone();
                async move {
                    if let Some(state) = bounds_ref.lock().unwrap().take() {
                        if let Err(e) = state.save() {
                            log::warn!("Failed to save window state: {}", e);
                        }
                    }
                }
            }
        });

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
            cx.new(|cx| app::RootView::new(state.clone(), last_bounds.clone(), cx))
        })
        .ok();
    });
}
