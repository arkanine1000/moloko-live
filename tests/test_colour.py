import numpy as np
import pytest

from molokolive.colour import (
    Identity,
    LiftPalette,
    TonePalette,
    available_palettes,
    hex_to_rgb,
    hex_to_rgb_array,
    lift,
    load_palette,
    oklab_to_srgb,
    read_hex_palette,
    rgb_to_hex,
    srgb_to_oklab,
)
from tests.conftest import NEUTRAL_LIFT


def test_oklab_round_trip_is_exact(rng):
    corners = np.array([[0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [0, 255, 255],
                        [255, 0, 255]], np.uint8)
    sample = rng.integers(0, 256, (4096, 3)).astype(np.uint8)
    for rgb in (corners, sample):
        assert np.array_equal(oklab_to_srgb(srgb_to_oklab(rgb)), rgb)


def test_oklab_reference_values():
    lab = srgb_to_oklab(np.array([[255, 255, 255], [0, 0, 0]]))
    assert lab[0] == pytest.approx([1.0, 0.0, 0.0], abs=1e-6)
    assert lab[1] == pytest.approx([0.0, 0.0, 0.0], abs=1e-6)


def test_hex_helpers():
    assert hex_to_rgb("#0d0d14") == (13, 13, 20)
    assert hex_to_rgb(" ac3232 ") == (172, 50, 50)
    assert rgb_to_hex((13, 13, 20)) == "#0d0d14"
    assert rgb_to_hex(np.array([255, 0, 1], np.uint8)) == "#ff0001"
    assert hex_to_rgb_array(["#000000", "#ffffff"]).tolist() == [[0, 0, 0], [255, 255, 255]]
    assert hex_to_rgb_array([]).shape == (0, 3)


def test_read_hex_palette(palettes_dir):
    rgb = read_hex_palette(palettes_dir / "tones.hex")
    assert rgb.dtype == np.uint8
    assert rgb.tolist() == [[0, 0, 0], [128, 128, 128], [255, 255, 255]]


def test_tone_palette_picks_the_nearest_lightness():
    palette = TonePalette(np.array([[0, 0, 0], [128, 128, 128], [255, 255, 255]], np.uint8))
    mapped = palette.map(np.array([[10, 10, 10], [200, 20, 20], [250, 250, 250]], np.uint8))
    assert mapped.tolist() == [[0, 0, 0], [128, 128, 128], [255, 255, 255]]


def test_tone_palette_breaks_ties_by_hue():
    palette = TonePalette(np.array([[200, 60, 60], [60, 60, 200]], np.uint8))  # about the same lightness
    mapped = palette.map(np.array([[220, 40, 40], [40, 40, 220]], np.uint8))
    assert mapped.tolist() == [[200, 60, 60], [60, 60, 200]]


def test_lift_matches_the_neutral_lift_pack():
    # Golden values read from the neutral-lift LUT of the cg_floor pack.
    game = hex_to_rgb_array(["#000000", "#1b0d11", "#ebd9bc"])
    assert lift(game, NEUTRAL_LIFT).tolist() == [[32, 37, 45], [58, 65, 78], [221, 226, 237]]


def test_lift_starts_at_the_black_level_and_keeps_order():
    greys = np.array([[v, v, v] for v in range(0, 256, 5)], np.uint8)
    lifted = srgb_to_oklab(lift(greys, NEUTRAL_LIFT))[:, 0]
    assert lifted[0] == pytest.approx(NEUTRAL_LIFT["l0"], abs=0.01)
    assert np.all(np.diff(lifted) >= 0)
    assert lift(np.zeros((0, 3), np.uint8), NEUTRAL_LIFT).shape == (0, 3)


def test_load_palette_kinds(palettes_dir):
    assert isinstance(load_palette("none", palettes_dir), Identity)
    assert isinstance(load_palette("tones", palettes_dir), TonePalette)
    assert isinstance(load_palette("lift", palettes_dir), LiftPalette)
    assert available_palettes(palettes_dir) == ["none", "lift", "tones"]
    rgb = np.array([[1, 2, 3]], np.uint8)
    assert load_palette("none", palettes_dir).map(rgb).tolist() == [[1, 2, 3]]
    assert load_palette("lift", palettes_dir).map(rgb).tolist() == lift(rgb, NEUTRAL_LIFT).tolist()


def test_load_palette_unknown(palettes_dir):
    with pytest.raises(LookupError, match="unknown palette 'notes'.*none, lift, tones"):
        load_palette("notes", palettes_dir)
