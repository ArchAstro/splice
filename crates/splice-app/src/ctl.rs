//! `splice ctl`: working in the running window from another program, after
//! Stet's `stet ctl`. One JSON request per line over a local socket, one JSON
//! reply. The client half turns a command line into a request; the editor
//! answers it on the UI thread (`editor/agent.rs`).

use crate::session::dirs;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;

pub const USAGE: &str = r#"splice ctl <command>   work in the running Splice (replies are JSON)

Looking
  status                         the project, canvas, playhead, selection, export progress
  read                           everything: media, every track and clip, with ids and times
  frame [T] <file.png>           save the picture at time T (default: the playhead), as export draws it
  versions                       the versions kept in the project file

Media
  import <file>... [--at T] [--track N]
                                 bring files into the project; with --at, also place them end to end

Editing (each is one undo step; clips and media are named by id, media also by name)
  add --media M [--at T] [--track N]       put media on the timeline (default: the end of track N)
  move --clip C [--to T] [--track N]       move a clip in time or to another layer
  trim --clip C [--start T] [--end T]      move a clip's left or right edge to T
  split [--at T] [--clip C]                cut at T (default: the playhead) every clip under it, or C
  delete --clip C [--ripple]               remove clips; --ripple closes the gap
  duplicate --clip C                       copy clips to just after themselves
  volume --clip C --value V                1 is as recorded, 0 silent, up to 4
  track --track N [--mute|--unmute] [--hide|--show]
  canvas [--aspect 16:9|9:16|1:1|4:5] [--size WxH] [--fps N]
  undo | redo

Titles, colours and looks
  text --text WORDS [--at T] [--duration D] [--track N] [style] [look]
                                 put a title on the canvas (default: at the playhead, over what is there)
  color --color C [--at T] [--duration D] [--track N] [look]
                                 a full-frame colour: a background, a title card, a tint (#00000080)
  style --clip C [--text WORDS] [style]    change a title's words or style, or a colour clip's --color
  look --clip C [look]                     place, size, fade, animate, key or mask any picture
  fonts [--query WORDS]                    the font families installed here

  style:  --font NAME  --size PX  --color C  --box C|none  --align left|center|right
          --bold/--no-bold  --italic/--no-italic  --shadow/--no-shadow
  look:   --x F --y F (centre offset, fractions of the canvas)  --scale F (1 = natural)  --opacity F
          --enter KIND [--enter-for S]  --exit KIND [--exit-for S]
              KIND: none, fade, slide-up, slide-down, slide-left, slide-right, zoom, pop
          --drift none|zoom-in|zoom-out (a slow zoom across the clip)
          --key green|blue|#rrggbb|none [--similarity F] [--blend F] [--despill/--no-despill]
          --mask ellipse|rect|none [--mask-x F --mask-y F --mask-width F --mask-height F]
                [--feather F] [--invert/--no-invert]   (mask numbers are fractions of the clip's frame)
  C is #rrggbb, #rrggbbaa or a name: white, black, paper, ink, vermilion, gold, green, blue, clear.

Music (a score: instruments and notes, played by General MIDI sounds)
  score new [--file score.json] [--name N] [--at T] [--track N] [--wait]
                                 add a score to the media (the starter if no file); with --at, place it
  score read --media M           the score as JSON, and what it compiles to: length, notes per part
  score write --media M --file score.json [--wait]
                                 replace the score; it re-renders, and every clip of it follows
  score instruments              the instruments (General MIDI) and the drum kit's keys
  The format is Clapper's: {title, tempo, meter, tracks: [{id, instrument, clips: [{at, notes:
  [{at, duration, pitch, velocity}], repeat, length, transpose}], gain, pan, reverb, drums}]}.
  Times are in beats (quarter notes); pitches "C4" or MIDI numbers.

Scenes (moving pictures from React code, rendered by Clapper)
  scene new [--file scene.tsx] [--name N] [--duration D] [--at T] [--track N] [--opaque] [--wait]
                                 add a scene (a starter diagram if no file); see-through unless --opaque
  scene read --media M           its code, length, folder, and how its render went
  scene write --media M [--file scene.tsx] [--duration D] [--opaque|--transparent] [--wait]
                                 change it; it re-renders (a few seconds) and its clips follow
  scene studio --media M         open Clapper's studio on it in the browser (live preview)
  open --media M                 open a score or scene in Splice's own editor

