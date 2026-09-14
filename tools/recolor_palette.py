#!/usr/bin/env python3
"""Build a colour-map palette from a hand-recoloured Milk-chan sprite.

  python tools/recolor_palette.py rip/recolours/neutral_1-temp.png --sprite gg_neutral_1_oa --name milkchan-neutral

Recolours are derived from the game's art, so keep them under rip/ (gitignored).

1. Map: the recolour is aligned with the game's own render of --sprite (eyes open, mouth closed), and every
   game colour gets the colour it was painted.
2. Hue match to firefly-neutral: each painted colour keeps its lightness and chroma but takes the hue that
   firefly-neutral gave the same game colour in cg_firefly (nearest within 0.08 OKLab), else the hue of the
   palette entry nearest in lightness. A near-grey reference (C < 0.015) has no meaningful hue, so its
   chroma is taken as well.
3. Fill: game colours the recolour never shows (other sprites' shades, the talking mouth, the textbox) copy
   their nearest painted colour, shifted by the lightness difference.

Writes palettes/<name>.json with the complete game-colour -> colour map.
"""
import argparse, json, sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "apps"))
import milkchan as mc  # noqa: E402  (shared sprite loading + OKLab helpers)

# A game colour borrows firefly-neutral's choice only for (nearly) the same colour, and only where the wallpaper
# paints that colour consistently: a loose match sent the dark-red hair shade to the maroon hand outline,
# which the wallpaper splits between blue and mauve.
REF_NEAR, REF_SHARE, GREY_C, CHROMATIC, HUE_WINDOW = 0.03, 0.4, 0.015, 0.012, 0.08


def lab(cols):
    return mc.srgb_to_oklab(np.asarray(cols, float).reshape(-1, 3))


def hexc(c):
    return "#" + bytes(int(x) for x in c).hex()


def key24(rgb):
    rgb = rgb.astype(np.uint64)
    return (rgb[:, 0] << 16) | (rgb[:, 1] << 8) | rgb[:, 2]


def unkey24(k):
    return (int(k >> 16) & 255, int(k >> 8) & 255, int(k) & 255)


def dominant_map(src, dst, mask, min_px=1, min_share=0.0):
    """Most common dst colour for each src colour under mask -> ({src: dst}, worst purity)."""
    pair = (key24(src[mask][:, :3]) << 24) | key24(dst[mask][:, :3])
    pairs, counts = np.unique(pair, return_counts=True)
    best, total = {}, {}
    for p, n in zip(pairs, counts):
        s, d = int(p >> 24), int(p & 0xFFFFFF)
        total[s] = total.get(s, 0) + int(n)
        if n > best.get(s, (0, 0))[0]:
            best[s] = (int(n), d)
    mapping = {unkey24(s): unkey24(d) for s, (n, d) in best.items() if total[s] >= min_px and n / total[s] >= min_share}
    purity = min(n / total[s] for s, (n, _) in best.items())
    return mapping, purity


def render_reference(rip, name):
    sprite = {s.name: s for s in mc.load_sprites(rip / "raw")}.get(name)
    if sprite is None:
        sys.exit(f"unknown sprite {name} (see apps/milkchan.py --list)")
    assets, out = mc.Assets(rip / "frames"), Image.new("RGBA", mc.SCREEN)
    for kind, *rest in sprite.layers:
        rel = rest[0] if kind in ("static", "mouth") else (rest[0][0].path if rest[0] else None)  # eyes open, mouth closed
        if rel:
            out.alpha_composite(Image.open(next(p for r in assets.roots if (p := r / rel).is_file())).convert("RGBA"))
    return np.array(out)


def game_colours(rip):
    """Every colour (any alpha > 0) in all sprite layers plus the textbox."""
    files = sorted((rip / "frames" / "images" / "sprites").rglob("*.png")) + [rip / "frames" / "gui" / "textbox.png"]
    keys = set()
    for f in files:
        a = np.array(Image.open(f).convert("RGBA"))
        keys.update(np.unique(key24(a[a[..., 3] > 0][:, :3])).tolist())
    return [unkey24(k) for k in sorted(keys)]


