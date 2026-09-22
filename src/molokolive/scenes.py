"""The scenes the wallpaper plays, each as the layer stack rip/raw/script.rpy shows it."""
from dataclasses import dataclass
from pathlib import Path

from molokolive.cli import ToolError
from molokolive.renpy import Animation, Composite, Pool, Still

NATIVE = (960, 540)  # the game's own resolution; it draws everything at x2
PIXEL = 2            # full-resolution pixels per native pixel
BACKGROUND = (13, 13, 20)  # #0d0d14, the game's near-black and the mini_cg backdrop

# `show skyN at circle: truecenter zoom 1.01`: the zoom hides the edges while `circle` drifts the sky +-2 px.
SKY_ZOOM = 1.01
# The window every mini_cg is drawn in (native px, union over all their frames); the rest is BACKGROUND.
MINI_CROP = (224, 50, 736, 334)
# Dirty-region grid (native px): a patch lists its changed TILE x TILE cells, merged into rectangles.
TILE = 16
# Share of full-resolution pixels allowed off the 2x2 grid before an image is rejected.
MAX_OFF_GRID = 0.05


def sky(pool):
    """`show skyN at circle: truecenter zoom 1.01`."""
    return {"image": pool, "zoom": SKY_ZOOM, "name": "sky", "drift": "circle"}


def reflection(pool):
    """A cg_mirror_gg pool: `show cg_mirror_ggN at circle2: truecenter zoom 1.01`, between the sky and the mirror."""
    return {"image": pool, "zoom": SKY_ZOOM, "name": "reflection", "drift": "circle2"}


# A layer is an image name, {"image", "zoom", "name", "drift"}, or a list of animations: all but the last play
# once, then the last loops.
SCENES = {
    "cg_ceiling": {"layers": [sky("sky3"), "cg_ceiling", "fireflies_anim"]},  # the script adds the fireflies after a choice
    "cg_dream": {"layers": [sky("sky3"), "cg_dream_s"]},  # cg_dream_s is the variant with blinking eyes
    # The script only ever hides this one, never shows it. Its irises are transparent, so it needs a sky behind it.
    "cg_eyelash": {"layers": [sky("sky2"), "cg_eyelash"]},
    "cg_fall_close": {"layers": [sky("sky4"), "cg_fall_close"]},
    "cg_fall_far": {"layers": [sky("sky1"), "cg_fall_far"]},
    "cg_firefly": {"layers": [sky("sky1"), "cg_firefly"]},
    "cg_floor": {"layers": [sky("sky1"), "cg_floor"]},
    "cg_mirror": {"layers": [sky("sky1"), reflection("cg_mirror_gg4"), "cg_mirror_idle"]},
    "cg_mirror_brush": {"layers": [sky("sky1"), reflection("cg_mirror_gg1"), "cg_mirror_brush"]},
    "cg_pills": {"layers": [sky("sky2"), "cg_pills"]},
    "mini_cg_1": {"crop": MINI_CROP, "layers": [["mini_cg_1_1", "mini_cg_1_2"]]},
    "mini_cg_door": {"crop": MINI_CROP, "layers": ["mini_cg_door"]},
    "mini_cg_eyes": {"crop": MINI_CROP, "layers": ["eyes_fear"]},
    "mini_cg_momp": {"crop": MINI_CROP, "layers": ["mini_cg_momp_22"]},
    "mini_cg_run": {"crop": MINI_CROP, "layers": ["mini_cg_run"]},
}


@dataclass
class Choices:
    """A layer showing one of its images, picked when the scene starts."""
    name: str
    zoom: float | None
    paths: list
    drift: str | None = None  # a transform name in script.rpy


@dataclass
class Timeline:
    """A layer playing an animation: see renpy.Animation."""
    name: str
    zoom: float | None
    frames: list
    loop: int | None


def resolve(images, spec):
    """A layer spec from SCENES -> [Choices | Timeline], using the image definitions of art.rpy."""
    if isinstance(spec, list):  # animations chained: all but the last play once
        frames, loop = [], None
        for item in spec:
            anim = images[item]
            if not isinstance(anim, Animation):
                raise ToolError(f"{item}: only animations can be chained")
            loop = None if anim.loop is None else len(frames) + anim.loop
            frames += anim.frames
        return [Timeline(spec[-1], None, frames, loop)]
    if isinstance(spec, dict):
        layers = resolve(images, spec["image"])
        for layer in layers:
            layer.name, layer.zoom = spec.get("name", layer.name), spec.get("zoom")
            if isinstance(layer, Choices):
                layer.drift = spec.get("drift")
        return layers
    if spec.endswith(".png"):
        return [Choices(Path(spec).stem, None, [spec])]
    image = images[spec]
    if isinstance(image, Still):
        return [Choices(spec, None, [image.path])]
    if isinstance(image, Pool):
        return [Choices(spec, None, image.paths)]
    if isinstance(image, Animation):
        return [Timeline(spec, None, image.frames, image.loop)]
    if isinstance(image, Composite):
        return [layer for sub in image.layers for layer in resolve(images, sub)]
    raise ToolError(f"{spec}: unsupported image definition")


def scene_layers(images, scene):
    """The layers of a scene, bottom to top."""
    return [layer for spec in SCENES[scene]["layers"] for layer in resolve(images, spec)]
