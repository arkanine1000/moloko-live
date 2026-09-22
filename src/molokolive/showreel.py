#!/usr/bin/env python3
"""Render scene packs into one showreel video, the way the engine plays them.

  molokolive-showreel                                        # every pack, default palette
  molokolive-showreel cg_floor mini_cg_run --palette firefly-neutral --out rip/showreel/test.mp4

Offline, from the scene packs (molokolive-pack): the engine's timeline rules (random holds per visit, loops, wrap
patches, a shortest hold of 1/--fps), its random sky choice (seeded here), the drift of skies and reflections (rounded
to the nearest frame), and its cover scaling with nearest sampling, which matches the engine on screen pixel for
pixel. Each scene gets its label in the game's font. The video shows game art, so it belongs under the gitignored
rip/.

Every game hold is a multiple of 0.05 s, so the default 20 fps lands each step on a frame boundary. Needs ffmpeg.
"""
import argparse
import json
import random
import subprocess
import sys
import textwrap
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

from molokolive.paths import RIP, ROOT, default_packs

FONT = RIP / "raw" / "images" / "122.ttf"  # Retro Gaming, the game's dialogue font


class HelpFormatter(argparse.RawDescriptionHelpFormatter):
    """Help at 100 columns that never breaks a word at a hyphen (palette and file names stay whole)."""

    def __init__(self, prog):
        super().__init__(prog, width=100, max_help_position=30)

    def _split_lines(self, text, width):
        return textwrap.wrap(" ".join(text.split()), width, break_on_hyphens=False)


class Scene:
    def __init__(self, name, palette, rng, fps, drift_on=True, packs=None):
        d = (packs or default_packs()) / name
        m = json.loads((d / "manifest.json").read_text())
        if palette not in m["palettes"]:
            sys.exit(f"{name}: no palette {palette!r} (rebuild with molokolive-pack --palette {palette})")
        image = lambda rel: np.array(Image.open(d / rel))
        patch = lambda p: None if not p else (p["rect"], image(p["image"]))
        self.rng, self.fps, self.background, self.layers, picks = rng, fps, m["background"], [], []
        for layer in m["layers"]:
            if "choices" in layer:
                i = rng.randrange(len(layer["choices"]))
                base = image(layer["choices"][i])
                drift = None
                if cycle := (layer.get("drift") if drift_on else None):
                    # (frame within the cycle, dx, dy); the layer rests at the offset the cycle ends on
                    drift = {"period": round(cycle["period"] * fps),
                             "steps": [(round(t * fps), dx, dy) for t, dx, dy in cycle["steps"]],
                             "rest": tuple(cycle["steps"][-1][1:]), "offset": None}
                self.layers.append({"pixels": base.copy(), "base": base, "anim": None, "drift": drift})
                if drift:
                    self.move(self.layers[-1], drift["rest"])
                if len(layer["choices"]) > 1:
                    picks.append(f"{layer['name']} {Path(layer['sources'][i]).stem}")
            else:
                steps = [(s["hold"], patch(s["patch"])) for s in layer["steps"]]
                self.layers.append({"pixels": image(layer["base"]), "drift": None, "anim": {
                    "steps": steps, "loop": layer["loop"], "wrap": patch(layer.get("wrap")), "current": 0, "due": None}})
        self.label = name + (f"  ({', '.join(picks)})" if picks else "")
        self.lut = np.fromfile(d / m["palettes"][palette], np.uint8).reshape(256, 4)[:, [2, 1, 0]]
        self.size = m["size"]

    @staticmethod
    def move(layer, offset):
        """Show the layer's base image shifted by `offset` native pixels, repeating the edges (as the engine does)."""
        dx, dy = offset
        h, w = layer["base"].shape
        layer["pixels"] = layer["base"][np.clip(np.arange(h) - dy, 0, h - 1)][:, np.clip(np.arange(w) - dx, 0, w - 1)]
        layer["drift"]["offset"] = offset

    def ticks(self, holds):
        return max(1, round(max(self.rng.choice(holds), 1 / self.fps) * self.fps))

    def cycle(self):
        """Seconds for one pass through the longest timeline (at mean holds) or drift cycle."""
        timelines = [sum(max(float(np.mean(h)), 1 / self.fps) for h, _ in L["anim"]["steps"]) for L in self.layers if L["anim"]]
        drifts = [L["drift"]["period"] / self.fps for L in self.layers if L.get("drift")]
        return max(timelines + drifts, default=0.0)

    def moves(self):
        return any(L["anim"] or L.get("drift") for L in self.layers)

    def start(self):
        for L in self.layers:
            if a := L["anim"]:
                a["due"] = self.ticks(a["steps"][0][0])

    def advance(self, tick):
        """Step every timeline and drift due by `tick`; whether any pixels changed."""
        changed = False
        for L in self.layers:
            if d := L.get("drift"):
                position = tick % d["period"]
                offset = next((tuple(s[1:]) for s in reversed(d["steps"]) if s[0] <= position), d["rest"])
                if offset != d["offset"]:
                    self.move(L, offset)
                    changed = True
            a = L["anim"]
            while a and a["due"] is not None and a["due"] <= tick:
                last = a["current"] + 1 >= len(a["steps"])
                if last and a["loop"] is None:
                    a["due"] = None
                    break
                nxt = a["loop"] if last else a["current"] + 1
                if p := (a["wrap"] if last else a["steps"][nxt][1]):
                    (x0, y0, x1, y1), pixels = p
                    L["pixels"][y0:y1, x0:x1] = pixels
                    changed = True
                a["current"], a["due"] = nxt, a["due"] + self.ticks(a["steps"][nxt][0])
        return changed

    def composite(self):
        frame = np.full(self.layers[0]["pixels"].shape, self.background, np.uint8)
        for L in self.layers:
            frame = np.where(L["pixels"] != 0, L["pixels"], frame)
        return frame


