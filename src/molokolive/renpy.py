"""Reading the game's Ren'Py scripts: image definitions in art.rpy and drift transforms in script.rpy.

Only the constructs the scenes use are understood: `image X = "path"`, `Animation(...)`, `LiveComposite(...)`,
and ATL blocks with stills, pauses, `choice:` and `repeat`.
"""
import math
import re
from dataclasses import dataclass

from molokolive.cli import ToolError

STRING = re.compile(r'"([^"]+)"')
NUMBER = re.compile(r"(?:pause\s+)?(\d*\.?\d+)")


@dataclass
class Still:
    """`image X = "path"`."""
    path: str


@dataclass
class Pool:
    """An ATL `choice:` block of stills: one is picked each time the image is shown."""
    paths: list


@dataclass
class Animation:
    """Frames in order. Each frame holds for one of its durations, picked at random on every visit. After the
    last frame, playback continues at frame `loop`, or stops if `loop` is None."""
    frames: list  # [(path, (seconds, ...))]
    loop: int | None


@dataclass
class Composite:
    """`LiveComposite(...)`: image names layered bottom to top, all at offset (0, 0)."""
    layers: list


def parse_images(text):
    """Every image definition in art.rpy: name -> Still | Pool | Animation | Composite."""
    lines = text.splitlines()
    images, i = {}, 0
    while i < len(lines):
        line = lines[i]
        if m := re.match(r'\s*image\s+(\w+)\s*=\s*"([^"]+)"\s*$', line):
            images[m[1]] = Still(m[2])
            i += 1
        elif m := re.match(r"\s*image\s+(\w+)\s*=\s*(Animation|LiveComposite)\s*\(", line):
            body, depth = "", 0
            while True:
                body += lines[i] + "\n"
                depth += lines[i].count("(") - lines[i].count(")")
                i += 1
                if depth <= 0:
                    break
            body = body[body.index("(") + 1:]
            if m[2] == "Animation":  # loops by default
                frames = [(p, (float(t),)) for p, t in re.findall(r'"([^"]+)"\s*,\s*(\d*\.?\d+)', body)]
                images[m[1]] = Animation(frames, 0)
            else:
                images[m[1]] = Composite(parse_composite(m[1], body))
        elif m := re.match(r"(\s*)image\s+(\w+)\s*:\s*$", line):
            indent, j = len(m[1]), i + 1
            while j < len(lines) and (not lines[j].strip() or len(lines[j]) - len(lines[j].lstrip()) > indent):
                j += 1
            images[m[2]] = parse_atl(m[2], lines[i + 1:j])
            i = j
        else:
            i += 1
    return images


def parse_composite(name, body):
    """The layers of a LiveComposite body: `(x, y), "name"` or `(x, y), WhileSpeaking(...)` pairs."""
    layers = []
    for x, y, expr in re.findall(r'\(\s*(-?\d+)\s*,\s*(-?\d+)\s*\)\s*,\s*(WhileSpeaking\([^)]*\)|"[^"]+")', body):
        if (x, y) != ("0", "0"):
            raise ToolError(f"{name}: offset LiveComposite layers are not supported")
        strings = STRING.findall(expr)
        if expr.startswith("WhileSpeaking"):
            # WhileSpeaking(who, talking, silent=Null()): nobody speaks on a wallpaper, so only the silent
            # image counts, and it is nothing when left out.
            if len(strings) >= 3:
                layers.append(strings[2])
        else:
            layers.append(strings[0])
    return layers


