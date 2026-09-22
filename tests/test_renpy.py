import pytest

from molokolive.cli import ToolError
from molokolive.renpy import Animation, Composite, Pool, Still, parse_atl, parse_images, parse_transform


def test_parse_images_kinds(art):
    images = parse_images(art)
    assert images["dream"] == Still("images/dream.png")
    assert images["blink"] == Animation([("images/blink_1.png", (0.1,)), ("images/blink_2.png", (0.2,)),
                                         ("images/blink_1.png", (0.1,))], loop=0)
    assert images["sky1"] == Pool(["images/skybox/1.png", "images/skybox/2.png", "images/skybox/3.png"])


def test_composite_keeps_only_silent_images(art):
    # WhileSpeaking without a silent image shows nothing when nobody speaks.
    assert parse_images(art)["face"] == Composite(["dream", "dream"])


def test_composite_offsets_are_rejected():
    with pytest.raises(ToolError, match="offset LiveComposite"):
        parse_images('image x = LiveComposite((10, 10), (5, 0), "a")')


def test_atl_holds_and_choices(art):
    eyes = parse_images(art)["eyes"]
    assert eyes.loop == 0
    assert eyes.frames == [("images/eyes_open.png", (2.0, 5.0)), ("images/eyes_half.png", (0.1,)),
                           ("images/eyes_closed.png", (0.2,))]


def test_atl_without_repeat_stops():
    assert parse_atl("x", ['"a.png"', '0.5', '"b.png"']).loop is None


def test_atl_errors():
    with pytest.raises(ToolError, match="pause before the first image"):
        parse_atl("x", ["0.5", '"a.png"'])
    with pytest.raises(ToolError, match="unsupported ATL 'linear 1 alpha 0'"):
        parse_atl("x", ['"a.png"', "linear 1 alpha 0"])


def test_transform_circle_steps(script):
    assert parse_transform(script, "circle", 2) == {
        "period": 8.0,
        "steps": [[0.667, -1, 0], [1.333, -1, 1], [2.667, 0, 1], [3.333, 1, 1], [4.667, 1, 0], [5.333, 1, -1],
                  [6.667, 0, -1], [7.333, -1, -1]],
    }
    assert parse_transform(script, "circle2", 2) == {
        "period": 8.0, "steps": [[1.0, -1, 0], [3.0, -1, -1], [5.0, 0, -1], [7.0, 0, 0]],
    }


def test_transform_errors(script):
    with pytest.raises(ToolError, match="transform nope not found"):
        parse_transform(script, "nope", 2)
    with pytest.raises(ToolError, match="unsupported 'xoffset 2'"):
        parse_transform(script, "still", 2)
