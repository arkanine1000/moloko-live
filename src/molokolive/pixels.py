"""Pixel helpers for indexed and RGBA images as numpy arrays."""
import numpy as np


def key24(rgb):
    """(..., 3) colours -> (...) 24-bit integers, for sorting and searching."""
    rgb = np.asarray(rgb).astype(np.uint32)
    return (rgb[..., 0] << 16) | (rgb[..., 1] << 8) | rgb[..., 2]


def zoom_nearest(a, z):
    """Nearest-neighbour zoom about the centre, same size (Ren'Py `truecenter zoom z`); keeps the colour set."""
    h, w = a.shape[:2]
    ys = np.floor((np.arange(h) + 0.5 - h / 2) / z + h / 2).astype(int).clip(0, h - 1)
    xs = np.floor((np.arange(w) + 0.5 - w / 2) / z + w / 2).astype(int).clip(0, w - 1)
    return a[ys][:, xs]


def sample_grid(a, pixel):
    """Sample an image drawn on a `pixel` x `pixel` grid down to one pixel per block, at the grid phase that fits
    best -> (sample, off-grid pixel count, phase). Pixels that fit no phase keep their block's top-left value."""
    best = None
    for dy in range(pixel):
        for dx in range(pixel):
            shifted = np.roll(a, (-dy, -dx), (0, 1))
            sample = shifted[::pixel, ::pixel]
            off = int((np.repeat(np.repeat(sample, pixel, 0), pixel, 1) != shifted).any(-1).sum())
            if best is None or off < best[1]:
                best = (sample.copy(), off, (dx, dy))
            if off == 0:
                return best
    return best


def bounding_box(mask):
    """[x0, y0, x1, y1] around the true pixels, or None if there are none."""
    ys, xs = np.nonzero(mask)
    if not len(ys):
        return None
    return [int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1]


def dirty_rects(diff, tile):
    """Changed pixels as rectangles on a `tile` grid: runs of changed tiles in each tile row, stacked while the same
    run continues in the next row. Far fewer pixels than the bounding box when the changes are scattered."""
    h, w = diff.shape
    th, tw = -(-h // tile), -(-w // tile)
    padded = np.zeros((th * tile, tw * tile), bool)
    padded[:h, :w] = diff
    tiles = padded.reshape(th, tile, tw, tile).any((1, 3))
    rects, running = [], {}  # (tx0, tx1) -> first tile row
    for ty in range(th + 1):
        runs = set()
        if ty < th:
            edges = np.flatnonzero(np.diff(np.concatenate([[0], tiles[ty].astype(np.int8), [0]])))
            runs = {(int(s), int(e)) for s, e in zip(edges[::2], edges[1::2], strict=True)}
        for run in [r for r in running if r not in runs]:
            rects.append((running.pop(run), run, ty))
        for run in runs:
            running.setdefault(run, ty)
    return sorted([tx0 * tile, ty0 * tile, min(tx1 * tile, w), min(ty1 * tile, h)] for ty0, (tx0, tx1), ty1 in rects)
