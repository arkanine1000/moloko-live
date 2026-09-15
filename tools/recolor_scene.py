#!/usr/bin/env python3
"""Build a lift-rule palette from hand-recoloured scene frames.

  python tools/recolor_scene.py rip/recolours/dream.png:cg_dream rip/recolours/fall_far.png:cg_fall_far --name neutral-lift

Needs the scenes' packs (tools/pack.py). Recolours are derived from the game's art, so keep them under rip/.

1. Align: each recolour (1920x1080, the game's framing) is matched against its pack. Every combination of the CG
   layers' images and timeline frames is tried, and, for scenes over a sky, every skybox still of every pool at
   zoom 1 or 1.01 with up to +-2 native px of drift: an eyeballed recolour needn't use the scene's own pool. The
   best candidate is the one where each game colour gets the most consistent painted colour.
2. Painted map per scene: game colour -> its dominant painted colour, kept in the palette as reference.
3. Rule, in OKLab/OKLCH, fitted over every distinct painted colour:
     lightness  L' = L0 + (1 - L0) * L^gamma
     chroma     C' = c0 + c1 * C + c2 * L' (1 - L')
     hue        h' = h0 + h1 * L'  (degrees)
   The fit gives each recolour its own black level L0 (they were eyeballed separately) and shares the rest. The
   palette then uses one black level for every scene, the lightest recolour's, for simplicity.

Adjustments after the fit, recorded in the palette:
  --hue-from PALETTE.hex   take the hue curve h' = h0 + h1 * L' from a tone palette instead (fitted over its
                           chromatic entries, weighted by use in --hue-weights, an indexed image with that palette)
  --chroma-scale K         multiply chroma

Writes palettes/<name>.json. tools/pack.py turns the rule into a LUT per scene.
"""
import argparse, itertools, json, os, re, sys, textwrap
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "apps"))
import milkchan as mc  # noqa: E402  (OKLab helpers, tone-mapped palettes for the baseline)

RIP = ROOT / "rip"
SKY_COLOURS = [(13, 13, 20), (82, 38, 62), (172, 50, 50)]  # every skybox still uses exactly these
MIN_SHARE, MIN_PX = 0.9, 20  # a pair counts for the fit only if painted consistently on enough pixels


class HelpFormatter(argparse.RawDescriptionHelpFormatter):
    """Help at 100 columns that never breaks a word at a hyphen (palette and file names stay whole)."""

    def __init__(self, prog):
        super().__init__(prog, width=100, max_help_position=30)

    def _split_lines(self, text, width):
        return textwrap.wrap(" ".join(text.split()), width, break_on_hyphens=False)


def default_packs():
    """Where molokolive looks for scene packs: $XDG_DATA_HOME/molokolive/packs, else ~/.local/share/molokolive/packs."""
    return Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share") / "molokolive" / "packs"


def key24(rgb):
    rgb = np.asarray(rgb).astype(np.int64)
    return (rgb[..., 0] << 16) | (rgb[..., 1] << 8) | rgb[..., 2]


def hexc(rgb):
    return "#" + bytes(int(v) for v in rgb).hex()


def zoom_nearest(a, z):
    h, w = a.shape[:2]
    ys = np.floor((np.arange(h) + 0.5 - h / 2) / z + h / 2).astype(int).clip(0, h - 1)
    xs = np.floor((np.arange(w) + 0.5 - w / 2) / z + w / 2).astype(int).clip(0, w - 1)
    return a[ys][:, xs]


def layer_images(pack, layer):
    """Every image a pack layer can show: its choices, or each frame of its timeline."""
    if "choices" in layer:
        return [(src, np.array(Image.open(pack / c))) for src, c in zip(layer["sources"], layer["choices"])]
    cur = np.array(Image.open(pack / layer["base"]))
    out = [("frame 0", cur.copy())]
    for i, step in enumerate(layer["steps"][1:], 1):
        if p := step["patch"]:
            x0, y0, x1, y1 = p["rect"]
            cur[y0:y1, x0:x1] = np.array(Image.open(pack / p["image"]))
        out.append((f"frame {i}", cur.copy()))
    return out


