import pytest

from molokolive.cli import ToolError
from molokolive.renpy import parse_images
from molokolive.scenes import SCENES, Choices, Timeline, resolve, scene_layers, sky


def test_resolve_each_kind(art):
    images = parse_images(art)
    assert resolve(images, "dream") == [Choices("dream", None, ["images/dream.png"])]
    assert resolve(images, "images/plain.png") == [Choices("plain", None, ["images/plain.png"])]
    assert resolve(images, "sky1") == [Choices("sky1", None, ["images/skybox/1.png", "images/skybox/2.png", "images/skybox/3.png"])]
    assert resolve(images, "blink") == [Timeline("blink", None, images["blink"].frames, 0)]
    assert resolve(images, "face") == [Choices("dream", None, ["images/dream.png"])] * 2


def test_resolve_sky_spec_sets_name_zoom_and_drift(art):
    layer, = resolve(parse_images(art), sky("sky1"))
    assert (layer.name, layer.zoom, layer.drift) == ("sky", 1.01, "circle")


def test_resolve_chains_animations(art):
    images = parse_images(art)
    layer, = resolve(images, ["blink", "eyes"])
    assert layer.frames == images["blink"].frames + images["eyes"].frames
    assert layer.loop == 3  # the last animation loops from its own start
    with pytest.raises(ToolError, match="only animations can be chained"):
        resolve(images, ["dream", "blink"])


def test_every_scene_names_defined_images():
    names = {n for spec in SCENES.values() for layer in spec["layers"]
             for n in (layer if isinstance(layer, list) else [layer["image"] if isinstance(layer, dict) else layer])}
    assert all(isinstance(n, str) for n in names)
    assert scene_layers.__doc__
