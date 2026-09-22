# Palettes

A palette maps the game's colours to the colours on screen. The pack builder resolves each scene's colours
through the palette into a 256-entry lookup table, so the engine only ever swaps tables. `colour.py` holds the
maths; `molokolive-recolour` fits new palettes.

## Kinds

| Kind | File | How a colour maps |
|---|---|---|
| tone list | `palettes/NAME.hex`, one `#rrggbb` per line | to the entry nearest in OKLab, with lightness weighed far above hue |
| lift rule | `palettes/NAME.json` with `"rule"` | through a curve in OKLab/OKLCH, the same for every scene |
| `none` | | unchanged |

**Tone mapping** keeps each colour's lightness and takes the palette's hue. The hue weight is 0.01: for the
near-monochrome palettes this is for, pure lightness matching reproduced a hand-made reference best, and hue only
breaks ties between entries of the same lightness.

**A lift rule** is three curves, applied to every colour:

```
lightness  L' = l0 + (1 - l0) * L^gamma
chroma     C' = c0 + c1 * C + c2 * L' (1 - L')
hue        h' = h0 + h1 * L'            (degrees)
```

`l0` is the black level: the game's blacks come out at that lightness, which is what gives the wallpaper its
lifted, hazy look.

## The default: neutral-lift

`neutral-lift` was fitted by `molokolive-recolour` to two scenes recoloured by hand, `cg_dream` and
`cg_fall_far`, then adjusted:

- one black level for every scene, the lightest of the two fits (each recolour was painted independently and
  got its own);
- the hue curve taken from the `firefly-neutral` tone list, weighted by how much of a reference image each entry
  covers, because the fitted hue read too green;
- chroma multiplied by 1.25.

The palette file records the fitted values, the adjustments, and each scene's painted colour map for reference.

## Fitting a palette

1. Paint over a 1920x1080 screenshot of a scene and save it under `rip/recolours/`.
2. `molokolive-recolour rip/recolours/mine.png:cg_dream --name mine`, with `--hue-from`, `--hue-weights` and
   `--chroma-scale` as wanted.
3. `molokolive-pack --palette mine neutral-lift`, then `molokolive --palette mine`.

The fitter aligns each recolour with its pack first: it tries every combination of the CG layers' frames and,
for scenes over a sky, every sky still of every pool at zoom 1 or 1.01 with up to two pixels of drift, and keeps
the candidate where each game colour gets the most consistent painted colour. Only colour pairs painted
consistently over enough pixels take part in the fit. The tool prints how far the rule and the baseline palette
are from the painted colours, as mean and maximum OKLab distance per scene.

## Where the tone list came from

`firefly-neutral.hex` is the palette of a hand-graded wallpaper. Its grade is not a per-colour map of any game
frame, and its sky matches none of the game's 85 stills, which is why the wallpaper uses the game's own sky pools
under a fitted rule rather than trying to reproduce the picture.
