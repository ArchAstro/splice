//! Splice: a small, quiet video editor.

mod actions;
mod audio;
mod cli;
mod ctl;
mod editor;
mod ffmpeg;
mod icons;
mod library;
mod player;
mod session;
mod theme;
mod timeline;
mod titles;

use editor::Editor;
use gpui::{App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "ctl") {
        std::process::exit(ctl::cli(&args[1..]));
    }
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }
    let open = args.iter().find(|a| !a.starts_with('-')).map(PathBuf::from);

    titles::warm();
    Application::new().with_assets(icons::Icons).run(move |cx: &mut App| {
        actions::bind(cx);
        cx.set_menus(actions::menus());
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.on_action(|_: &actions::HideApp, cx| cx.hide());
        cx.on_action(|_: &actions::HideOthers, cx| cx.hide_other_apps());

        let bounds = Bounds::centered(None, size(px(1360.0), px(860.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Like Stet: no system title bar; the traffic lights sit on Splice's own.
                    titlebar: Some(TitlebarOptions {
                        title: Some("Splice".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.0), px(13.0))),
                    }),
                    window_min_size: Some(size(px(900.0), px(600.0))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Editor::new(open, window, cx)),
            )
            .expect("open the window");

        let editor = window.update(cx, |_, _, cx| cx.entity()).expect("the editor");
        window
            .update(cx, |_, window, cx| {
                let editor = editor.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    editor.update(cx, |editor, _| editor.flush());
                    true
                });
            })
            .ok();
        cx.on_app_quit(move |cx| {
            editor.update(cx, |editor, _| editor.flush());
            ctl::stop_listening();
            async {}
        })
        .detach();
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
