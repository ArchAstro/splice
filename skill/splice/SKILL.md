---
name: splice
description: Work with the user inside their Splice video editor - see the project they have open, place, move, trim, split and delete clips on the timeline, import media, plan the story as beats and cues, record the screen, compose music as a score, make animated scenes and diagrams, look at any frame, export, and take requests they send from the editor. Also covers installing Splice and this skill. Use when the user asks for help editing a video they have open in Splice, says "my edit/timeline/movie/story/score", asks you to listen for requests from Splice, or asks to install or set up Splice.
---

# Working in the user's Splice editor

`splice ctl` talks to the running Splice window. Every reply is JSON with
`"ok"`, and an `"error"` that says what to do when it is false. Each edit is
one undo step for the user, shows at once, and is autosaved into their
project file. Nothing you do saves a version or exports unless you ask it to.

## 0. Is Splice installed?

```sh
command -v splice && splice --version && splice ctl status
```

- `splice` found, and `status` answers: go to step 1.
- `splice` found, but the reply says Splice is not running: ask the user to
  open it (Splice in Applications, or `splice my-film.splice`). Do not launch
  it for them unless asked.
- `splice` not found: offer to install it, and install only when they agree.
  In a checkout of the repository, `target/release/splice` works too (build
  it with `make build-release`).

### Installing Splice and this skill

Splice runs on macOS 13 or newer (Apple Silicon or Intel).

```sh
curl -fsSL https://raw.githubusercontent.com/ArchAstro/splice/main/install.sh | sh
```

This downloads the latest release, verifies its checksum, installs
`Splice.app` in `/Applications` with the `splice` command in `~/.local/bin`
(the same program), this skill in `~/.claude/skills/splice` (other agents:
copy `SKILL.md` to wherever your skills live), and ffmpeg with Homebrew if it
is missing. If the download is refused because the repository is private, and
`gh` is signed in:

```sh
gh api repos/ArchAstro/splice/contents/install.sh -H "Accept: application/vnd.github.raw" | sh
```

To build from source instead (needs Rust; if `cargo` is missing, point the
user to https://rustup.rs rather than installing a toolchain yourself):

```sh
gh repo clone ArchAstro/splice && sh splice/scripts/install.sh
```

Check it worked with `splice --version`, then have the user open Splice.
Running the installer again updates it. To remove it: delete
`/Applications/Splice.app`, `~/.local/bin/splice` and
`~/.claude/skills/splice`. Projects (`.splice` files) and recordings in
`~/Movies/Splice` are the user's; never delete them.

## 1. See what is open

```sh
splice ctl status    # project, canvas, duration, playhead, selection, export progress
splice ctl read      # everything: media and every track and clip, with ids and times
```

- If the reply says Splice is not running, tell the user; do not start it
  yourself unless they ask.
- If several Splice windows are open, commands are refused until you name
  the project: add `--project NAME` (or its path) to every command, or run
  with `SPLICE_PROJECT=NAME`. The error lists the open projects; ask the
  user which one they mean if it is not clear.
- Times are seconds. Tracks count from 1, the bottom layer; higher tracks
  draw over lower ones (that is how overlays work).
- Clips have an `id`, a `media` id, `start`/`end` on the timeline,
  `source_in` (where in the media file the clip begins) and `volume`.
- `revision` goes up with every change; read again before editing if it moved.

## 2. Look before and after you edit

```sh
splice ctl frame 12.5 /tmp/at-12s.png   # the picture at 12.5s, exactly as export draws it
splice ctl frame /tmp/now.png           # at the playhead
```

Read the PNG to see it. Use this to find the right moment (scan a few
times), and to check your work afterwards.

## 3. Edit

```sh
splice ctl import ~/Movies/clip.mov                 # into the media panel
splice ctl import a.mp4 b.mp4 --at end              # and onto the timeline, end to end
splice ctl add --media logo.png --at 0 --track 2    # media by id or name; default: end of the track
splice ctl move --clip 7 --to 10 --track 1
splice ctl trim --clip 7 --start 11.2 --end 15      # where its edges go, in timeline seconds
splice ctl split --at 8.5                           # every clip under 8.5s; --clip 3 for one
splice ctl delete --clip 4,5 --ripple               # --ripple closes the gap
splice ctl duplicate --clip 3
splice ctl volume --clip 9 --value 0.4              # 1 as recorded, 0 silent
splice ctl track --track 2 --mute --hide            # --unmute, --show
splice ctl canvas --aspect 9:16                     # or --size 1080x1920, --fps 30
splice ctl undo
```

- Clips on one track never overlap. A clip placed or moved where there is no
  room rises to the first free track above; the reply shows where it landed.
- `trim` keeps a video clip inside its media: you cannot extend it past the
  file's start or end. Images can be any length.
- `split` returns the new right halves under `new`.
- Read the reply: it has the clip as it ended up, the new `duration` and
  `revision`. If something failed, read again and retry once; if it fails
  twice, tell the user what happened.

## 4. Titles, colours, and how pictures look

```sh
splice ctl fonts --query avenir                     # font families installed here
splice ctl text --text "Day one" --at 2 --duration 3 --font "Avenir Next" --size 96 --enter pop --exit fade
splice ctl text --text "Ada Lovelace" --y 0.32 --size 54 --box "#000000b3" --no-shadow   # a caption
splice ctl style --clip 12 --text "Day two" --color gold --italic
splice ctl color --color black --at 0 --duration 2                 # a title card or background
splice ctl color --color "#00000080" --at 10 --duration 4          # a tint over what is under it
splice ctl look --clip 7 --scale 0.4 --x 0.28 --y -0.25            # picture in picture, top right
splice ctl look --clip 7 --mask ellipse --feather 0.15             # a soft round crop
splice ctl look --clip 9 --key green --similarity 0.18             # green screen (blue, #rrggbb too)
splice ctl look --clip 3 --drift zoom-in --enter fade --enter-for 1
```

- A title's `--size` is the letter height in canvas pixels. Text uses real
  system fonts; `fonts` lists them, and a font not listed is refused.
- `--x`/`--y` move a picture's centre by fractions of the canvas (0 is
  centred, +0.5 is the right/bottom edge); `--scale` 1 is its natural size
  (media fitted to the canvas, a title as set).
