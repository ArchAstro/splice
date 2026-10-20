---
name: splice
description: Work with the user inside their Splice video editor - see the project they have open, place, move, trim, split and delete clips on the timeline, import media, look at any frame, export, and take requests they send from the editor. Use when the user asks for help editing a video they have open in Splice, says "my edit/timeline/movie", or asks you to listen for requests from Splice.
---

# Working in the user's Splice editor

`splice ctl` talks to the running Splice window. Every reply is JSON with
`"ok"`, and an `"error"` that says what to do when it is false. Each edit is
one undo step for the user, shows at once, and is autosaved into their
project file. Nothing you do saves a version or exports unless you ask it to.

If `splice` is not on the PATH, use the binary from the repository:
`target/release/splice` (build it with `make build-release`).

## 1. See what is open

```sh
splice ctl status    # project, canvas, duration, playhead, selection, export progress
splice ctl read      # everything: media and every track and clip, with ids and times
```

- If the reply says Splice is not running, tell the user; do not start it
  yourself unless they ask.
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

## 5. Playback, the window, versions, export

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

## 6. Taking requests from inside Splice

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
  "here" and "now" mean those.
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
