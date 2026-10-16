mod api;
mod attention;
mod backend;
mod catalog;
mod connection;
mod declarative;
mod graphics;
mod launch;
mod notify;
mod packages;
mod preferences;
mod recent;
mod sessions;
mod settings;
mod shortcuts;
mod tabs;
mod terminal;
mod tool_display;
mod transcript;
mod ui;

use gpui_kit::*;

use launch::Launch;
use ui::Launcher;

const APP_ID: &str = "dev.proteus.native";

fn main() {
    graphics::prepare_environment();
    let launch = match Launch::from_args() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("proteus-native: {error:#}");
            eprintln!(
                "usage: proteus-native [WORKSPACE | --choose-workspace] [--config PROFILE | --origin URL --token TOKEN]"
            );
            std::process::exit(2);
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ui::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            // The top bar draws the window controls and moves the window itself.
            let base = gpui_kit::component::TitleBar::window_options();
            let window_options = WindowOptions {
                app_id: Some(APP_ID.to_owned()),
                titlebar: base.titlebar.map(|titlebar| TitlebarOptions {
                    title: Some("Proteus".into()),
                    ..titlebar
                }),
                window_decorations: Some(window_decorations()),
                window_bounds: Some(WindowBounds::centered(size(px(1180.), px(820.)), cx)),
                // Scrolling does not focus the window on every compositor, and
                // the default ~30 fps cap for inactive windows makes glides
                // and animations under the pointer stutter.
                inactive_frame_interval: None,
                ..base
            };
            gpui_kit::open_window(window_options, cx, |window, cx| {
                cx.new(|cx| Launcher::new(launch.workspace, launch.target, window, cx))
            })
            .expect("open window");
            cx.activate(true);
        });
}

/// niri draws no window frame and sends no tiled state to a client-decorated
/// window, so the client frame there would only add a margin and a
/// full-window shadow that costs integrated graphics as much as the rest of
/// the interface on every frame.
fn window_decorations() -> WindowDecorations {
    let niri = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
        desktop
            .split(':')
            .any(|name| name.eq_ignore_ascii_case("niri"))
    });
    if niri {
        WindowDecorations::Server
    } else {
        WindowDecorations::Client
    }
}
