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

## Layout

| Crate | Owns |
|---|---|
| `crates/splice-core` | timeline model and edits, undo, the `.splice` file, export plans. No platform code. |
| `crates/splice-app` | GPUI window, bin, viewer, timeline, ffmpeg decoding, playback, sound (cpal) |

Decoding and export run ffmpeg as a child process. A bad file can't crash the
editor, and codec support is whatever your ffmpeg has.