def cover_maps(canvas, width, height):
    """Output column/row -> canvas column/row, the engine's cover geometry."""
    cw, ch = canvas
    k = max(width / cw, height / ch)
    ox, oy = (width - cw * k) / 2, (height - ch * k) / 2
    xs = np.floor((np.arange(width) + 0.5 - ox) / k).astype(int).clip(0, cw - 1)
    ys = np.floor((np.arange(height) + 0.5 - oy) / k).astype(int).clip(0, ch - 1)
    return xs, ys


def main():
    ap = argparse.ArgumentParser(
        prog="molokolive-showreel",
        description="Render the scenes into one video, played the way molokolive plays them. Needs ffmpeg.",
        epilog="""examples:
  molokolive-showreel                                   every scene -> rip/showreel/showreel-neutral-lift.mp4
  molokolive-showreel cg_floor mini_cg_run --size 960x540 --out /tmp/two-scenes.mp4""",
        formatter_class=HelpFormatter,
    )
    ap.add_argument("scenes", nargs="*", metavar="SCENE", help="scenes to include, in order (default: all)")
    ap.add_argument("--palette", metavar="NAME", default="neutral-lift", help="colour palette (default: %(default)s)")
    ap.add_argument("--out", type=Path, metavar="FILE", help="video to write (default: rip/showreel/showreel-PALETTE.mp4)")
    ap.add_argument("--size", metavar="WIDTHxHEIGHT", default="1920x1080", help="video size (default: %(default)s)")
    ap.add_argument("--fps", type=int, default=20, help="frames per second (default: %(default)s, which fits every game timing)")
    ap.add_argument("--min-seconds", type=float, metavar="S", default=8, help="shortest time per moving scene (default: %(default)s)")
    ap.add_argument("--max-seconds", type=float, metavar="S", default=30,
                    help="longest time per moving scene; the default fits a full cycle of every scene (default: %(default)s)")
    ap.add_argument("--static-seconds", type=float, metavar="S", default=4, help="time for a scene that doesn't move (default: %(default)s)")
    ap.add_argument("--seed", type=int, default=20260915, help="seed for the random skies and timings, so renders repeat (default: %(default)s)")
    ap.add_argument("--no-labels", action="store_true", help="leave out the scene names")
    ap.add_argument("--no-drift", action="store_true", help="keep the skies still")
    ap.add_argument("--packs", type=Path, metavar="DIR", default=default_packs(),
                    help="scene packs to render (default: ~/.local/share/molokolive/packs)")
    args = ap.parse_args()

    width, height = map(int, args.size.split("x"))
    if not args.packs.is_dir():
        sys.exit(f"no scene packs in {args.packs} (build them with molokolive-pack, or pass --packs)")
    scenes = args.scenes or sorted(p.name for p in args.packs.iterdir() if (p / "manifest.json").is_file())
    out = args.out or RIP / "showreel" / f"showreel-{args.palette}.mp4"
    out.parent.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    font = ImageFont.truetype(str(FONT), max(12, height // 42))
    encoder = subprocess.Popen(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{width}x{height}",
         "-r", str(args.fps), "-i", "-", "-c:v", "libx264", "-preset", "slow", "-tune", "animation", "-crf", "16",
         "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(out)],
        stdin=subprocess.PIPE)

    total = 0.0
    for name in scenes:
        scene = Scene(name, args.palette, rng, args.fps, drift_on=not args.no_drift, packs=args.packs)
        animated = scene.moves()
        seconds = min(max(scene.cycle(), args.min_seconds), args.max_seconds) if animated else args.static_seconds
        xs, ys = cover_maps(scene.size, width, height)
        scene.start()
        frame = None
        for tick in range(round(seconds * args.fps)):
            if scene.advance(tick) or frame is None:
                im = Image.fromarray(scene.lut[scene.composite()[ys[:, None], xs[None, :]]], "RGB")
                if not args.no_labels:
                    draw, at = ImageDraw.Draw(im), (height // 45, height - height // 19)
                    x0, y0, x1, y1 = draw.textbbox(at, scene.label, font=font)
                    pad = height // 120
                    draw.rectangle((x0 - pad, y0 - pad, x1 + pad, y1 + pad), fill=(20, 24, 29))
                    draw.text(at, scene.label, font=font, fill=(206, 214, 223))
                frame = im.tobytes()
            encoder.stdin.write(frame)
        total += seconds
        print(f"{scene.label:40} {seconds:5.1f} s", flush=True)
    encoder.stdin.close()
    if encoder.wait():
        sys.exit("ffmpeg failed")
    print(f"{total:.0f} s -> {out.relative_to(ROOT) if out.is_relative_to(ROOT) else out}")


if __name__ == "__main__":
    main()
