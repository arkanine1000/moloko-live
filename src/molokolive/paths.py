"""Where the tools find the game files, the palettes and the packs."""
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]  # the repository, for an editable install
RIP = ROOT / "rip"                          # the game files tools/rip.sh extracted; gitignored
PALETTES = ROOT / "palettes"


def default_packs():
    """Where molokolive looks for scene packs: $XDG_DATA_HOME/molokolive/packs, else ~/.local/share/molokolive/packs."""
    return Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share") / "molokolive" / "packs"