def sky_stills():
    """(pool, file, zoom) -> native index map into SKY_COLOURS, for every still of every pool."""
    art = (RIP / "raw" / "art.rpy").read_text(encoding="utf-8")
    keys = key24(np.array(SKY_COLOURS))
    stills = {}
    for pool in re.findall(r"^\s*image\s+(sky\d+)\s*:", art, re.M):
        block = re.search(rf"image {pool}:\n(.*?)(?=\n\s*image |\Z)", art, re.S).group(1)
        for rel in re.findall(r'"images/(skybox/\d+\.png)"', block):
            a = np.array(Image.open(RIP / "frames" / "images" / rel).convert("RGB"))[::2, ::2]
            for z in (1.0, 1.01):
                stills[pool, rel, z] = np.searchsorted(keys, key24(zoom_nearest(a, z) if z != 1.0 else a))
    return stills


def dominant(counts):
    """bincount matrix (source x painted index) -> per source: (painted index, share, pixels)."""
    total = counts.sum(1)
    return {int(s): (int(counts[s].argmax()), counts[s].max() / total[s], int(total[s])) for s in np.flatnonzero(total)}


def align(recolour, scene, stills, packs):
    img = Image.open(recolour)
    if img.mode != "P":
        img = img.convert("RGB").quantize(256, method=Image.Quantize.FASTOCTREE, dither=Image.Dither.NONE)
    if img.size != (1920, 1080):
        sys.exit(f"{recolour}: {img.size[0]}x{img.size[1]}, expected the game's 1920x1080 framing")
    painted = np.array(img)[::2, ::2].astype(np.int64)
    palette = np.array(img.getpalette()[:768], np.uint8).reshape(-1, 3)
    pack = packs / scene
    manifest = json.loads((pack / "manifest.json").read_text())
    if manifest["size"] != [960, 540]:
        sys.exit(f"{scene}: cropped scenes are not supported yet")
    colours = np.array([[0, 0, 0]] + [[int(c[i:i + 2], 16) for i in (1, 3, 5)] for c in manifest["colours"]], np.uint8)
    cg_layers = [l for l in manifest["layers"] if l["name"] != "sky"]
    has_sky = len(cg_layers) < len(manifest["layers"])

    best = None
    for combo in itertools.product(*(layer_images(pack, l) for l in cg_layers)):
        cg = np.zeros(painted.shape, np.uint8)
        for _, idx in combo:
            cg = np.where(idx != 0, idx, cg)
        mask = cg != 0 if has_sky else np.ones(painted.shape, bool)
        counts = np.bincount(cg[mask].astype(np.int64) * 256 + painted[mask], minlength=65536).reshape(256, 256)
        purity = counts.max(1).sum() / mask.sum()
        if best is None or purity > best[0]:
            best = (purity, [name for name, _ in combo], cg, counts)
    purity, frames, cg, counts = best
    cg_map = {hexc(colours[s]): (hexc(palette[p]), share, px) for s, (p, share, px) in dominant(counts).items()}
    print(f"{recolour.name} -> {scene}: {', '.join(frames)}; CG purity {purity:.1%}")

    sky_map = {}
    if has_sky:
        sky = cg == 0
        best = None
        for (pool, rel, z), still in stills.items():
            for dx, dy in itertools.product(range(-2, 3), repeat=2):
                shifted = np.roll(still, (dy, dx), (0, 1)) if dx or dy else still
                c = np.bincount(shifted[sky].astype(np.int64) * 256 + painted[sky], minlength=3 * 256).reshape(3, 256)
                p = c.max(1).sum() / sky.sum()
                if best is None or p > best[0]:
                    best = (p, pool, rel, z, dx, dy, c)
        p, pool, rel, z, dx, dy, c = best
        sky_map = {hexc(SKY_COLOURS[s]): (hexc(palette[q]), share, px) for s, (q, share, px) in dominant(c).items()}
        print(f"  sky: {rel} ({pool}) zoom {z} drift ({dx},{dy}); purity {p:.1%}")
    return cg_map, sky_map


