# Splice

A small, quiet video editor. Rust, GPU-rendered with [GPUI](https://www.gpui.rs),
CapCut as the inspiration and simplicity as the rule: bring in videos, images
and sound, arrange and layer them on a timeline, cut, play, export.

## Run

Needs Rust 1.90+ and ffmpeg (`brew install ffmpeg`).

```sh
make run                     # opens your last project, or a new one
cargo run --release -p splice-app -- path/to/film.splice
```

## Using it

- **Import**: ⌘I, the Import button, or drop files from Finder anywhere on the
  window. Drop them on the timeline to place them where they land.
- **Arrange**: drag media from the bin onto the timeline. Drag clips to move
  them in time or between layers, and drag above the top layer to make a new
  one. Higher layers draw over lower ones. Clips snap to edges and the
  playhead; hold ⌥ to move freely.
- **Trim**: drag either end of a clip.
- **Cut**: S or ⌘B splits the selection at the playhead, or every clip under
  it if nothing is selected. ⌫ deletes, ⇧⌫ ripple-deletes (closes the gap),
  ⌘D duplicates.
- **Play**: Space plays and pauses. ← → step one frame, ⇧← ⇧→ (or J, L) one
  second. Click or drag the ruler to scrub.
- **Layers**: each track's gutter toggles *visible / hidden* and
  *sound / muted* (H and M act on the selection's tracks).
- **Canvas**: the first clip sets the canvas shape. The shape button cycles
  16:9, 9:16, 1:1 and 4:5.
- **Export**: ⌘E (H.264 + AAC MP4), or ⇧⌘E for a fast draft.
- **Everything else**: ⌘/ opens the command menu, with every command and its key.

## Titles and looks

- **Text** (⌘T, or the Text shelf): titles in any installed font, with size,
  bold, italic, alignment, colour, a box behind and a soft shadow. Type in the
  inspector, or double-click a title on the stage.
- **Colour** shelf: full-frame colours for backgrounds and title cards, and a
  half-black tint.
- **Inspector** (⌘2), for the selected clip:
  - **Place:** drag a picture on the stage, or its corners to resize. It snaps
    to the centre lines. There's a nine-point grid, plus size, opacity and
    position sliders.
  - **Animate:** in and out with fade, slide (four ways), zoom or pop, with a
    length you choose. A slow zoom can run across the whole clip.
  - **Green screen:** key out green or blue, with strength, softness and fringe
    clean-up.
  - **Mask:** circle or box, with size, position, feather and invert.
  - **Sound:** volume.

The viewer and export share one renderer, so what you see is what you get.
Titles are drawn once in Rust (cosmic-text) and that same picture is overlaid
on export. Keys and masks are ffmpeg filters that the viewer's decoder runs
too. Animation is one expression, evaluated for the viewer and printed into
ffmpeg for export.

## Scores and scenes

Two kinds of media Splice makes rather than imports. Each opens in its own
editor (double-click it in the media panel or on the timeline, or use the
**New score** / **New scene** tiles), and each re-renders when it changes,
with every clip of it following.

- **Score**: music as notes. Instruments, tempo, meter and notes in beats,
  shown as a piano roll with one lane per instrument. Click to add a note,
  drag to move or lengthen it, ⌫ to delete. Each lane has mute, solo and an
  instrument picker covering the 128 General MIDI instruments plus a drum kit.
  It plays with the timeline, so you hear it against the picture. Splice
  renders it with rustysynth and the GeneralUser GS SoundFont, which it
  downloads once (32 MB) and checks. The format is
  [Clapper](https://github.com/ArchAstro/clapper)'s score JSON, so the
  assistant can compose it: **Ask** in the editor sends it what you have open.
- **Scene**: an animation or diagram written as a React component and
  rendered by Clapper, one frame at a time. The editor shows the code beside
  the composited picture, with its length and a transparent background
  (on by default, so it overlays what is under it). **Code** opens the file
  in your editor; **Studio** opens Clapper's studio on it in the browser. A
  save in either re-renders it in Splice within seconds. Splice uses
  `clapper` from `$SPLICE_CLAPPER` or the PATH. If it finds neither, it
  fetches Clapper's pinned v0.4.0 launcher, checking its checksum. On first
  use the launcher downloads its runtime (Node, Chromium, ffmpeg).

Renders are cached by what they are made from, and kept in the project file
so it still opens anywhere.

## Projects

A project is one `.splice` file: a SQLite database holding the imported
media and every version of the timeline. Copy it anywhere and it opens with
its media intact.

- **Autosave**: every change is written within two seconds. New projects
  start as `~/Movies/Splice/Untitled N.splice`; ⌘S on one asks where to keep it.
- **Versions**: autosaves and saves (⌘S) are both kept in the file. ⇧⌘H
  lists them, and restoring one is an ordinary edit, so ⌘Z takes it back.
- **Formats are versioned**: the file records its format version and the
  project JSON its schema, and both have upgrade hooks.

Headless, for scripts:

```sh
splice --new film.splice a.mp4 b.png c.m4a   # media end to end
splice --info film.splice
splice --export film.splice film.mp4 [--draft]
```

## Assistants

Other programs work in the open project through `splice ctl` while you edit,
after Stet's `stet ctl`. The included skill (`skill/splice/SKILL.md`, installed
with `make install-skill`) teaches Claude Code to use it.

| Command | Does |
|---|---|
| `splice ctl status` / `read` | the project; every track and clip with ids and times |
| `splice ctl frame [T] out.png` | the picture at time T, drawn as export draws it |
| `splice ctl import FILES [--at T]` | brings media in, optionally onto the timeline |
| `splice ctl add / move / trim / split / delete / duplicate / volume` | timeline edits, each one undo step |
| `splice ctl track`, `canvas`, `undo`, `redo` | layers, canvas shape, history |
| `splice ctl text`, `color`, `style`, `fonts` | titles in system fonts, colour cards and tints |
| `splice ctl look` | position, size, opacity, animation, green screen, mask |
| `splice ctl score new / read / write / instruments` | compose music: a score in Clapper's JSON |
| `splice ctl scene new / read / write / studio` | animations and diagrams from React code |
| `splice ctl open --media M` | opens a score or scene in its editor |
| `splice ctl seek`, `play`, `pause`, `select`, `command` | the playhead, selection and any menu command |
| `splice ctl save`, `versions`, `export FILE [--wait]` | versions and export |
| `splice ctl wait --name Claude` | connects an assistant and waits for your message |
| `splice ctl say --text T` | shows a line in the status bar |

While an assistant waits, the title bar shows `● Claude`; press ⇧⌘A (or click
it) to send a message. Your playhead and selected clips travel with it, so
"tighten this clip" means the one you selected. The socket is
`~/Library/Application Support/Splice/splice.sock`, and only your user can
connect to it.

## Layout

| Crate | Owns |
|---|---|
| `crates/splice-core` | timeline model and edits, undo, the `.splice` file, export plans. No platform code. |
| `crates/splice-app` | GPUI window, bin, viewer, timeline, ffmpeg decoding, playback, sound (cpal) |

Decoding and export run ffmpeg as a child process. A bad file can't crash the
editor, and codec support is whatever your ffmpeg has.
