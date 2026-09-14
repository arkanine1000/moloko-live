# molokolive

A live wallpaper engine themed on Nikita Kryukov's *Milk inside a bag of milk* games, meant to be extremely
light on CPU, GPU and battery (palette cycling on indexed art, low internal resolution, low frame rate, pausing
when the desktop is hidden).

**The wallpaper engine does not exist yet.** So far this repository holds the groundwork: tools that extract
and study the game's assets, and a Milk-chan demo.

## Milk-chan demo

`apps/milkchan.py` is a playground for testing and reference only, not the wallpaper engine. It rebuilds
Milk-chan from the game's own sprite definitions and reproduces the game's behaviour closely enough to
compare against. It covers:

- Pose, emotion and mood selection, idle blink, and frame-by-frame stepping.
- The game's say screen: textbox, Retro Gaming font, 30 cps typewriter, and the talk logic (mouth flaps and a
  looping sound while typing, 0.3 s fadeout after).
- Runtime palettes (`palettes/`), including exact colour-map palettes built from hand recolours.
- Sharp fill or pixel-perfect scaling.

```sh
python apps/milkchan.py           # GUI; key help is in the window title
python apps/milkchan.py --list    # all sprites
python apps/milkchan.py --render out.png --pose arms_down --emotion smile --mood 2
```

## Layout

| Path | What |
|---|---|
| `apps/milkchan.py` | Milk-chan demo (playground) |
| `apps/milkchan_lines.json` | Demo dialogue picks, as line ids only |
| `tools/rip.sh` | Extract the game's `archive.rpa` into `rip/` (videos skipped) |
| `tools/dialogue.py` | Milk-chan's spoken English dialogue, tagged with the sprite shown |
| `tools/palettes.py` | Extract, swap and neutralize PNG palettes |
| `tools/recolor_palette.py` | Build a colour-map palette from a hand-recoloured sprite |
| `palettes/` | Palettes: `*.hex` lists, `*.json` colour maps, swatches |
| `rip/` | Extracted game assets, dialogue and recolours (**gitignored**, never commit) |

## Setup

The Python environment comes from direnv and pyenv (`.envrc`: `layout pyenv 3.14.6`). Python needs tkinter.

```sh
pyenv install 3.14.6
direnv allow
pip install -r requirements.txt
```

## Game assets

No game assets are included. You need your own copy of *Milk outside a bag of milk outside a bag of milk*
(Steam app 1604000, Ren'Py 7.3.5):

```sh
tools/rip.sh                 # -> rip/raw (full archive), rip/frames (images only)
python tools/dialogue.py     # -> rip/dialogue/milkchan_en.{jsonl,txt}
```

Everything extracted from the game stays in `rip/`, which is gitignored. Recolours of game art belong there too.

## Commits

Scoped [Conventional Commits](https://www.conventionalcommits.org/) across the repository, e.g.
`feat(milkchan): add dialogue mode` or `fix(palettes): skip metadata json`.
