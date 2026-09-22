"""What the command-line tools share: the help layout and how errors reach the user."""
import argparse
import sys
import textwrap


class ToolError(Exception):
    """A problem to report in one line, without a traceback."""


def run(command):
    """Run a tool's command function; a ToolError ends the process with its message on stderr."""
    try:
        command()
    except ToolError as e:
        sys.exit(str(e))


class HelpFormatter(argparse.RawDescriptionHelpFormatter):
    """Help at 100 columns that never breaks a word at a hyphen (palette and file names stay whole)."""

    def __init__(self, prog):
        super().__init__(prog, width=100, max_help_position=30)

    def _split_lines(self, text, width):
        return textwrap.wrap(" ".join(text.split()), width, break_on_hyphens=False)
