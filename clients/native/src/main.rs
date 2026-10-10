mod api;
mod backend;
mod connection;
mod graphics;
mod launch;
mod tool_display;
mod transcript;
mod ui;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

use launch::LaunchOptions;
use ui::Workspace;

const APP_ID: &str = "dev.proteus.native";

fn main() {
    graphics::prepare_environment();
    let options = match LaunchOptions::from_args() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("proteus-native: {error:#}");
            eprintln!(
                "usage: proteus-native [WORKSPACE] [--config PROFILE | --origin URL --token TOKEN]"
            );
            std::process::exit(2);
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let window_options = WindowOptions {
                app_id: Some(APP_ID.to_owned()),
                titlebar: Some(TitlebarOptions {
                    title: Some("Proteus".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::centered(size(px(1180.), px(820.)), cx)),
                ..Default::default()
            };
            gpui_kit::open_window(window_options, cx, |window, cx| {
                cx.new(|cx| Workspace::new(options, window, cx))
            })
            .expect("open window");
            cx.activate(true);
        });
}
