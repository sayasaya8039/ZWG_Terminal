//! ZWG Terminal — Ghostty-powered Windows terminal emulator
//! Built with Zig + GPUI + ConPTY

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app;
mod shell;
mod terminal;

use gpui::*;

actions!(zwg, [Quit, NewTab, CloseTab, SplitRight, SplitDown]);

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("zwg=info,gpui=warn"),
    )
    .init();

    log::info!("ZWG Terminal v{} starting", env!("CARGO_PKG_VERSION"));

    let app = Application::new();
    app.run(|cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());

        let app_state = app::AppState::new(cx);
        let state = cx.new(|_cx| app_state);

        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: Size {
                    width: px(1400.0),
                    height: px(900.0),
                },
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("ZWG Terminal".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        cx.open_window(opts, |_window, cx| {
            let view = cx.new(|cx| app::RootView::new(state.clone(), cx));
            view
        })
        .ok();
    });
}
