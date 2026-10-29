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
  clean --clip C [--noise F] [--even/--no-even] | --off
                                 clean up a voice: rumble cut, steady noise down (0 to 1, 0.5), level evened out
  track --track N [--mute|--unmute] [--hide|--show]
  canvas [--aspect 16:9|9:16|1:1|4:5] [--size WxH] [--fps N] [--letterbox 2.39|1.85|2|none]
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
          --animate fade|rise|pop|type|karaoke|none [--by word|letter|line] [--over S]
                (words come in one at a time, spread over S seconds, or as captions' words are said)
  look:   --x F --y F (centre offset, fractions of the canvas)  --scale F (1 = natural)  --opacity F
          --enter KIND [--enter-for S]  --exit KIND [--exit-for S]
              KIND: none, fade, slide-up, slide-down, slide-left, slide-right, zoom, pop
          --drift none|zoom-in|zoom-out (a slow zoom across the clip)
          --key green|blue|#rrggbb|none [--similarity F] [--blend F] [--despill/--no-despill]
          --mask ellipse|rect|none [--mask-x F --mask-y F --mask-width F --mask-height F]
                [--feather F] [--invert/--no-invert]   (mask numbers are fractions of the clip's frame)
          --mirror/--no-mirror (flip left to right, as a camera shows you to yourself)
          --crop-left F --crop-right F --crop-top F --crop-bottom F | --crop none
                (cut edges away, fractions of the picture; it keeps its size and what remains stays put)
          --grade none|warm|cool|vivid|muted|faded|film|teal-orange|mono|noir   (a ready-made look)
          --exposure F (stops, −1 to 1) --contrast F --saturation F --warmth F --tint F (each −1 to 1)
                (colour, for video and pictures; `look --all` gives every video and picture the same)
          --blur F --vignette F --grain F --shake F --glitch F   (effects, 0 to 1; 0 is off)
          --backdrop on|off [--backdrop-blur F --backdrop-dim F]   (a blurred copy filling the canvas behind)
          --redact blur|pixelate|black|none [--redact-x F --redact-y F --redact-width F --redact-height F]
                (hide a box of the picture: a face, a screen; fractions of the clip's frame, centre and size)
  C is #rrggbb, #rrggbbaa or a name: white, black, paper, ink, vermilion, gold, green, blue, clear.

Transitions, speed and keyframes
  transition --clip C --kind KIND [--for S] | --none
                                 the way into clip C from the clip before it (centred on the cut), or,
                                 with nothing just before, in over what is beneath. KIND: dissolve,
                                 dip-black, dip-white, wipe-left/right/up/down, push-left/right/up/down,
                                 zoom. Default length 0.6s
  transition --all --kind KIND [--for S] [--track N]
                                 the same at every cut between two clips (on track N)
  speed --clip C [--rate R] [--reverse|--forwards]
                                 play at R (2 is twice as fast, 0.5 half; 0.1 to 16): the clip gets
                                 shorter or longer over the same source, and what follows on its track
                                 moves to match. Sound keeps its pitch
  freeze --clip C [--at T] [--for S]       hold the frame at T (default: the playhead) for S (default 2)
  keyframe --clip C --at T [--x F] [--y F] [--scale F] [--opacity F] [--ease EASE]
                                 set values at timeline time T; between keyframes they glide. EASE
                                 (from this keyframe to the next): linear, in, out, in-out (default), hold
  keyframe --clip C --at T --drop [--prop x|y|scale|opacity]   take keyframes at T away
  keyframe --clip C --clear [--prop NAME]  take them all away; values stay as they are at the playhead
  zoom --clip C [--to 2] | --none          a screen recording made in Splice: zoom in towards each burst
                                 of clicks noted while recording (keyframes on place and size)
  leak --kind warm|sun|cool [--at T]       a light leak washing over what is there, 4s (a scene: edit as one)
  captions --clip C [--words N] [--plain] [--wait]
                                 hear what is said in clip C (on this Mac, whisper.cpp) and lay captions
                                 over it on a new track: lines of up to N words (6), each word lighting up
                                 as it is said (--plain: whole lines). Each caption is a title to edit
  captions --file subtitles.srt  captions from an .srt or .vtt file

Music (a score: instruments and notes, played by General MIDI sounds)
  score new [--file score.json] [--name N] [--at T] [--track N] [--wait]
                                 add a score to the media (the starter if no file); with --at, place it
  score read --media M           the score as JSON, and what it compiles to: length, notes per part
  score write --media M --file score.json [--wait]
                                 replace the score; it re-renders, and every clip of it follows
  score instruments              the instruments (General MIDI) and the drum kit's keys
  The format: {title, tempo, meter, tracks: [{id, instrument, clips: [{at, notes:
  [{at, duration, pitch, velocity}], repeat, length, transpose}], gain, pan, reverb, drums}]}.
  Times are in beats (quarter notes); pitches "C4" or MIDI numbers.

Scenes (animation and diagrams: layers of shapes, lines, paths and words that move)
  scene new [--file scene.json] [--name N] [--duration D] [--at T] [--track N] [--opaque] [--wait]
                                 add a scene (a starter diagram if no file); see-through unless --opaque
  scene read --media M           its JSON, length, file, and how its render went
  scene write --media M [--file scene.json] [--duration D] [--opaque|--transparent] [--wait]
                                 change it; it re-renders (about a second) and its clips follow
  scene help                     the scene format, with an example
  scene sheet --media M <file.png> [--every S]
                                 frames across the scene in a grid, each with its time: see the motion
  scene frame --media M [T] <file.png>     the scene alone at T seconds into it
  scene patch --media M [--layer NAME] --json '{"text": "Plan"}' [--wait]
                                 merge into one named layer (or the scene); null removes a field
  scene add --media M --add '{"type": "text", …}' [--into NAME] [--wait]
                                 add a layer on top, or inside the group or shape NAME
  scene remove --media M --remove NAME [--wait]
  scene check --file scene.json [--duration D]
                                 validate without changing anything: errors, warnings, an outline
  open --media M                 open a score or scene in Splice's own editor (a symbol: its own window)

Symbols (pieces kept for reuse in any movie: intros, lower thirds, logos)
  symbol list                    your symbols: names, ids, lengths, canvas sizes, placeholders
  symbol read --name N           one of your symbols, without using it
  symbol make [--clip C,…] [--name N] [--keep] [--wait]
                                 make the clips (default: the selection) a symbol; they become one use
                                 of it unless --keep. Titles become placeholders ("Title", "Title 2")
  symbol make --media M [--name N]          keep one piece of media (or a score, a scene) as a symbol
  symbol add --name N [--at T] [--track N] [--wait]
                                 use a symbol (default: at the playhead, over what is there)
  symbol read --media M|--clip C           a use: its placeholders, what each holds, how it stands
                                 against your symbols (current, newer, different, missing)
  symbol fill --media M|--clip C --label L (--text WORDS | --color C | --use MEDIA | --reset) [--wait]
                                 fill one placeholder for this use only; it re-renders
  symbol length --media M|--clip C --duration D [--wait]
                                 make a use run longer or shorter: its hold stretches (so does dragging
                                 the end of a whole use on the timeline)
  symbol fit --media M|--clip C --as canvas|own [--wait]
                                 in a movie of another shape: lay it out for this movie, or keep its own
  symbol update --media M|--clip C          take the published version, keeping fills that still fit
  symbol update --all [--name N]           update every out-of-date use (of N)
  symbol break --clip C          turn a use back into its clips
  symbol keep --media M [--name N]          put a use's own copy in your symbols (when it is missing)
  symbol edit --name N|--media M           open the symbol in its own window; save there to publish
  symbol rename --name N --new NEW | duplicate --name N --new NEW | delete --name N
                                 manage your symbols (delete moves it to the Trash)
  placeholder --clip C [--label L] | --none
                                 in a symbol's own window: mark what each use can change (no label: named
                                 by what it is); the same label joins placeholders. Then `save` publishes
  hold --start T --end T | --clear         in a symbol's own window: the part a longer or shorter use
                                 stretches (default: all of it, evenly)

Story (planning: beats end to end along the timeline, and cues)
  story read                     beats (id, start, length, title, story, capture, coverage) and cues
  story list                     the plan as a shot list (plain text, in `text`)
  story add --title T [--duration D] [--after BEAT] [--story TEXT] [--capture TEXT]
  story set --beat B [--title T] [--story TEXT] [--capture TEXT] [--duration D] [--color C]
                                 a new length moves the beats after it, footage and all
  story move --beat B --index I  put a beat at position I (from 0); every beat's footage follows
  story remove --beat B          take a beat out (its footage stays; later beats close up)
  story fill --beat B --media M  lay media into a beat: at its start, as long as the beat
  story shot add --beat B [--image FILE] [--note TEXT]
                                 a reference shot: what it should look like (a picture, a frame of a video)
  story shot set --shot S --note TEXT  |  story shot move --shot S --index I  |  story shot remove --shot S
  story cast --beat B [--add ENTRY] [--remove ENTRY]
                                 who and what from the bible is in a beat
  story brief --beat B [--dir DIR]
                                 everything to make a beat's footage: what happens, what to capture, its
                                 cast's words and pictures, the look, length and shape, the frames just
                                 before and after (written to DIR), and an `outline` prompt to start from
  story take pending --beat B [--tool T] [--model M] [--mode X] [--prompt P] [--refs a,b] [--job J]
                                 a take being made (generated): shows on the beat while it runs
  story take land --take T --video FILE [--seam] [--keep]
  story take add --beat B --video FILE [origin as above] [--seam] [--keep]
                                 footage for a take; it is laid into the beat unless --keep (and a beat
                                 with nothing laid in takes it anyway). --seam skips an extension's
                                 first three frames, which repeat what it extends. The reply's `take`
                                 says if it was chosen, and if it is short of or longer than the beat
  story take fail --take T       a generation that failed: its pending take goes
  story take choose --take T | story take remove --take T
  cue add [--at T] --label TEXT  |  cue set --cue C [--at T] [--label TEXT]  |  cue remove --cue C
  view edit|story|record         switch the window between Edit, the Story board and Record

The bible (who and what the movie keeps coming back to, and its look)
  bible read                     entries: kind, name, words, pictures, the beats each is in
  bible add --kind character|place|thing|look --name N [--words W] [--image FILE]
  bible set --entry E [--name N] [--words W]
  bible picture --entry E --image FILE | --remove I     (the first picture is the face briefs lead with)
  bible remove --entry E         E is an entry's id or name

Recording (each source to its own file; the take is laid out as the scene says)
  record sources                 displays, windows, cameras, microphones, and permissions
  record setup [SETUP] [--default]
                                 show or change what this project records; --default makes it the
                                 setup new projects start with. SETUP is any of:
      --screen display:N|window:APP|none   --region X,Y,W,H (points, on that display)
      --camera NAME|none   --microphone NAME|default|none   --system | --no-system (the Mac's sound)
      --layout screen|bubble|side|camera   --corner tl|tr|bl|br   --size 0.3 (bubble height)
      --shape circle|square   --split 0.32 (camera's share, side by side)   --mirror/--no-mirror
      --clean/--no-clean (takes' mic clips come in with the voice cleaned up)
      --codec h264|hevc   --quality good|high|best   --fps 30|60   --height full|4k|1440|1080
      --cursor | --no-cursor   --clicks | --no-clicks (note clicks, to zoom in on later)
  record adjust screen|camera [LOOK] [--reset]
                                 change a picture as recorded clips will: --crop-left/-right/-top/
                                 -bottom F, colour (--exposure …), --key green, --blur F, --redact …
                                 (any look option but place and size, which the scene sets); shown
                                 live, and each take's clip starts with it (the file is untouched)
  record mic-mode                open macOS's Mic Mode menu (Voice Isolation, Wide Spectrum); the
                                 mode in use shows in `record status` as mic_mode
  record start [SETUP] [--for S] [--beat B | --at T [--track N] | --into media]
                                 start (answers once recording); the take goes after everything on
                                 the timeline, into beat B, at T, or only into the media
  record pause | resume
  record stop [--wait]           stop; with --wait, answer once the take is in the project
  record status                  recording or not, for how long, sound levels, the last take's folder

Operating the window (as a person would: for checking the interface)
  ui                             window size and scale, mouse, stage, timeline and picture bounds,
                                 and in Record the scene and the camera bubble
  click X Y [--double] [--right] [--shift|--cmd|--alt|--ctrl]
  hover X Y                      move the pointer there (tooltips show after a moment)
  drag X1 Y1 X2 Y2 [--shift…]    press, move, release
  scroll X Y [--dy N] [--dx N]   scroll the panel under the point (negative --dy scrolls down)
  key KEYS...                    press keys: cmd-t, escape, shift-left, enter, space
  type TEXT                      type characters into whatever has the keyboard
  Positions are window points from the top left: a Retina screenshot's pixels halved (see `ui`).

Playing and the window
  seek T | play | pause
  select [--clip C]                        select clips (none: deselect)
  command <words>                          run a command from the command menu (zoom fit, versions)
  save [--label TEXT]                      keep a version
  export <file.mp4> [--draft|--high] [--from T --to T | --clip C | --beat B] [--wait]
                                 the movie, or just a stretch of it (a clip's span, a beat)
  say --text TEXT                          show one line in Splice's status bar

Talking with the person
  wait [--name NAME]             connect as an assistant: blocks until they send a message from
                                 Splice (Cmd-Shift-A), prints it with what they had selected, and exits
  call                           send one JSON request read from stdin

  With more than one Splice window open, name the project: --project NAME|PATH (anywhere in the
  command) or SPLICE_PROJECT=NAME; with one, commands go to it.
  T is seconds (12.5), m:ss (1:02.5) or h:mm:ss. Tracks count from 1, the bottom layer;
  a track number one above the top makes a new layer. --clip takes ids separated by commas.
  A value of @FILE is read from FILE, and - from stdin."#;

/// Where running Splice windows listen: one socket each, named by process.
fn sockets() -> PathBuf {
    dirs().data.join("ctl")
}

/// Where this Splice listens.
pub fn socket_path() -> PathBuf {
    sockets().join(format!("{}.sock", std::process::id()))
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

/// Starts answering requests on this window's own socket.
pub fn listen() -> Option<UnboundedReceiver<Incoming>> {
    use std::os::unix::fs::PermissionsExt;
    let dir = sockets();
    std::fs::create_dir_all(&dir).ok()?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    // Before per-window sockets, every Splice shared one.
    let _ = std::fs::remove_file(dirs().data.join("splice.sock"));
    let socket = socket_path();
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).ok()?;
    let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
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

/// A running Splice window, and the project it has open.
struct Window {
    socket: PathBuf,
    project: PathBuf,
    name: String,
}

/// Every Splice window that answers. Sockets left by windows that are gone
/// are cleared away.
fn windows() -> Vec<Window> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(sockets()) else {
        return found;
    };
    for entry in entries.flatten() {
        let socket = entry.path();
        if socket.extension().is_none_or(|e| e != "sock") {
            continue;
        }
        match ask(&socket, &json!({ "cmd": "who" }).to_string(), false) {
            Some(reply) => {
                let who: Value = serde_json::from_str(&reply).unwrap_or(Value::Null);
                found.push(Window {
                    socket,
                    project: PathBuf::from(who["project"].as_str().unwrap_or("")),
                    name: who["name"].as_str().unwrap_or("").to_string(),
                });
            }
            None if UnixStream::connect(&socket).is_err() => {
                let _ = std::fs::remove_file(&socket);
            }
            None => {}
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

fn listing(windows: &[Window]) -> String {
    windows
        .iter()
        .map(|w| format!("{} ({})", w.name, w.project.display()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The window to talk to: the only one, or the one whose project `wanted`
/// names (a path, or a project's name).
fn choose(wanted: Option<&str>) -> Result<PathBuf, String> {
    let open = windows();
    if open.is_empty() {
        return Err("Splice is not running; open it with `splice` or `make run`".into());
    }
    let Some(wanted) = wanted.map(str::trim).filter(|w| !w.is_empty()) else {
        return match &open[..] {
            [only] => Ok(only.socket.clone()),
            _ => Err(format!(
                "{} Splice windows are open: {}. Say which with --project NAME (or set SPLICE_PROJECT)",
                open.len(),
                listing(&open)
            )),
        };
    };
    let path = std::fs::canonicalize(wanted).ok();
    let name = wanted.trim_end_matches(".splice").to_lowercase();
    let matches: Vec<&Window> = open
        .iter()
        .filter(|w| {
            path.as_ref()
                .is_some_and(|p| std::fs::canonicalize(&w.project).ok().as_ref() == Some(p))
                || w.name.to_lowercase() == name
        })
        .collect();
    match &matches[..] {
        [one] => Ok(one.socket.clone()),
        [] => Err(format!("no open Splice window has {wanted}; open: {}", listing(&open))),
        _ => Err(format!(
            "several windows match {wanted}; give its path: {}",
            listing(&open)
        )),
    }
}

/// Sends one line to one window and reads its answer.
fn ask(socket: &std::path::Path, line: &str, patient: bool) -> Option<String> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let _ = stream.set_read_timeout(Some(if patient {
        Duration::from_secs(60 * 60 * 24 * 365)
    } else {
        Duration::from_secs(15)
    }));
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
            | "--query" | "--crop" | "--grade" | "--ease" | "--prop" | "--backdrop" | "--redact" | "--letterbox"
            | "--animate" | "--by" => {
                request.insert(arg[2..].into(), json!(value(next(arg)?)?));
            }
            "--size" | "--x" | "--y" | "--scale" | "--opacity" | "--similarity" | "--blend" | "--feather"
            | "--mask-x" | "--mask-y" | "--mask-width" | "--mask-height" | "--enter-for" | "--exit-for"
            | "--crop-left" | "--crop-top" | "--crop-right" | "--crop-bottom" | "--exposure" | "--contrast"
            | "--saturation" | "--warmth" | "--tint" | "--rate" | "--blur" | "--vignette" | "--grain" | "--shake"
            | "--glitch" | "--backdrop-blur" | "--backdrop-dim" | "--redact-x" | "--redact-y" | "--redact-width"
            | "--redact-height" | "--noise" => {
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
            "--at" | "--to" | "--from" | "--start" | "--end" | "--duration" => {
                let raw = next(arg)?;
                request.insert(arg[2..].into(), time_value(arg, raw)?);
            }
            "--track" | "--fps" | "--version" | "--beat" | "--cue" | "--index" | "--display" | "--after" | "--shot"
            | "--take" => {
                let n: u64 = next(arg)?.parse().map_err(|_| format!("{arg} expects a number"))?;
                request.insert(arg[2..].into(), json!(n));
            }
            "--image" | "--video" | "--dir" => {
                let file = next(arg)?;
                request.insert(arg[2..].into(), json!(absolute(file)));
            }
            "--entry" => {
                let e = next(arg)?;
                request.insert("entry".into(), e.parse::<u64>().map_or(json!(e), |id| json!(id)));
            }
            "--window" => {
                let w = next(arg)?;
                request.insert("window".into(), w.parse::<u64>().map_or(json!(w), |id| json!(id)));
            }
            "--over" => {
                let v: f64 = next(arg)?.parse().map_err(|_| "--over expects seconds".to_string())?;
                request.insert("over".into(), json!(v));
            }
            "--for" => {
                let v: f64 = next(arg)?.parse().map_err(|_| "--for expects seconds".to_string())?;
                request.insert("for".into(), json!(v));
            }
            "--mic" | "--clicks" | "--system" | "--cursor" | "--default" | "--mirror" | "--even" | "--clean"
            | "--off" => {
                request.insert(arg[2..].into(), json!(true));
            }
            "--no-clicks" | "--no-system" | "--no-cursor" | "--no-mirror" | "--no-even" | "--no-clean" => {
                request.insert(arg[5..].into(), json!(false));
            }
            "--screen" | "--camera" | "--microphone" | "--layout" | "--corner" | "--shape" | "--codec"
            | "--quality" | "--height" => {
                request.insert(arg[2..].into(), json!(next(arg)?));
            }
            "--split" => {
                let v: f64 = next(arg)?
                    .parse()
                    .map_err(|_| "--split expects a fraction".to_string())?;
                request.insert("split".into(), json!(v));
            }
            "--every" => {
                let v: f64 = next(arg)?.parse().map_err(|_| "--every expects seconds".to_string())?;
                request.insert("every".into(), json!(v));
            }
            "--words" if command == "captions" => {
                let n: u64 = next(arg)?.parse().map_err(|_| "--words expects a number".to_string())?;
                request.insert("words".into(), json!(n));
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
            "--media" | "--use" => {
                let m = next(arg)?;
                request.insert(arg[2..].into(), m.parse::<u64>().map_or(json!(m), |id| json!(id)));
            }
            "--text" | "--label" | "--name" | "--new" | "--as" | "--kind" | "--words" | "--prompt" | "--model"
            | "--mode" | "--tool" | "--refs" | "--job" | "--aspect" | "--layer" | "--into" | "--remove" | "--json"
            | "--add" | "--title" | "--story" | "--capture" | "--region" | "--note" => {
                request.insert(arg[2..].into(), json!(value(next(arg)?)?));
            }
            "--file" => {
                let file = next(arg)?;
                request.insert("content".into(), json!(value(&format!("@{file}"))?));
            }
            "--opaque" | "--transparent" => {
                request.insert("transparent".into(), json!(arg == "--transparent"));
            }
            "--shift" | "--cmd" | "--alt" | "--ctrl" | "--right" => {
                request.insert(arg[2..].into(), json!(true));
            }
            "--double" => {
                request.insert("count".into(), json!(2));
            }
            "--count" => {
                let n: u64 = next(arg)?.parse().map_err(|_| "--count expects a number".to_string())?;
                request.insert("count".into(), json!(n));
            }
            "--dx" | "--dy" => {
                let v: f64 = next(arg)?.parse().map_err(|_| format!("{arg} expects points"))?;
                request.insert(arg[2..].into(), json!(v));
            }
            "--ripple" | "--wait" | "--keep" | "--none" | "--all" | "--reset" | "--clear" | "--seam" | "--drop"
            | "--plain" => {
                request.insert(arg[2..].into(), json!(true));
            }
            "--reverse" | "--forwards" => {
                request.insert("reverse".into(), json!(arg == "--reverse"));
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
        | "clean" | "track" | "canvas" | "undo" | "redo" | "play" | "pause" | "select" | "save" | "say" | "wait"
        | "text" | "color" | "style" | "look" | "fonts" | "transition" | "speed" | "freeze" | "keyframe" | "leak"
        | "captions" | "zoom" => command.as_str(),
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
            match (sub.as_str(), &words[1..]) {
                ("frame", [file]) => {
                    request.insert("path".into(), json!(absolute(file)));
                }
                ("frame", [time, file]) => {
                    request.insert("at".into(), time_value("frame", time)?);
                    request.insert("path".into(), json!(absolute(file)));
                }
                ("sheet", [file]) => {
                    request.insert("path".into(), json!(absolute(file)));
                }
                ("frame" | "sheet", _) => return Err(format!("{command} {sub} expects [T] <file.png>")),
                _ => {}
            }
            command.as_str()
        }
        "open" => "open",
        "symbol" => {
            request.insert("sub".into(), json!(words.first().map_or("list", |w| w.as_str())));
            "symbol"
        }
        "placeholder" => "placeholder",
        "bible" => {
            request.insert("sub".into(), json!(words.first().map_or("read", |w| w.as_str())));
            "bible"
        }
        "hold" => "hold",
        "ui" => "ui",
        "story" | "cue" | "record" => {
            let sub = words.first().map_or(
                match command.as_str() {
                    "record" => "status",
                    _ => "read",
                },
                |w| w.as_str(),
            );
            request.insert("sub".into(), json!(sub));
            if let Some(action) = words.get(1).filter(|_| sub == "shot" || sub == "take") {
                request.insert("action".into(), json!(action));
            }
            if command == "record" && sub == "adjust" {
                let source = words.get(1).ok_or("record adjust expects screen or camera")?;
                request.insert("source".into(), json!(source));
            }
            command.as_str()
        }
        "view" => {
            let which = words.first().ok_or("view expects edit, story or record")?;
            request.insert("view".into(), json!(which));
            "view"
        }
        "click" | "hover" | "drag" | "scroll" => {
            let numbers: Vec<f64> = words
                .iter()
                .map(|w| {
                    w.parse::<f64>()
                        .map_err(|_| format!("{command} expects positions in window points"))
                })
                .collect::<Result<_, _>>()?;
            let names: &[&str] = if command == "drag" {
                &["x", "y", "x2", "y2"]
            } else {
                &["x", "y"]
            };
            if numbers.len() != names.len() {
                return Err(format!("{command} expects {} numbers", names.len()));
            }
            for (name, n) in names.iter().zip(numbers) {
                request.insert((*name).into(), json!(n));
            }
            command.as_str()
        }
        "key" => {
            if words.is_empty() {
                return Err("key expects keys, e.g. cmd-t escape".into());
            }
            request.insert("keys".into(), json!(words));
            "key"
        }
        "type" => {
            if !request.contains_key("text") {
                request.insert("text".into(), json!(words.join(" ")));
            }
            "type"
        }
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
    // Which window: --project anywhere in the arguments, else $SPLICE_PROJECT.
    let mut project = std::env::var("SPLICE_PROJECT").ok();
    let mut rest = Vec::new();
    let mut given = args.iter();
    while let Some(arg) = given.next() {
        match arg.strip_prefix("--project") {
            Some("") => project = given.next().cloned(),
            Some(value) if value.starts_with('=') => project = Some(value[1..].to_string()),
            _ => rest.push(arg.clone()),
        }
    }
    let args = &rest[..];
    if args.is_empty() {
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
        || (matches!(
            request["cmd"].as_str(),
            Some("export" | "score" | "scene" | "record" | "symbol")
        ) && request["wait"] == json!(true));
    let socket = match choose(project.as_deref()) {
        Ok(socket) => socket,
        Err(e) => {
            println!("{:#}", fail(e));
            return 1;
        }
    };
    let Some(reply) = ask(&socket, &request.to_string(), patient) else {
        println!("{:#}", fail("that Splice window did not answer; it may have closed"));
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
        let r = build(&args(
            "record adjust camera --crop-left 0.2 --exposure 0.5 --key green --reset",
        ))
        .unwrap();
        assert_eq!(
            r,
            json!({"cmd": "record", "sub": "adjust", "source": "camera", "crop_left": 0.2, "exposure": 0.5,
                   "key": "green", "reset": true})
        );
        assert!(build(&args("record adjust")).is_err(), "which picture");
        assert_eq!(
            build(&args("record mic-mode")).unwrap(),
            json!({"cmd": "record", "sub": "mic-mode"})
        );
        assert!(build(&args("move --clip x")).is_err());
        assert!(build(&args("explode")).is_err());
        let r = build(&args(
            "record setup --screen window:Safari --camera C920 --no-system --layout bubble --size 0.25 --fps 60 --height 1440 --default",
        ))
        .unwrap();
        assert_eq!(
            r,
            json!({"cmd": "record", "sub": "setup", "screen": "window:Safari", "camera": "C920", "system": false,
                   "layout": "bubble", "size": 0.25, "fps": 60, "height": "1440", "default": true})
        );
        let r = build(&args("record setup --mirror --clean")).unwrap();
        assert_eq!(
            r,
            json!({"cmd": "record", "sub": "setup", "mirror": true, "clean": true})
        );
        let r = build(&args("clean --clip 9 --noise 0.6 --no-even")).unwrap();
        assert_eq!(r, json!({"cmd": "clean", "clips": [9], "noise": 0.6, "even": false}));
        let r = build(&args("look --clip 3 --mirror")).unwrap();
        assert_eq!(r, json!({"cmd": "look", "clips": [3], "mirror": true}));
    }

    #[test]
    fn builds_symbol_requests() {
        let r = build(&args("symbol make --clip 3,4 --name Intro --keep")).unwrap();
        assert_eq!(
            r,
            json!({"cmd": "symbol", "sub": "make", "clips": [3, 4], "name": "Intro", "keep": true})
        );
        let r = build(&args("symbol fill --media 9 --label Logo --use logo.png --wait")).unwrap();
        assert_eq!(
            r,
            json!({"cmd": "symbol", "sub": "fill", "media": 9, "label": "Logo", "use": "logo.png", "wait": true})
        );
        assert_eq!(build(&args("symbol")).unwrap()["sub"], json!("list"));
        let r = build(&args("placeholder --clip 5 --none")).unwrap();
        assert_eq!(r, json!({"cmd": "placeholder", "clips": [5], "none": true}));
        let r = build(&args("symbol rename --name Intro --new Opener")).unwrap();
        assert_eq!(
            r,
            json!({"cmd": "symbol", "sub": "rename", "name": "Intro", "new": "Opener"})
        );
        let r = build(&args("symbol fit --clip 4 --as canvas")).unwrap();
        assert_eq!(r, json!({"cmd": "symbol", "sub": "fit", "clips": [4], "as": "canvas"}));
        let r = build(&args("bible add --kind character --name Ada --words silver")).unwrap();
        assert_eq!(
            r,
            json!({"cmd": "bible", "sub": "add", "kind": "character", "name": "Ada", "words": "silver"})
        );
        let r = build(&args("story take pending --beat 3 --model seedance_2_5 --job j1")).unwrap();
        assert_eq!(
            r,
            json!({"cmd": "story", "sub": "take", "action": "pending", "beat": 3, "model": "seedance_2_5", "job": "j1"})
        );
        let r = build(&args("export out.mp4 --from 2 --to 5")).unwrap();
        assert_eq!((r["from"].clone(), r["to"].clone()), (json!(2.0), json!(5.0)));
        let r = build(&args("look --clip 2 --crop-left 0.25")).unwrap();
        assert_eq!(r["crop_left"], json!(0.25));
        let r = build(&args("symbol update --all")).unwrap();
        assert_eq!(r, json!({"cmd": "symbol", "sub": "update", "all": true}));
    }
}
