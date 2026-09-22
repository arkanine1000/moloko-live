import numpy as np

from molokolive.pixels import bounding_box, dirty_rects, key24, sample_grid, zoom_nearest


def test_key24():
    assert key24(np.array([13, 13, 20], np.uint8)) == 0x0d0d14
    assert key24(np.array([[[255, 0, 0]]], np.uint8)).tolist() == [[0xff0000]]


def test_zoom_nearest():
    a = np.arange(16).reshape(4, 4)
    assert np.array_equal(zoom_nearest(a, 1.0), a)
    assert zoom_nearest(a, 2.0).tolist() == a[[1, 1, 2, 2]][:, [1, 1, 2, 2]].tolist()
    assert set(zoom_nearest(a, 1.3).ravel()) <= set(a.ravel())


def test_sample_grid(rng):
    native = rng.integers(0, 256, (6, 6, 4)).astype(np.uint8)
    full = np.repeat(np.repeat(native, 2, 0), 2, 1)
    sample, off, phase = sample_grid(full, 2)
    assert (off, phase) == (0, (0, 0)) and np.array_equal(sample, native)
    sample, off, phase = sample_grid(np.roll(full, (1, 1), (0, 1)), 2)
    assert (off, phase) == (0, (1, 1)) and np.array_equal(sample, native)
    stray = full.copy()
    stray[0, 0] ^= 1  # one pixel off: the other three of its block no longer match the sample
    sample, off, phase = sample_grid(stray, 2)
    assert (off, phase) == (3, (0, 0)) and np.array_equal(sample, stray[::2, ::2])


def test_bounding_box():
    mask = np.zeros((5, 7), bool)
    assert bounding_box(mask) is None
    mask[1, 2] = mask[3, 5] = True
    assert bounding_box(mask) == [2, 1, 6, 4]


def test_dirty_rects():
    diff = np.zeros((10, 10), bool)
    assert dirty_rects(diff, 4) == []
    diff[0, 0] = diff[9, 9] = True  # two lone pixels: one tile each, clipped to the image
    assert dirty_rects(diff, 4) == [[0, 0, 4, 4], [8, 8, 10, 10]]
    diff[:] = False
    diff[0:10, 1] = True  # a column: the same run in every tile row stacks into one rectangle
    assert dirty_rects(diff, 4) == [[0, 0, 4, 10]]
    diff[:] = False
    diff[0, 0] = diff[0, 5] = True  # two adjacent tiles: one wide rectangle
    assert dirty_rects(diff, 4) == [[0, 0, 8, 4]]
    diff[4, 0] = True  # the run below is narrower, so it starts a new rectangle
    assert dirty_rects(diff, 4) == [[0, 0, 8, 4], [0, 4, 4, 8]]