Playing and the window
  seek T | play | pause
  select [--clip C]                        select clips (none: deselect)
  command <words>                          run a command from the command menu (zoom fit, versions)
  save [--label TEXT]                      keep a version
  export <file.mp4> [--draft|--high] [--wait]
  say --text TEXT                          show one line in Splice's status bar

Talking with the person
  wait [--name NAME]             connect as an assistant: blocks until they send a message from
                                 Splice (Cmd-Shift-A), prints it with what they had selected, and exits
  call                           send one JSON request read from stdin

  T is seconds (12.5), m:ss (1:02.5) or h:mm:ss. Tracks count from 1, the bottom layer;
  a track number one above the top makes a new layer. --clip takes ids separated by commas.
  A value of @FILE is read from FILE, and - from stdin."#;

/// Where a running Splice listens.
pub fn socket_path() -> PathBuf {
    dirs().data.join("splice.sock")
}

/// What the window receives: a request and where its reply goes, or word
/// that a caller hung up before it was answered.
pub enum Incoming {
    Request {
        id: u64,
        line: String,
        reply: Sender<String>,
    },
    Gone(u64),
}

/// Starts answering requests. `None` when another Splice already does.
pub fn listen() -> Option<UnboundedReceiver<Incoming>> {
    let socket = socket_path();
    if UnixStream::connect(&socket).is_ok() {
        return None;
    }
    // Whatever is there was left by a Splice that is gone.
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).ok()?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
    }
    let (tx, rx) = unbounded();
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().flatten().enumerate() {
            let id = n as u64 + 1;
            let tx = tx.clone();
            // One thread per caller: an assistant waiting for a message holds
            // its connection open without holding up anyone else.
            std::thread::spawn(move || {
                let (Ok(mut reply_to), Ok(probe)) = (stream.try_clone(), stream.try_clone()) else {
                    return;
                };
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_err() || line.trim().is_empty() {
                    return;
                }
                let (reply, answer) = channel();
                if tx.unbounded_send(Incoming::Request { id, line, reply }).is_err() {
                    return;
                }
                // The caller sends nothing more, so a read that ends means it hung up.
                let _ = probe.set_nonblocking(true);
                let alive = || !matches!((&probe).read(&mut [0u8; 1]), Ok(0));
                let reply = loop {
                    match answer.recv_timeout(Duration::from_millis(400)) {
                        Ok(reply) => break reply,
                        Err(RecvTimeoutError::Timeout) if alive() => {}
                        Err(_) => {
                            let _ = tx.unbounded_send(Incoming::Gone(id));
                            return;
                        }
                    }
                };
                let _ = reply_to.set_nonblocking(false);
                let _ = reply_to.write_all(reply.replace('\n', " ").as_bytes());
                let _ = reply_to.write_all(b"\n");
            });
        }
    });
    Some(rx)
}

/// Removes the socket on the way out.
pub fn stop_listening() {
    let _ = std::fs::remove_file(socket_path());
}

/// Sends one request to the running Splice; `None` if none is running.
/// `patient` waits as long as it takes (a message, an import, an export).
fn send(line: &str, patient: bool) -> Option<String> {
    let mut stream = UnixStream::connect(socket_path()).ok()?;
    let _ = stream.set_read_timeout((!patient).then(|| Duration::from_secs(15)));
    stream.write_all(line.replace('\n', " ").as_bytes()).ok()?;
    stream.write_all(b"\n").ok()?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).ok()?;
    (!reply.is_empty()).then_some(reply)
}

pub fn fail(message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": message.into() })
}

/// Seconds from `12.5`, `1:02.5` or `1:00:00`.
pub fn parse_time(text: &str) -> Option<f64> {
    let mut total = 0.0;
    for part in text.trim().split(':') {
        let value: f64 = part.parse().ok()?;
        if value < 0.0 {
            return None;
        }
        total = total * 60.0 + value;
    }
    Some(total)
}

fn value(raw: &str) -> Result<String, String> {
    if raw == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?;
        return Ok(text);
    }
    match raw.strip_prefix('@') {
        Some(file) => std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}")),
        None => Ok(raw.to_string()),
    }
}

fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or(path.to_string(), |p| p.to_string_lossy().into_owned())
}