- Animations: `--enter`/`--exit` take none, fade, slide-up, slide-down,
  slide-left, slide-right, zoom or pop (all but none also fade);
  `--enter-for` is seconds. `--drift zoom-in|zoom-out` zooms slowly across
  the whole clip.
- Keys and masks are for video and pictures; masks work on titles and
  colours too. Mask numbers are fractions of the clip's own frame.
- Text and colour clips are on the timeline like any other clip: move,
  trim, split and delete them the same way. `read` shows their `text` and
  `look`.
- Always check with `frame` afterwards: it is drawn exactly as export draws.

## 5. Music: compose a score

A score is media made from notes: instruments, a tempo, notes in beats. Splice
plays it with General MIDI instruments (a SoundFont) and it sits on the
timeline like any sound; every clip of it follows when you change it.

```sh
splice ctl score instruments                         # the 128 instruments, and the drum kit's keys
splice ctl score new --file score.json --at 0 --wait # add it (and place it); --wait until it renders
splice ctl score read --media 20                     # the JSON, plus length, notes and range per part
splice ctl score write --media 20 --file score.json --wait
```

```json
{ "title": "Morning Light", "tempo": 100, "meter": [4, 4], "tail": 2,
  "tracks": [
    { "id": "melody", "instrument": "flute", "gain": 0.85, "reverb": 0.4, "pan": 0.15,
      "clips": [{ "notes": [{ "at": 0, "duration": 1, "pitch": "F#5", "velocity": 88 }] }] },
    { "id": "drums", "instrument": "drums", "drums": true, "gain": 0.6,
      "clips": [{ "at": 4, "length": 4, "repeat": 7,
                  "notes": [{ "at": 0, "duration": 0.25, "pitch": 36 }, { "at": 1, "duration": 0.25, "pitch": 38 }] }] }
  ] }
```

- Times are quarter-note beats. `tempo` is BPM or `[{at, bpm}]`; `meter` is
  `[n, d]`. A clip's notes are relative to its `at`; `repeat` plays it again
  every `length` beats; `transpose` shifts it by semitones. Pitches are
  `"C4"` (middle C, 60), `"F#5"`, `"Bb2"`, or MIDI numbers. Velocity 1–127.
- `instrument` is a General MIDI name from `score instruments` (`program`
  overrides it). Drums: `"drums": true`, and the pitch picks the drum (36
  kick, 38 snare, 42 closed hat, 46 open hat, 49 crash, 51 ride).
- Compose like a composer: pick a key, meter, tempo and a short motif; give
  parts jobs (melody, harmony, bass, rhythm); keep each in its register
  (bass E1–G3, melody around C4–C6); build an arc (statement, development,
  contrast, return, cadence) and develop the motif rather than wandering.
  Match the length to the picture: `read` the timeline, and fit the bars to
  the cuts (seconds = beats × 60 / tempo).
