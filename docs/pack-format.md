# Scene packs

A pack is one directory per scene, written by `molokolive-pack` and read by the engine's `pack/manifest.rs`. The
default location is `$XDG_DATA_HOME/molokolive/packs`, else `~/.local/share/molokolive/packs`; both programs take
`--packs`. Packs are rebuilt from scratch on every run of the builder.

```
<packs>/<scene>/
  manifest.json          format below
  images/<hash>.png      8-bit indexed, index 0 transparent, PLTE = game colours (so they are viewable), deduplicated
  luts/<palette>.bin     256 x BGRA: index -> screen colour (index 0 unused)
  preview/<palette>.png  the native canvas with the first image of every layer
```

Images are named by a hash of their content, so identical frames share one file. A full 256-entry PLTE is written
so that Pillow keeps 8-bit indices.

## manifest.json, version 1

| Key | Meaning |
|---|---|
| `size [w, h]` | the native canvas. The game draws everything at 960x540 and doubles it; `mini_cg` scenes are cropped to their 512x284 window |
| `crop [x0, y0, x1, y1]` | where the canvas sits in the game's native 960x540 |
| `background` | the index shown where every layer is transparent, and around the canvas |
| `colours` | `"#rrggbb"` for index 1, 2, ... |
| `palettes` | `{name: LUT path}` |
| `layers` | bottom to top, each a *choices* or a *timeline* layer |

A **choices layer** shows one of its images, picked when the scene starts. A sky pool is one; a still image is a
pool of one.

```json
{"name": "sky", "choices": ["images/….png", …], "sources": ["images/skybox/12.png", …],
 "drift": {"period": 8.0, "steps": [[0.667, -1, 0], …]}}
```

`sources` are the game's own paths, used for the sky numbers `molokolive status` shows. `drift`, if present, gives
the layer's offset in native pixels from time `t` of each period on; see [drift.md](drift.md).

A **timeline layer** plays an animation:

```json
{"name": "cg_floor", "base": "images/….png",
 "steps": [{"hold": [2.0, 3.0, 4.0], "patch": null}, {"hold": [0.1], "patch": {…}}, …],
 "loop": 0, "wrap": {…}}
```

Step 0 shows `base`; entering step *i* > 0 applies its patch. Each visit holds for one of the listed durations,
picked at random. After the last step, `wrap` (the patch from the last image to step `loop`) is applied and
playback continues at `loop`; a null `loop` stops on the last step.

A **patch** is the pixels that change between two frames:

```json
{"rect": [x0, y0, x1, y1], "image": "images/….png", "dirty": [[x0, y0, x1, y1], …]}
```

`rect` is the bounding box of the change and `image` the layer's new pixels inside it. `dirty` lists the changed
pixels as rectangles on a 16 px tile grid, so the engine knows every dirty region before it draws. For a change
scattered over the canvas the tiles cover far fewer pixels than the box: the moving shadows of `mini_cg_run`
touch about a sixth of the window, while their bounding box is nearly all of it.

## How the builder samples the art

The game's images are 1920x1080 on an exact 2x2 pixel grid, so the builder keeps one pixel per block. A few
stills of the mirror reflections are drawn one full-resolution pixel off the grid; the builder tries every phase
and keeps the one with the fewest misfits, then keeps each block's top-left pixel for what still does not fit
(seam columns, one-pixel outlines). An image with more than 5 % of its pixels off the grid is rejected. See
[game-assets.md](game-assets.md) for which images are affected.

Soft alpha is snapped to 0 or 255 at a threshold of 128, and the builder reports how many pixels that touched.

Skies are zoomed by 1.01 about the centre with nearest sampling, as the game shows them (`truecenter zoom 1.01`),
which keeps their three colours.

## Scene table

`scenes.py` lists each scene as the layer stack `script.rpy` shows. A few entries differ from the game's names:

- `cg_mirror` is two scenes, `cg_mirror` (the idle pose over reflection pool `gg4`) and `cg_mirror_brush` (over `gg1`).
- `cg_dream` uses `cg_dream_s`, the variant with blinking eyes.
- `cg_ceiling` includes `fireflies_anim`, which the script adds after a choice.
- `cg_eyelash` is never shown by the script, only hidden. Its irises are transparent, so it is packed over `sky2`.
- `mini_cg_mom` is left out, `cg_balcony` too (1920x2752 with a 75 s vertical pan: the wrong shape for a wallpaper).