fn time_value(flag: &str, raw: &str) -> Result<Value, String> {
    if raw == "end" {
        return Ok(json!("end"));
    }
    parse_time(raw)
        .map(|t| json!(t))
        .ok_or(format!("{flag} expects a time: seconds, m:ss or h:mm:ss"))
}

/// Turns `splice ctl` arguments into a request.
fn build(args: &[String]) -> Result<Value, String> {
    let (command, rest) = args.split_first().ok_or(String::new())?;
    let mut request = Map::new();
    let mut words = Vec::new();
    let mut clips: Vec<Value> = Vec::new();
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        let mut next = |name: &str| rest.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--font" | "--color" | "--box" | "--align" | "--enter" | "--exit" | "--drift" | "--key" | "--mask"
            | "--query" => {
                request.insert(arg[2..].into(), json!(value(next(arg)?)?));
            }
            "--size" | "--x" | "--y" | "--scale" | "--opacity" | "--similarity" | "--blend" | "--feather"
            | "--mask-x" | "--mask-y" | "--mask-width" | "--mask-height" | "--enter-for" | "--exit-for" => {
                let raw = next(arg)?;
                let v = match raw.parse::<f64>() {
                    Ok(v) => json!(v),
                    // `canvas --size 1080x1920` is a size, not a number.
                    Err(_) if arg == "--size" => json!(raw),
                    Err(_) => return Err(format!("{arg} expects a number")),
                };
                request.insert(arg[2..].replace('-', "_"), v);
            }
            "--bold" | "--italic" | "--shadow" | "--invert" | "--despill" => {
                request.insert(arg[2..].into(), json!(true));
            }
            "--no-bold" | "--no-italic" | "--no-shadow" | "--no-invert" | "--no-despill" => {
                request.insert(arg[5..].into(), json!(false));
            }
            "--at" | "--to" | "--start" | "--end" | "--duration" => {
                let raw = next(arg)?;
                request.insert(arg[2..].into(), time_value(arg, raw)?);
            }
            "--track" | "--fps" | "--version" => {
                let n: u64 = next(arg)?.parse().map_err(|_| format!("{arg} expects a number"))?;
                request.insert(arg[2..].into(), json!(n));
            }
            "--value" => {
                let v: f64 = next(arg)?.parse().map_err(|_| "--value expects a number".to_string())?;
                request.insert("value".into(), json!(v));
            }
            "--clip" => {
                for id in next(arg)?.split(',').filter(|s| !s.trim().is_empty()) {
                    clips.push(json!(id.trim().parse::<u64>().map_err(|_| "--clip expects clip ids")?));
                }
            }
            "--media" => {
                let m = next(arg)?;
                request.insert("media".into(), m.parse::<u64>().map_or(json!(m), |id| json!(id)));
            }
            "--text" | "--label" | "--name" | "--aspect" => {
                request.insert(arg[2..].into(), json!(value(next(arg)?)?));
            }
            "--file" => {
                let file = next(arg)?;
                request.insert("content".into(), json!(value(&format!("@{file}"))?));
            }
            "--opaque" | "--transparent" => {
                request.insert("transparent".into(), json!(arg == "--transparent"));
            }
            "--ripple" | "--wait" => {
                request.insert(arg[2..].into(), json!(true));
            }
            "--draft" | "--high" => {
                request.insert("quality".into(), json!(&arg[2..]));
            }
            "--mute" | "--unmute" => {
                request.insert("muted".into(), json!(arg == "--mute"));
            }
            "--hide" | "--show" => {
                request.insert("hidden".into(), json!(arg == "--hide"));
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            word => words.push(word.to_string()),
        }
    }
    if !clips.is_empty() {
        request.insert("clips".into(), json!(clips));
    }
    let cmd = match command.as_str() {
        "status" | "read" | "versions" | "add" | "move" | "trim" | "split" | "delete" | "duplicate" | "volume"
        | "track" | "canvas" | "undo" | "redo" | "play" | "pause" | "select" | "save" | "say" | "wait" | "text"
        | "color" | "style" | "look" | "fonts" => command.as_str(),
        "restore" => {
            if let Some(id) = words.first().and_then(|w| w.parse::<u64>().ok()) {
                request.insert("version".into(), json!(id));
            }
            "restore"
        }
        "seek" => {
            let t = words.first().ok_or("seek expects a time")?;
            request.insert("at".into(), time_value("seek", t)?);
            "seek"
        }
        "frame" => {
            let (file, time) = match &words[..] {
                [file] => (file, None),
                [time, file] => (file, Some(time)),
                _ => return Err("frame expects [T] <file.png>".into()),
            };
            if let Some(time) = time {
                request.insert("at".into(), time_value("frame", time)?);
            }
            request.insert("path".into(), json!(absolute(file)));
            "frame"
        }
        "import" => {
            if words.is_empty() {
                return Err("import expects files".into());
            }
            request.insert(
                "files".into(),
                json!(words.iter().map(|w| absolute(w)).collect::<Vec<_>>()),
            );
            "import"
        }
        "export" => {
            let file = words.first().ok_or("export expects a file to write")?;
            request.insert("path".into(), json!(absolute(file)));
            "export"
        }
        "command" => {
            request.insert("command".into(), json!(words.join(" ")));
            "command"
        }
        "score" | "scene" => {
            let sub = words.first().ok_or(format!("{command} expects new, read, write…"))?;
            request.insert("sub".into(), json!(sub));
            command.as_str()
        }
        "open" => "open",
        "call" => return serde_json::from_str(&value("-")?).map_err(|e| format!("stdin is not JSON: {e}")),
        other => return Err(format!("unknown command `{other}`")),
    };
    request.insert("cmd".into(), json!(cmd));
    Ok(Value::Object(request))
}