def match_hue(game, painted, refs, pal):
    """Keep the painted lightness and chroma; take firefly-neutral's hue -> (colour, provenance)."""
    L, a, b = lab(painted)[0]
    C = float(np.hypot(a, b))
    keys = list(refs)
    d = np.linalg.norm(lab(keys) - lab(game)[0], axis=1)
    if d.min() < REF_NEAR:
        ref = refs[keys[int(d.argmin())]]
        _, ra, rb = lab(ref)[0]
        rc, h, how = float(np.hypot(ra, rb)), float(np.arctan2(rb, ra)), f"firefly map {hexc(ref)}"
        if rc < GREY_C:  # near-grey reference: its hue is noise, so take its low chroma too
            C, how = min(C, rc), how + " (grey)"
    else:
        # Chroma-weighted mean hue of the palette around this lightness; one nearest entry can be an outlier.
        pl = lab(pal)
        pc = np.hypot(pl[:, 1], pl[:, 2])
        w = pc * (pc > CHROMATIC) * np.exp(-((pl[:, 0] - L) / HUE_WINDOW) ** 2) / pc.clip(1e-9)
        h, how = float(np.arctan2((w * pl[:, 2]).sum(), (w * pl[:, 1]).sum())), f"palette hue near L {L:.2f}"
    return tuple(int(x) for x in mc.oklab_to_srgb(np.array([[L, C * np.cos(h), C * np.sin(h)]]))[0]), how


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("recolour", type=Path, help="1920x1080 recoloured sprite, same framing as the game layers")
    ap.add_argument("--sprite", default="gg_neutral_1_oa")
    ap.add_argument("--name", default="milkchan-neutral")
    ap.add_argument("--rip", type=Path, default=mc.RIP)
    ap.add_argument("--hue-palette", type=Path, default=ROOT / "palettes" / "firefly-neutral.hex")
    ap.add_argument("--hue-cg", type=Path, default=mc.RIP / "frames" / "images" / "cg_firefly" / "1.png")
    ap.add_argument("--hue-wallpaper", type=Path, default=Path.home() / "Pictures" / "wallpapers" / "firefly-neutral.png")
    args = ap.parse_args()

    ref = render_reference(args.rip, args.sprite)
    rec = np.array(Image.open(args.recolour).convert("RGBA"))
    if rec.shape != ref.shape:
        sys.exit(f"recolour is {rec.shape[1]}x{rec.shape[0]}, expected the full 1920x1080 layer frame")
    overlap = ((ref[..., 3] > 0) & (rec[..., 3] > 0)).sum() / max(1, ((ref[..., 3] > 0) | (rec[..., 3] > 0)).sum())
    painted, purity = dominant_map(ref, rec, (ref[..., 3] == 255) & (rec[..., 3] > 0))
    print(f"alignment IoU {overlap:.4f} · painted game colours {len(painted)} · worst purity {purity:.2f}")
    if overlap < 0.98:
        sys.exit("recolour does not line up with the reference sprite")

    cg = np.array(Image.open(args.hue_cg).convert("RGBA"))
    wall = np.array(Image.open(args.hue_wallpaper).convert("RGBA"))
    refs, _ = dominant_map(cg, wall, cg[..., 3] == 255, min_px=500, min_share=REF_SHARE)
    pal = [tuple(bytes.fromhex(h)) for h in args.hue_palette.read_text().split()]

    matched, report = {}, []
    for game, colour in sorted(painted.items(), key=lambda kv: lab(kv[1])[0][0]):
        new, how = match_hue(game, colour, refs, pal)
        matched[game] = new
        report.append((game, colour, new, how, float(np.linalg.norm(lab(new)[0] - lab(colour)[0]))))

    mapping, keys = dict(matched), list(matched)
    src_lab, dst_lab = lab(keys), lab([matched[k] for k in keys])
    derived = []
    for c in game_colours(args.rip):
        if c in mapping:
            continue
        g = lab(c)[0]
        i = int(np.linalg.norm(src_lab - g, axis=1).argmin())
        t = dst_lab[i].copy()
        t[0] = float(np.clip(t[0] + g[0] - src_lab[i][0], 0.0, 1.0))
        mapping[c] = tuple(int(x) for x in mc.oklab_to_srgb(t[None])[0])
        derived.append(c)

    # Viewer ground: a step darker than the darkest colour that is a real mass on the sprite (>= 1% of its
    # pixels), so the near-black sweater and textbox fill stay visible. The global darkest map entry comes
    # from stray 4-pixel shades and would give pure black.
    opaque = ref[ref[..., 3] == 255][:, :3]
    keys, counts = np.unique(key24(opaque), return_counts=True)
    masses = [matched[unkey24(int(k))] for k, n in zip(keys, counts) if n >= 0.01 * len(opaque) and unkey24(int(k)) in matched]
    darkest = min(lab(masses), key=lambda v: v[0]).copy()
    darkest[0] = max(0.0, darkest[0] - 0.07)
    ground = hexc(mc.oklab_to_srgb(darkest[None])[0])

    out = ROOT / "palettes" / f"{args.name}.json"
    out.write_text(json.dumps({
        "name": args.name,
        "ground": ground,
        "description": f"Hand recolour of {args.sprite}, hue-matched to {args.hue_palette.stem}; "
                       f"unpainted game colours derived from their nearest painted colour.",
        "painted": {hexc(g): {"painted": hexc(c), "final": hexc(n), "hue": how, "dE": round(de, 4)}
                    for g, c, n, how, de in report},
        "map": {hexc(k): hexc(v) for k, v in sorted(mapping.items())},
    }, indent=2) + "\n")

    print("game     painted  -> final    dE      hue from")
    for g, c, n, how, de in report:
        print(f"{hexc(g)}  {hexc(c)} -> {hexc(n)}  {de:.4f}  {how}")
    print(f"derived {len(derived)} unpainted colours · {len(mapping)} total -> {out}")


if __name__ == "__main__":
    main()
