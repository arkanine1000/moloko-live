#!/usr/bin/env python3
"""Build scene packs for the wallpaper engine (engine/).

  python tools/pack.py cg_firefly
  python tools/pack.py cg_firefly --palette firefly-neutral milkchan-neutral none

v0 (a first spike): static scenes only, with their layer stacks written out in SCENES below from
rip/raw/script.rpy. Script parsing and animations come with v1.

Writes rip/packs/<scene>/ (game-derived, so under the gitignored rip/):
  scene.txt               size, scale and layers bottom to top; a layer with several images is a random choice
  layers/<name>.png       8-bit indexed, native 960x540, index 0 transparent, PLTE = game colours (viewable)
  layers/<pool>/<n>.png   one per still of a random-choice pool (e.g. the sky1 skyboxes)
  luts/<palette>.bin      256 x BGRA: scene index -> screen colour; index 0 is unused
  preview/<palette>.png   native-size composite with the first choice of each layer, to eyeball without X

All layers of a scene share one index space, so a palette is only a LUT and never touches layer data.
"""
import argparse, re, sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "apps"))
import milkchan as mc  # noqa: E402  (tone mapping and palette loading)

RIP = ROOT / "rip"
NATIVE = (960, 540)
SCALE = 2

# script.rpy: `show sky1 at circle: truecenter zoom 1.01` then `show cg_firefly`. The zoom only hides the edges
# while `circle` drifts the sky by +-2 px; drift is not in v0, but the zoom is kept so the framing matches.
SCENES = {
    "cg_firefly": [
        {"name": "sky", "pool": "sky1", "zoom": 1.01},
        {"name": "cg_firefly", "image": "images/cg_firefly/1.png"},
    ],
}


def pool_images(pool):
    """Stills of an `image <pool>:` block of `choice:` entries in art.rpy, in script order."""
    lines = (RIP / "raw" / "art.rpy").read_text(encoding="utf-8").splitlines()
    start = next(i for i, l in enumerate(lines) if re.match(rf"\s*image\s+{pool}\s*:", l))
    indent = len(lines[start]) - len(lines[start].lstrip())
    paths = []
    for line in lines[start + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) <= indent:
            break
        if m := re.fullmatch(r'\s*"([^"]+\.png)"\s*', line):
            paths.append(m.group(1))
    return paths


def native(rel):
    """1920x1080 game image -> native 960x540 RGBA with binary alpha (soft pixels snapped) -> (array, snapped)."""
    a = np.array(Image.open(RIP / "frames" / rel).convert("RGBA"))
    if a.shape[:2] != (NATIVE[1] * SCALE, NATIVE[0] * SCALE):
        sys.exit(f"{rel}: {a.shape[1]}x{a.shape[0]}, expected {NATIVE[0] * SCALE}x{NATIVE[1] * SCALE}")
    n = a[::SCALE, ::SCALE]
    if not (np.repeat(np.repeat(n, SCALE, 0), SCALE, 1) == a).all():
        sys.exit(f"{rel}: not on an exact {SCALE}x{SCALE} pixel grid")
    soft = int(((n[..., 3] > 0) & (n[..., 3] < 255)).sum())
    n = n.copy()
    n[..., 3] = np.where(n[..., 3] >= 128, 255, 0)
    return n, soft


def zoom_nearest(a, z):
    """Nearest-neighbour zoom about the centre, same size (Ren'Py `truecenter zoom z`); keeps the colour set."""
    h, w = a.shape[:2]
    ys = np.floor((np.arange(h) + 0.5 - h / 2) / z + h / 2).astype(int).clip(0, h - 1)
    xs = np.floor((np.arange(w) + 0.5 - w / 2) / z + w / 2).astype(int).clip(0, w - 1)
    return a[ys][:, xs]


def key24(rgb):
    rgb = rgb.astype(np.uint32)
    return (rgb[..., 0] << 16) | (rgb[..., 1] << 8) | rgb[..., 2]


def build(scene, palettes):
    out = RIP / "packs" / scene
    layers, soft = [], 0  # (name, [(relative out path, RGBA array)])
    for spec in SCENES[scene]:
        images = [(f"layers/{spec['pool']}/{Path(p).stem}.png", p) for p in pool_images(spec["pool"])] \
            if "pool" in spec else [(f"layers/{spec['name']}.png", spec["image"])]
        loaded = []
        for dest, rel in images:
            a, s = native(rel)
            soft += s
            loaded.append((dest, zoom_nearest(a, spec["zoom"]) if spec.get("zoom") else a))
        layers.append((spec["name"], loaded))

    keys = np.unique(np.concatenate([key24(a[a[..., 3] == 255][:, :3]) for _, imgs in layers for _, a in imgs]))
    if len(keys) > 255:
        sys.exit(f"{scene}: {len(keys)} colours, the index space holds 255")
    colours = np.stack([(keys >> 16) & 255, (keys >> 8) & 255, keys & 255], 1).astype(np.uint8)
    plte = [0, 0, 0] + colours.ravel().tolist()
    plte += [0] * (768 - len(plte))  # a full 256-entry PLTE keeps Pillow from writing fewer than 8 bits

    for sub in ("layers", "luts", "preview"):
        (out / sub).mkdir(parents=True, exist_ok=True)
    indexed = {}
    for _, imgs in layers:
        for dest, a in imgs:
            idx = np.where(a[..., 3] == 255, np.searchsorted(keys, key24(a[..., :3])) + 1, 0).astype(np.uint8)
            (out / dest).parent.mkdir(parents=True, exist_ok=True)
            im = Image.fromarray(idx, "P")
            im.putpalette(plte)
            im.save(out / dest, transparency=0)
            indexed[dest] = idx

    lines = ["# molokolive scene pack v0: layers bottom to top; several images = random choice at start",
             f"size {NATIVE[0]} {NATIVE[1]}", f"scale {SCALE}"]
    lines += [f"layer {name} " + " ".join(dest for dest, _ in imgs) for name, imgs in layers]
    (out / "scene.txt").write_text("\n".join(lines) + "\n")

    first = np.zeros(NATIVE[::-1], np.uint8)
    for _, imgs in layers:
        idx = indexed[imgs[0][0]]
        first = np.where(idx != 0, idx, first)

    available = mc.load_palettes()
    for name in palettes:
        if name not in available:
            sys.exit(f"unknown palette {name!r}; known: {', '.join(available)}")
        pal = available[name]
        mapped = colours if pal is None else pal.map(colours)
        lut = np.zeros((256, 4), np.uint8)
        lut[1:len(mapped) + 1] = np.column_stack([mapped[:, 2], mapped[:, 1], mapped[:, 0], np.full(len(mapped), 255)])
        file = name.replace(" ", "-")
        (out / "luts" / f"{file}.bin").write_bytes(lut.tobytes())
        rgb = np.zeros((256, 3), np.uint8)
        rgb[1:len(mapped) + 1] = mapped
        Image.fromarray(rgb[first], "RGB").save(out / "preview" / f"{file}.png")

    images = sum(len(imgs) for _, imgs in layers)
    print(f"{scene}: {len(layers)} layers, {images} images, {len(keys)} colours, "
          f"{soft} soft-alpha px snapped, palettes {', '.join(palettes)} -> {out.relative_to(ROOT)}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("scenes", nargs="+", choices=sorted(SCENES))
    ap.add_argument("--palette", nargs="+", default=["firefly-neutral", "milkchan-neutral", "none"])
    args = ap.parse_args()
    for scene in args.scenes:
        build(scene, args.palette)


if __name__ == "__main__":
    main()
