//! Splice: a small, quiet video editor.

mod actions;
mod audio;
mod cli;
mod ctl;
mod editor;
mod ffmpeg;
mod icons;
mod library;
mod music;
mod player;
mod pointer;
mod recorder;
mod scenes;
mod session;
mod symbols;
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
    // Files handed over by Finder (a double-clicked project, media dropped on
    // the Dock icon) arrive here, before or after the window opens.
    let (opened, mut to_open) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
    let app = Application::new().with_assets(icons::Icons);
    app.on_open_urls(move |urls| {
        let paths: Vec<PathBuf> = urls.iter().filter_map(|u| file_path(u)).collect();
        if !paths.is_empty() {
            let _ = opened.unbounded_send(paths);
        }
    });
    app.run(move |cx: &mut App| {
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
        let receiver = editor.clone();
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(paths) = to_open.next().await {
                if receiver.update(cx, |editor, cx| editor.open_files(paths, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
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

/// `file:///Users/me/My%20Movie.splice` → its path.
fn file_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_file_urls() {
        assert_eq!(
            super::file_path("file:///Users/me/My%20Movie.splice"),
            Some("/Users/me/My Movie.splice".into())
        );
        assert_eq!(super::file_path("https://example.com"), None);
    }
}