def parse_atl(name, lines):
    """An ATL block of stills, pauses, `choice:` and `repeat` -> Pool | Animation. Choice blocks of stills are a
    pool (one picked per show); choice blocks of numbers are alternative pauses for the preceding still."""
    rows = [(len(l) - len(l.lstrip()), l.strip()) for l in lines if l.strip() and not l.strip().startswith("#")]
    frames, pool, choices, loop, k = [], [], [], None, 0

    def add_hold(extra):
        if not frames:
            raise ToolError(f"{name}: pause before the first image")
        frames[-1][1] = tuple(round(h + e, 4) for h in frames[-1][1] for e in extra)

    while k < len(rows):
        indent, text = rows[k]
        k += 1
        if text == "choice:":
            sub = []
            while k < len(rows) and rows[k][0] > indent:
                sub.append(rows[k][1])
                k += 1
            if paths := [p for t in sub for p in STRING.findall(t)]:
                pool += paths
            else:
                choices.append(sum(float(NUMBER.fullmatch(t)[1]) for t in sub))
            continue
        if choices:
            add_hold(choices)
            choices = []
        if m := re.fullmatch(r'"([^"]+)"', text):
            frames.append([m[1], (0.0,)])
        elif m := NUMBER.fullmatch(text):
            add_hold((float(m[1]),))
        elif text == "repeat":
            loop = 0
        else:
            raise ToolError(f"{name}: unsupported ATL {text!r}")
    if choices:
        add_hold(choices)
    if pool and not frames:
        return Pool(pool)
    return Animation([(p, h) for p, h in frames], loop)


def parse_transform(text, name, pixel):
    """`transform NAME:` in script.rpy, a repeating block of `ease|linear D xoffset|yoffset V` lines ->
    {"period": s, "steps": [[t, dx, dy], ...]}: the cycle's whole native-pixel offsets and when each takes effect.

    Offsets in the script are full-resolution pixels; `pixel` full-resolution pixels make one native pixel. An
    offset changes where the tweened offset crosses half a native pixel (Ren'Py's ease is 0.5 - cos(pi t) / 2), so
    a tween over two native pixels steps twice. The first pass starts from rest; the cycle is the steady state after
    it, and a layer starts at the offset the cycle ends on (the last step's)."""
    lines = text.splitlines()
    start = next((i for i, l in enumerate(lines) if re.match(rf"\s*transform\s+{name}\s*:\s*$", l)), None)
    if start is None:
        raise ToolError(f"transform {name} not found in script.rpy")
    indent = len(lines[start]) - len(lines[start].lstrip())
    tweens, repeat = [], False
    for line in lines[start + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) <= indent:
            break
        text = line.strip()
        if not text:
            continue
        if text == "repeat":
            repeat = True
            break
        m = re.fullmatch(r"(ease|linear)\s+([\d.]+)\s+(x|y)offset\s+(-?[\d.]+)", text)
        if not m:
            raise ToolError(f"transform {name}: unsupported {text!r}")
        tweens.append((m[1], float(m[2]), m[3], float(m[4]) / pixel))
    if not repeat or not tweens:
        raise ToolError(f"transform {name}: expected a repeating drift")

    def run(position):
        position, t, steps = dict(position), 0.0, []
        native = {axis: round(v) for axis, v in position.items()}
        for kind, duration, axis, target in tweens:
            a, b = position[axis], target
            if a != b:
                lo, hi = min(a, b), max(a, b)
                edges = [k + 0.5 for k in range(math.floor(lo) - 1, math.ceil(hi) + 1) if lo < k + 0.5 < hi]
                for edge in sorted(edges, reverse=b < a):
                    f = (edge - a) / (b - a)
                    fraction = math.acos(1 - 2 * f) / math.pi if kind == "ease" else f
                    native[axis] = round(edge + 0.5) if b > a else round(edge - 0.5)
                    steps.append([round(t + fraction * duration, 3), native["x"], native["y"]])
            position[axis] = b
            t += duration
        return t, position, steps

    period, rest, _ = run({"x": 0.0, "y": 0.0})
    _, end, steps = run(rest)
    if end != rest or not steps:
        raise ToolError(f"transform {name}: its drift doesn't settle into a cycle after one pass")
    return {"period": round(period, 3), "steps": steps}