def fit(scenes):
    """scenes: {scene: [(game hex, painted hex)]} -> rule dict, per-scene L0, and the Lab arrays for scoring."""
    names = sorted(scenes)
    rows = [(names.index(s), g, p) for s in names for g, p in scenes[s]]
    sid = np.array([r[0] for r in rows])
    G = mc.srgb_to_oklab(np.array([[int(r[1][i:i + 2], 16) for i in (1, 3, 5)] for r in rows], float))
    P = mc.srgb_to_oklab(np.array([[int(r[2][i:i + 2], 16) for i in (1, 3, 5)] for r in rows], float))
    gL, gC = np.clip(G[:, 0], 0, 1), np.hypot(G[:, 1], G[:, 2])
    pL, pC = P[:, 0], np.hypot(P[:, 1], P[:, 2])
    ph = np.degrees(np.arctan2(P[:, 2], P[:, 1])) % 360

    best = None
    for gamma in np.linspace(0.6, 2.0, 141):
        x = gL ** gamma
        # For a fixed gamma, L' - x = L0 (1 - x) is linear in L0: least squares per scene.
        l0 = np.array([np.clip(((pL - x) * (1 - x))[sid == s].sum() / ((1 - x) ** 2)[sid == s].sum(), 0, 0.6)
                       for s in range(len(names))])
        rms = np.sqrt(np.mean((l0[sid] + (1 - l0[sid]) * x - pL) ** 2))
        if best is None or rms < best[0]:
            best = (rms, gamma, l0)
    _, gamma, l0 = best
    Lp = l0[sid] + (1 - l0[sid]) * gL ** gamma
    chroma, *_ = np.linalg.lstsq(np.column_stack([np.ones(len(pC)), gC, Lp * (1 - Lp)]), pC, rcond=None)
    hue, *_ = np.linalg.lstsq(np.column_stack([np.ones(len(ph)), Lp]), ph, rcond=None)
    rule = {"gamma": round(float(gamma), 3), "l0": round(float(l0.mean()), 4),
            "chroma": [round(float(v), 4) for v in chroma], "hue": [round(float(v), 2) for v in hue]}
    return rule, {s: round(float(v), 4) for s, v in zip(names, l0)}, G, P, sid


def apply_rule(rgb, rule, l0):
    """(n, 3) uint8 game colours -> (n, 3) uint8 under the lift rule."""
    lab = mc.srgb_to_oklab(np.asarray(rgb, float).reshape(-1, 3))
    L, C = np.clip(lab[:, 0], 0, 1), np.hypot(lab[:, 1], lab[:, 2])
    Lp = l0 + (1 - l0) * L ** rule["gamma"]
    c0, c1, c2 = rule["chroma"]
    Cp = np.clip(c0 + c1 * C + c2 * Lp * (1 - Lp), 0, None)
    h = np.radians(rule["hue"][0] + rule["hue"][1] * Lp)
    return mc.oklab_to_srgb(np.column_stack([Lp, Cp * np.cos(h), Cp * np.sin(h)]))


def palette_hue(hex_path, weights_image=None):
    """Hue curve [h0, h1] (degrees, h = h0 + h1 * L) of a tone palette's chromatic entries."""
    colours = np.array([[int(c[i:i + 2], 16) for i in (0, 2, 4)] for c in Path(hex_path).read_text().split()], np.uint8)
    weights = np.ones(len(colours))
    if weights_image:
        img = Image.open(weights_image)
        used = np.array(img.getpalette()[:len(colours) * 3], np.uint8).reshape(-1, 3) if img.mode == "P" else None
        if used is None or len(used) != len(colours) or (used != colours).any():
            sys.exit(f"{weights_image}: not an indexed image with the palette of {hex_path}")
        weights = np.bincount(np.array(img).ravel(), minlength=len(colours))[:len(colours)].astype(float)
    lab = mc.srgb_to_oklab(colours.astype(float))
    chroma = np.hypot(lab[:, 1], lab[:, 2])
    hue = np.degrees(np.arctan2(lab[:, 2], lab[:, 1])) % 360
    keep = (chroma > 0.01) & (weights > 0)  # near-greys have no meaningful hue
    h1, h0 = np.polyfit(lab[keep, 0], hue[keep], 1, w=weights[keep])
    return [round(float(h0), 2), round(float(h1), 2)]


