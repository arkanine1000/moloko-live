"""Fixtures for the tool tests. Everything is synthetic: no game files are needed."""
import json

import numpy as np
import pytest

ART = '''
image dream = "images/dream.png"

image blink = Animation(
    "images/blink_1.png", 0.1,
    "images/blink_2.png", 0.2,
    "images/blink_1.png", 0.1,
)

image face = LiveComposite(
    (960, 540),
    (0, 0), "dream",
    (0, 0), WhileSpeaking("gg", "blink"),
    (0, 0), WhileSpeaking("gg", "blink", "dream"),
)

image sky1:
    choice:
        "images/skybox/1.png"
    choice:
        "images/skybox/2.png"
        "images/skybox/3.png"

image eyes:
    "images/eyes_open.png"
    choice:
        2.0
    choice:
        pause 4
        1.0
    "images/eyes_half.png"
    0.1
    "images/eyes_closed.png"
    pause 0.2
    repeat
'''

SCRIPT = '''
init:
    transform circle:
        ease 2 yoffset 2
        ease 2 xoffset 2
        ease 2 yoffset -2
        ease 2 xoffset -2
        repeat

    transform circle2:
        ease 2 xoffset -2
        ease 2 yoffset -2
        ease 2 xoffset 0
        ease 2 yoffset 0
        repeat

    transform still:
        xoffset 2
'''

NEUTRAL_LIFT = {"gamma": 1.11, "l0": 0.2626, "chroma": [0.0032, 0.1589, 0.0747], "hue": [261.35, 3.97]}


@pytest.fixture
def art():
    return ART


@pytest.fixture
def script():
    return SCRIPT


@pytest.fixture
def palettes_dir(tmp_path):
    (tmp_path / "tones.hex").write_text("#000000\n808080\n\nffffff\n")
    (tmp_path / "lift.json").write_text(json.dumps({"name": "lift", "rule": NEUTRAL_LIFT}))
    (tmp_path / "notes.json").write_text('{"source": "something else"}')
    return tmp_path


@pytest.fixture
def rng():
    return np.random.default_rng(7)
