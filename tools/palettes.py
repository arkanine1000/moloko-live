#!/usr/bin/env python3
"""Extract, export and swap PNG palettes (PLTE chunks) without touching pixel data.

  palettes.py extract IMG.png [...] [-o DIR]   write DIR/<stem>.{hex,gpl,json} and a swatch <stem>.swatch.png
  palettes.py apply PALETTE IMG.png OUT.png    replace IMG's PLTE with PALETTE (.hex/.gpl/.json or another indexed .png)
  palettes.py neutralize IMG.png OUT.png       apply the firefly-neutral grade (OKLCH chroma x0.62, hue kept)

Indexed PNGs only (colour type 3). Stdlib only, so it runs anywhere.
"""
import argparse, json, pathlib, struct, sys, zlib

SIG = b"\x89PNG\r\n\x1a\n"


def read_chunks(path):
    data = pathlib.Path(path).read_bytes()
    if data[:8] != SIG:
        raise SystemExit(f"{path}: not a PNG")
    i, out = 8, []
    while i < len(data):
        (n,) = struct.unpack(">I", data[i:i + 4])
        out.append((data[i + 4:i + 8], data[i + 8:i + 8 + n]))
        i += 12 + n
    return out


def chunk(tag, body):
    return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body))


def png_palette(path):
    chunks = dict(read_chunks(path))
    if chunks[b"IHDR"][9] != 3 or b"PLTE" not in chunks:
        raise SystemExit(f"{path}: not an indexed PNG")
    v = chunks[b"PLTE"]
    return [v[j:j + 3].hex() for j in range(0, len(v), 3)], chunks.get(b"tRNS")


def load_palette(path):
    p = pathlib.Path(path)
    if p.suffix == ".png":
        return png_palette(p)[0]
    text = p.read_text()
    if p.suffix == ".json":
        return [c.lstrip("#") for c in json.loads(text)["colors"]]
    if p.suffix == ".gpl":
        cols = []
        for line in text.splitlines():
            parts = line.split()
            if len(parts) >= 3 and all(x.isdigit() for x in parts[:3]):
                cols.append(bytes(int(x) for x in parts[:3]).hex())
        return cols
    return [ln.strip().lstrip("#") for ln in text.splitlines() if ln.strip()]


def swatch_png(colors, cell=24, per_row=16):
    """Indexed PNG strip using the palette itself, index order left-to-right, top-to-bottom."""
    rows = -(-len(colors) // per_row)
    w, h = per_row * cell, rows * cell
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        for x in range(w):
            idx = (y // cell) * per_row + x // cell
            raw.append(idx if idx < len(colors) else 0)
    return (SIG + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 3, 0, 0, 0))
            + chunk(b"PLTE", b"".join(bytes.fromhex(c) for c in colors))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def cmd_extract(args):
    out = pathlib.Path(args.o)
    out.mkdir(parents=True, exist_ok=True)
    for img in args.images:
        colors, trns = png_palette(img)
        stem = pathlib.Path(img).stem
        (out / f"{stem}.hex").write_text("\n".join(colors) + "\n")
        gpl = [f"GIMP Palette", f"Name: {stem}", f"Columns: 16", "#"]
        gpl += [f"{int(c[0:2], 16):3} {int(c[2:4], 16):3} {int(c[4:6], 16):3}\t#{c} [{i}]" for i, c in enumerate(colors)]
        (out / f"{stem}.gpl").write_text("\n".join(gpl) + "\n")
        meta = {"source": str(pathlib.Path(img).resolve()), "count": len(colors), "colors": ["#" + c for c in colors]}
        if trns:
            meta["alpha"] = list(trns)
        (out / f"{stem}.json").write_text(json.dumps(meta, indent=2) + "\n")
        (out / f"{stem}.swatch.png").write_bytes(swatch_png(colors))
        print(f"{img}: {len(colors)} colours -> {out}/{stem}.*")


# "Neutral" grade, fitted from firefly.png -> firefly-neutral.png (48 index-aligned entries):
# OKLCH hue kept, chroma x0.621 (rms 0.0007), lightness 0.992*L + 0.0116 (rms 0.0043).
NEUTRAL = {"chroma": 0.621, "l_scale": 0.992, "l_offset": 0.0116}


def _lin(c):
    c /= 255
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def _enc(c):
    c = min(max(c, 0.0), 1.0)
    return round(255 * (12.92 * c if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055))


def to_oklab(hex6):
    r, g, b = (_lin(x) for x in bytes.fromhex(hex6))
    l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b) ** (1 / 3)
    m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b) ** (1 / 3)
    s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b) ** (1 / 3)
    return (0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
            0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s)


def from_oklab(L, A, B):
    l = (L + 0.3963377774 * A + 0.2158037573 * B) ** 3
    m = (L - 0.1055613458 * A - 0.0638541728 * B) ** 3
    s = (L - 0.0894841775 * A - 1.2914855480 * B) ** 3
    rgb = (4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
           -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
           -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s)
    return bytes(_enc(c) for c in rgb).hex()


def neutralize(colors, chroma=NEUTRAL["chroma"], l_scale=NEUTRAL["l_scale"], l_offset=NEUTRAL["l_offset"]):
    out = []
    for c in colors:
        L, A, B = to_oklab(c)
        out.append(from_oklab(l_scale * L + l_offset, A * chroma, B * chroma))
    return out


def cmd_neutralize(args):
    colors, _ = png_palette(args.image)
    new = neutralize(colors, args.chroma, args.l_scale, args.l_offset)
    chunks = read_chunks(args.image)
    plte = b"".join(bytes.fromhex(c) for c in new)
    pathlib.Path(args.out).write_bytes(SIG + b"".join(chunk(t, plte if t == b"PLTE" else v) for t, v in chunks))
    print(f"{args.out}: neutralized {len(new)} colours (chroma x{args.chroma})")


def cmd_apply(args):
    colors = load_palette(args.palette)
    chunks = read_chunks(args.image)
    old = len(dict(chunks)[b"PLTE"]) // 3
    if len(colors) < old:
        raise SystemExit(f"palette has {len(colors)} colours, image needs {old}")
    plte = b"".join(bytes.fromhex(c) for c in colors[:old])
    body = b"".join(chunk(t, plte if t == b"PLTE" else v) for t, v in chunks)
    pathlib.Path(args.out).write_bytes(SIG + body)
    print(f"{args.out}: PLTE swapped ({old} colours)")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(required=True)
    e = sub.add_parser("extract"); e.add_argument("images", nargs="+"); e.add_argument("-o", default="palettes"); e.set_defaults(fn=cmd_extract)
    a = sub.add_parser("apply"); a.add_argument("palette"); a.add_argument("image"); a.add_argument("out"); a.set_defaults(fn=cmd_apply)
    n = sub.add_parser("neutralize", help="apply the firefly-neutral grade to an indexed PNG's palette")
    n.add_argument("image"); n.add_argument("out")
    n.add_argument("--chroma", type=float, default=NEUTRAL["chroma"])
    n.add_argument("--l-scale", type=float, default=NEUTRAL["l_scale"])
    n.add_argument("--l-offset", type=float, default=NEUTRAL["l_offset"])
    n.set_defaults(fn=cmd_neutralize)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
