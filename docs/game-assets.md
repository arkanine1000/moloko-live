# The game's assets

The game is a Ren'Py 7.3.5 title (Steam app 1604000). `tools/rip.sh` extracts `game/archive.rpa` into `rip/raw`
(everything, including the `.rpy` scripts) and copies the images into `rip/frames`. All of it is gitignored.

## Resolution and colours

- All art is 1920x1080 on a 2x2 pixel grid: the native resolution is 960x540 and the game doubles it with nearest
  sampling. Three images have a few stray off-grid pixels; many `cg_mirror_gg` stills are drawn a full-resolution
  pixel off the grid or have one-pixel outlines, up to about 4 % of their pixels.
- Every image has 256 colours or fewer. The most colourful scene with its sky is `cg_ceiling` at 75; the rest are
  at 45 or fewer; the `mini_cg_*` scenes are two-colour indexed images.
- Nearly all alpha is 0 or 255. The exceptions are `cg_fall_close` (0.2 to 0.3 % of its pixels), `cg_firefly`
  (52 pixels) and two `cg_mirror` frames; in the game those edges blend over the sky.
- The in-game look is a composite: scene art over a sky, plus the interface. The wallpaper shows the first two.

## Scenes

The definitions are in `rip/raw/art.rpy` and `rip/raw/script.rpy`.

| Scene | Frames | Colours | Definition | Shown over |
|---|---|---|---|---|
| `cg_ceiling` | 16 | 154 | static base; `fireflies_anim`, 15 frames at 0.1 s | `sky3` |
| `cg_dream` | 8 | 11 | `LiveComposite` with eye and mouth layers | `sky3` |
| `cg_eyelash` | 4 | 6 | `Animation`: 6 s hold, then 3 frames at 0.05 s (a lash flicker) | packed over `sky2` |
| `cg_fall_close` | 10 | 33 | `Animation`, 10 frames at 0.1 s | `sky4` |
| `cg_fall_far` | 6 | 17 | `Animation`, 6 frames at 0.1 s | `sky1` |
| `cg_firefly` | 1 | 65 | static | `sky1` |
| `cg_floor` | 7 | 34 | `LiveComposite` with eye and mouth layers | `sky1` |
| `cg_mirror` | 93 | 31 | `LiveComposite`s over `cg_mirror_gg1..4`, pools of reflections | `sky1`, `sky2`, `sky3` |
| `cg_pills` | 1 | 12 | static | `sky2`, `sky4` |
| `mini_cg_1` | 21 | 2 | `Animation` at 0.1 s; frame 16 holds 4 s | — |
| `mini_cg_door` | 5 | 2 | `Animation` at 0.1 s | — |
| `mini_cg_eyes` | 17 | 2 | `eyes_fear`: holds of 1 to 11 s between 0.1 s frames, a glance 23 s into a 25.7 s cycle | — |
| `mini_cg_momp` | 6 | 2 | `Animation` at 0.05 s | — |
| `mini_cg_run` | 6 | 2 | `Animation` at 0.1 s | — |

The `mini_cg_*` art occupies a 512x284 native window at (224, 50) on a `#0d0d14` backdrop, so those packs are
cropped to it and the engine scales them to cover the screen (x3.80 on a 1080p screen, with under 1 % cut off).

## Skies

`images/skybox` holds 85 stills in four `choice:` pools, one still picked each time a sky is shown: `sky1` has
17, `sky2` 19, `sky3` 20 and `sky4` 29. All 85 use the same three colours. Every `show skyN at circle` also has
`truecenter zoom 1.01`, which hides the edges while the sky drifts by up to two pixels; see [drift.md](drift.md).

## Timings

- Blinking: `LiveComposite` scenes share one ATL. Eyes stay open for a `choice` of 2 to 6 s, then half-closed
  0.1 s, closed 0.2 s, half-open 0.1 s.
- Mouths flap at 0.1 s only while a line types (`WhileSpeaking`). Nobody speaks on a wallpaper, so the builder
  keeps only the silent image, which is nothing when the script leaves it out.
- Every hold is a multiple of 0.05 s, so 20 frames per second, the engine's default cap, lands each step on a
  frame boundary.

## The Ren'Py the tools understand

`renpy.py` parses only what the scenes use:

- `image X = "path"`
- `image X = Animation("a.png", 0.1, "b.png", 0.2, ...)`, which loops
- `image X = LiveComposite((w, h), (0, 0), "layer", (0, 0), WhileSpeaking("who", "talking", "silent"), ...)`,
  with all layers at offset (0, 0)
- ATL blocks: stills, pauses, `choice:` blocks of stills (a pool) or of numbers (alternative pauses), `repeat`
- `transform NAME:` blocks of `ease|linear D xoffset|yoffset V` lines ending in `repeat`, for drift

Anything else in a definition the tools need is an error, so a game update that adds a construct fails loudly.