- Write the JSON with a script when it is long (loops for chords and
  patterns). `write` refuses a score that does not compile and says why.
- Mix with each track's `gain` (1 is as written; 0.5 is −6 dB, 2 is +6 dB;
  leave it out for 1), `pan` (−1 left to 1 right) and `reverb` (0–1). Keep
  the melody forward, pads and harmony under it, bass centred; the render
  is normalized, so only the balance matters. The person can also mix by
  ear in the score editor's Level and Pan faders; `score read` shows what
  they set, so read before you write.
- For the movie's overall mix, each clip has a `volume` (`splice ctl volume
  --clip C --value 0.4`): music under voice usually sits around 0.3–0.5.
- You cannot hear it. Check what you can (`score read` ranges and lengths,
  `frame` for picture sync) and say so; ask the person to listen.

## 6. Scenes: animation and diagrams

A scene is media made from a description: layers of shapes, lines, paths and
words, and how they move. Splice paints it itself, frame by frame. It is
see-through where nothing is drawn, so it overlays the footage: diagrams,
charts, callouts, kinetic type, lower thirds.

```sh
splice ctl scene help                                # the format in full, and an example
splice ctl scene check --file scene.json --duration 6          # validate first: errors, warnings, outline
splice ctl scene new --file scene.json --duration 6 --at 12 --name "Pipeline" --wait
splice ctl scene read --media 23                     # outline, warnings, its JSON, file, render error
splice ctl scene sheet --media 23 /tmp/sheet.png     # frames across it in a grid, timed: SEE the motion
splice ctl scene frame --media 23 1.5 /tmp/f.png     # one frame of the scene alone
splice ctl scene patch --media 23 --layer title --json '{"text": "Q3", "fill": "#f2c14e"}' --wait
splice ctl scene add --media 23 --into steps --add '{"type": "rect", "name": "four", …}' --wait
splice ctl scene remove --media 23 --remove four --wait
splice ctl scene write --media 23 --file scene.json --wait    # replace it all; also --duration, --opaque
```

- Name the layers you will change (`"name": "title"`); `patch`, `add
  --into` and `remove` find them anywhere in the scene. `patch` merges
  (JSON merge patch: `null` removes a field); without `--layer` it patches
  the scene itself (`{"camera": null}`, `{"background": "#000"}`).
- Work in small steps: `check`, then `new` or `patch`, then `sheet`. Read
  the sheet before you say you are done; it shows crowding, things off the
  edge, and timing that a single frame cannot.

```json
{ "layers": [
  { "type": "line", "x": "25%", "y": "50%", "from": [110, 0], "to": ["25%", 0],
    "stroke": "#f3eee4", "strokeWidth": 6, "draw": { "at": 0.6, "duration": 0.5 } },
  { "type": "group", "stagger": 0.25, "children": [
    { "type": "rect", "x": "25%", "y": "50%", "width": 200, "height": 120, "radius": 20,
      "fill": "#ef6a55", "shadow": { "blur": 28, "y": 12 }, "enter": "pop",
      "children": [{ "type": "text", "text": "Idea", "size": 38, "bold": true }] }
  ] },
  { "type": "text", "x": "50%", "y": "25%", "size": 64, "bold": true, "fill": "#f2c14e",
    "counter": { "to": 12500, "duration": 2, "prefix": "$" }, "enter": "rise" }
], "camera": { "zoom": [[0, 1], [4, 1.2, "inOutSine"]] } }
```

- Positions are the centre of a layer, in canvas pixels or `"50%"`;
  children are placed from their parent. Later layers draw on top.
- Anything numeric can be keyframes: `[[seconds, value], [seconds, value,
  "outBack"]]`. `enter`/`exit` take fade, pop, zoom, rise, drop,
  slide-left, slide-right, spin, or `{at, duration, ease, spring, from}`.
  `draw` draws lines and outlines; `typewriter` types text; `counter`
  counts; a group's `stagger` spaces its children's entrances.
- Text uses installed fonts (`splice ctl fonts`).
- A render takes about a second. Edits refuse JSON that does not parse or
  validate and say where; `warnings` flag fonts that are not installed and
  motion that runs past the end. Check the scene over the footage with
  `splice ctl frame` once it is placed.
- While the person has a scene open they may edit the same file (its
  `file` in `read`); `read` before you write so you do not undo their change.

## 6b. Story: plan the movie, then fill it

The story is the plan: beats laid end to end from the start of the
timeline. Each beat has a title, `story` (what happens) and `capture` (what
to film or record). Footage belongs to the beat it starts in, so restructuring
the story carries the footage along.

```sh
splice ctl story read                                  # beats with ids, times, coverage, clips; cues
splice ctl story add --title "Hook" --duration 4 --story "Why care" --capture "Close on the app"
splice ctl story set --beat 27 --duration 6 --capture "Screen: open the inspector"
splice ctl story move --beat 29 --index 0              # its footage, and everyone's, follows
splice ctl story fill --beat 27 --media intro.mov      # lay media in at the beat's start, trimmed to fit
splice ctl story remove --beat 28                      # footage stays where it is
splice ctl story list                                  # the plan as a shot list (text)
splice ctl story shot add --beat 27 --image ref.png --note "Wide on the desk, morning light"
splice ctl story shot add --beat 27 --note "Close on the hands"     # a shot without a picture
splice ctl story shot set --shot 41 --note "Wider"     # also: shot move --index I, shot remove
splice ctl cue add --at 9 --label "music lifts"
splice ctl view story                                  # show the person the board
```

- Draft the story with the person before cutting: a few beats, each with what
  happens and what to capture. Fit durations to the music or voice-over.
- Give each beat reference shots: a picture (any image, or a frame of a
  video) and a note per shot. `story read` lists them with the path of each
  picture, so you can look at them.
- Then fill the beats with footage (`story fill`, or record). `coverage`
  says which beats still need it.

### Recording the screen

```sh
splice ctl record sources                              # displays and windows (ids, apps, titles)
splice ctl record start --window Safari --beat 27      # or --display 1, or --region X,Y,W,H
splice ctl record start --display 1 --for 20 --mic --clicks --at 30
splice ctl record status
splice ctl record stop --wait                          # answers once the take is in the project
```

- The take goes into the beat, onto the timeline at `--at`, or (by default)
  just into the media panel.
- Ask before recording, and record only what the person wants: a window is
  usually better than the whole screen. `--hide` makes Splice step out of the
  shot when recording a whole display.
- The first recording needs Screen Recording permission. If a take comes out
  empty, say so and ask the person to grant it in System Settings → Privacy &
  Security.

## 7. Playback, the window, versions, export

```sh
splice ctl seek 4.2
splice ctl play | pause
splice ctl select --clip 3,4         # what the user sees selected; `select` alone clears it
splice ctl command zoom fit          # anything in the command menu, by its words
splice ctl say --text "Cut the pauses; 12s shorter now."
splice ctl versions                  # kept inside the project file
splice ctl save --label "rough cut"  # keep a version (only when asked)
splice ctl export ~/Movies/cut.mp4 --wait          # --draft (fast), --high
```

Export without `--wait` returns at once; `status` shows its `progress`.

### Operating the window

To check the interface itself, act as a person would. These go to Splice's
own window (no system permissions needed):

```sh
splice ctl ui                      # window size and scale, stage/timeline bounds, picture boxes
splice ctl click 160 52            # --double, --right, --shift/--cmd/--alt/--ctrl
splice ctl hover 750 20            # tooltips appear after a moment
splice ctl drag 300 1100 420 1100  # press, move, release
splice ctl scroll 450 900 --dy -120
splice ctl key cmd-/  escape       # any keys, in order
splice ctl type "Opening night"    # into whatever has the keyboard
```

Positions are window points from the top left: a Retina screenshot's pixels
divided by the `scale` that `ui` reports. Prefer the editing commands above
for changing the project; use these to see and test how the interface
behaves.

## 8. Taking requests from inside Splice

The user can message you from the editor (Cmd-Shift-A, or the speech-bubble
button in the title bar). To receive those, wait:

```sh
splice ctl wait --name Claude
```

- Run it in the background. It blocks until the user sends a message,
  prints it, and exits. While it waits, Splice shows `● Claude` in the title
  bar; after it returns, `Claude working` until you wait again.
- The message JSON has `message` (what they typed), `playhead`, `selection`
  (the clips they had selected, in full) and `media_selected`. "This clip",
  "here" and "now" mean those. When they wrote from a score or scene's
  editor, `editing` names it ("this music", "this scene") and says how to
  read and change it.
- Act, check with `frame`, then tell them in one line with `say`.
- Then run `splice ctl wait` again to stay connected. Stop when the user
  tells you to, or when Splice is no longer running.

## Rules

1. Edit through `splice ctl`, never by writing the `.splice` file: it is a
   live SQLite project the window owns.
2. One request, one focused change. Say what you did in a line.
3. Do not save versions, export, change the canvas or delete media unless
   asked.
4. Look at frames instead of guessing what is on screen.