def main():
    ap = argparse.ArgumentParser(
        prog="tools/recolor_scene.py",
        description="Fit a palette to scene screenshots you recoloured by hand, for tools/pack.py to use.\n"
                    "(How the fit works is described at the top of this file.)",
        epilog="""example:
  python tools/recolor_scene.py rip/recolours/dream.png:cg_dream rip/recolours/fall_far.png:cg_fall_far \\
      --name neutral-lift --hue-from palettes/firefly-neutral.hex \\
      --hue-weights ~/Pictures/wallpapers/firefly-neutral.png --chroma-scale 1.25
  python tools/pack.py --palette neutral-lift""",
        formatter_class=HelpFormatter,
    )
    ap.add_argument("recolours", nargs="+", metavar="IMAGE:SCENE",
                    help="a recoloured 1920x1080 screenshot and the scene it shows, e.g. rip/recolours/dream.png:cg_dream")
    ap.add_argument("--name", default="neutral-lift", help="palette to write, as palettes/NAME.json (default: %(default)s)")
    ap.add_argument("--baseline", metavar="PALETTE", default="firefly-neutral",
                    help="palette to compare the fit against (default: %(default)s)")
    ap.add_argument("--hue-from", type=Path, metavar="PALETTE.hex", help="take the hue from this palette instead of the recolours")
    ap.add_argument("--hue-weights", type=Path, metavar="IMAGE",
                    help="an image using --hue-from's palette; its colours count by how much of the image they cover")
    ap.add_argument("--chroma-scale", type=float, metavar="K", default=1.0, help="multiply the saturation by K (default: 1)")
    ap.add_argument("--packs", type=Path, metavar="DIR", default=default_packs(),
                    help="scene packs to align against (default: ~/.local/share/molokolive/packs)")
    args = ap.parse_args()

    stills = sky_stills()
    maps, fit_pairs = {}, {}
    for spec in args.recolours:
        path, _, scene = spec.rpartition(":")
        if not path or not scene:
            sys.exit(f"{spec}: expected RECOLOUR.png:SCENE")
        cg_map, sky_map = align(Path(path), scene, stills, args.packs)
        exact = {g: p for g, (p, _, _) in sky_map.items()} | {g: p for g, (p, _, _) in cg_map.items()}
        maps[scene] = {"recolour": str(Path(path)), "painted": dict(sorted(exact.items()))}
        # Fit on consistent pairs; a colour painted differently on the sky than on the CG counts once per layer.
        fit_pairs[scene] = [(g, p) for m in (cg_map, sky_map) for g, (p, share, px) in m.items() if share >= MIN_SHARE and px >= MIN_PX]

    rule, l0, G, P, sid = fit(fit_pairs)
    lightest = max(l0, key=l0.get)
    rule["l0"] = l0[lightest]
    adjustments = {}
    if args.hue_from:
        adjustments["hue"] = f"from {args.hue_from.name}" + (f", weighted by {args.hue_weights.name}" if args.hue_weights else "")
        adjustments["fitted_hue"] = rule["hue"]
        rule["hue"] = palette_hue(args.hue_from, args.hue_weights)
    if args.chroma_scale != 1.0:
        adjustments["chroma_scale"] = args.chroma_scale
        rule["chroma"] = [round(c * args.chroma_scale, 4) for c in rule["chroma"]]
    for scene in maps:
        maps[scene] = {"fitted_l0": l0[scene], **maps[scene]}
    names = sorted(fit_pairs)
    game = np.array([[int(g[i:i + 2], 16) for i in (1, 3, 5)] for s in names for g, _ in fit_pairs[s]], np.uint8)
    dE = np.linalg.norm(mc.srgb_to_oklab(apply_rule(game, rule, rule["l0"]).astype(float)) - P, axis=1)
    base = mc.load_palettes().get(args.baseline)
    print(f"rule: L' = L0 + (1-L0) L^{rule['gamma']} with L0 {rule['l0']} ({lightest}, the lightest; fitted "
          f"{', '.join(f'{s} {v}' for s, v in l0.items())}); "
          f"C' = {rule['chroma'][0]} + {rule['chroma'][1]} C + {rule['chroma'][2]} L'(1-L'); h' = {rule['hue'][0]} + {rule['hue'][1]} L'")
    for i, s in enumerate(names):
        line = f"  {s}: rule vs painted mean dE {dE[sid == i].mean():.4f}, max {dE[sid == i].max():.4f}"
        if base is not None:
            dE_base = np.linalg.norm(mc.srgb_to_oklab(base.map(game[sid == i]).astype(float)) - P[sid == i], axis=1)
            line += f"; {args.baseline} {dE_base.mean():.4f}, max {dE_base.max():.4f}"
        print(line)

    out = ROOT / "palettes" / f"{args.name}.json"
    out.write_text(json.dumps({
        "name": args.name,
        "description": "Lift rule fitted to hand recolours of " + ", ".join(sorted(maps))
                       + ": lightness lifted from a per-scene black level, blue-grey hue, low chroma. "
                         "Built by tools/recolor_scene.py; tools/pack.py makes the LUTs.",
        "rule": rule,
        **({"adjustments": adjustments} if adjustments else {}),
        "scenes": maps,
    }, indent=2) + "\n")
    print(f"-> {out.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
