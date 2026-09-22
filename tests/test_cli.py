import pytest

from molokolive import pack, recolour, showreel
from molokolive.cli import ToolError, run


@pytest.mark.parametrize("tool", [pack, recolour, showreel])
def test_help(tool, monkeypatch, capsys):
    monkeypatch.setattr("sys.argv", ["x", "--help"])
    with pytest.raises(SystemExit) as e:
        tool.main()
    assert e.value.code == 0
    assert "--packs" in capsys.readouterr().out


def test_unknown_scene(monkeypatch, capsys):
    monkeypatch.setattr("sys.argv", ["x", "cg_nope"])
    with pytest.raises(SystemExit) as e:
        pack.main()
    assert e.value.code == 2
    assert "unknown scene cg_nope" in capsys.readouterr().err


def test_run_reports_tool_errors(capsys):
    def command():
        raise ToolError("something went wrong")

    with pytest.raises(SystemExit) as e:
        run(command)
    assert e.value.code == "something went wrong"