/// The `splice ctl` command line. Returns the exit code.
pub fn cli(args: &[String]) -> i32 {
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h" | "help") {
        println!("{USAGE}");
        return 0;
    }
    let request = match build(args) {
        Ok(request) => request,
        Err(err) => {
            if !err.is_empty() {
                eprintln!("splice ctl: {err}\n");
            }
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let patient = matches!(request["cmd"].as_str(), Some("wait" | "import" | "frame"))
        || (matches!(request["cmd"].as_str(), Some("export" | "score" | "scene")) && request["wait"] == json!(true))
        || (request["cmd"] == "scene" && request["sub"] == json!("studio"));
    let Some(reply) = send(&request.to_string(), patient) else {
        println!("{}", fail("Splice is not running; open it with `splice` or `make run`"));
        return 1;
    };
    let parsed: Value = serde_json::from_str(&reply).unwrap_or_else(|_| fail("Splice sent a reply that is not JSON"));
    println!("{}", serde_json::to_string_pretty(&parsed).unwrap_or(reply));
    if parsed["ok"] == json!(true) { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("12.5"), Some(12.5));
        assert_eq!(parse_time("1:02.5"), Some(62.5));
        assert_eq!(parse_time("1:00:00"), Some(3600.0));
        assert_eq!(parse_time("-1"), None);
        assert_eq!(parse_time("soon"), None);
    }

    #[test]
    fn builds_requests() {
        let r = build(&args("move --clip 3,4 --to 1:30 --track 2")).unwrap();
        assert_eq!(r, json!({"cmd": "move", "clips": [3, 4], "to": 90.0, "track": 2}));
        let r = build(&args("add --media logo.png --at end")).unwrap();
        assert_eq!(r, json!({"cmd": "add", "media": "logo.png", "at": "end"}));
        let r = build(&args("track --track 1 --mute --show")).unwrap();
        assert_eq!(r, json!({"cmd": "track", "track": 1, "muted": true, "hidden": false}));
        let r = build(&args("frame 2.5 shot.png")).unwrap();
        assert_eq!(r["at"], json!(2.5));
        assert!(r["path"].as_str().unwrap().ends_with("/shot.png"));
        let r = build(&args(
            "look --clip 2 --x -0.25 --enter slide-up --enter-for 0.8 --mask ellipse --no-invert",
        ))
        .unwrap();
        assert_eq!(
            r,
            json!({"cmd": "look", "clips": [2], "x": -0.25, "enter": "slide-up", "enter_for": 0.8, "mask": "ellipse", "invert": false})
        );
        let r = build(&args("text --text Hi --size 96 --bold --duration 2.5")).unwrap();
        assert_eq!(
            r,
            json!({"cmd": "text", "text": "Hi", "size": 96.0, "bold": true, "duration": 2.5})
        );
        assert!(build(&args("move --clip x")).is_err());
        assert!(build(&args("explode")).is_err());
    }
}
