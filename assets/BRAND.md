# Splice

To *splice* film is to join two pieces of it: cut, line up, and tape. It is
the oldest edit there is, and most of what this editor does.

## The mark

- **Icon** (`icon.png`): a serif lowercase *s* in paper on an ink tile, beside
  a vermilion playhead, over a timeline whose played part is vermilion. It is
  Stet's sibling: the same letterform family and the same one accent.
- **Wordmark** (in the title bar): *splice* in Iowan Old Style, followed by a
  vermilion bar, the playhead.

Write the product name as **Splice** in prose and `splice` where it is the
command.

## Colour

| Role | Value |
|---|---|
| Ground (ink) | `#12151f` |
| Panels | `#1a1f2d` |
| Text (paper) | `#f3eee4` |
| Vermilion (playhead, selection, the one accent) | `#ef6a55` |

Vermilion marks where you are: the playhead, what is selected, what is
recording. Use it for nothing else.

## How the art is made

The icon is a Splice scene (`src/icon.json`), drawn by Splice itself:

```console
splice --still assets/src/icon.json assets/icon.png --size 1024x1024
```
