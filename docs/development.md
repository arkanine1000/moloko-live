# Development

## Setup

Python 3.12 or newer. The repository pins 3.14 through direnv and pyenv (`.envrc`); any virtual environment works:

```sh
pip install -e '.[dev]'     # the tools, pytest and ruff
ruff check src tests
pytest
```

The engine is a normal Cargo crate:

```sh
cd engine
cargo fmt --check && cargo clippy --all-targets && cargo test
cargo install --path . --root ~/.local     # puts molokolive in ~/.local/bin, which i3's PATH has
```

`rustfmt.toml` sets 120 columns.

## Testing without the game

The Python tests use synthetic fixtures only: short Ren'Py snippets, tiny arrays, a temporary palettes
directory. The engine's tests cover the pure logic: wildcards, rectangle merging, rotation, geometry, thermal
hysteresis, option parsing.

With the game extracted, the strongest check for the pack builder is that a rebuild is byte-identical to a
previous one: build into a second directory with `--packs` and `diff -r` the two. The manifest's `palettes` map
follows the order of `--palette`, so pass the same list to both builds.

## Measuring cost

Measure process CPU time, not per-frame time, and measure the engine, picom and Xorg together: the compositor
recomposites every damaged region, so a change in the engine shows up in the other two. `--stats` prints
frames, rectangles and pixels per second and the time each stage takes. To see whether the engine is truly
idle, count its context switches over ten seconds. To check that output is unchanged, compare screenshots pixel
for pixel with the same `--start-scene` and `--sky`; to check cadence, record the screen with ffmpeg and time the
steps.

Targets, as percent of one core:

| State | CPU | Memory |
|---|---|---|
| paused | 0 (blocked on events) | under 30 MB |
| static scene visible | under 0.5 | |
| animating | under 2 | |

Testing on a live desktop means briefly switching to an empty workspace. Starting a second `molokolive` ends the
one already running (it takes over the window and the socket), so restart the usual one afterwards.

## Conventions

- Commits follow scoped Conventional Commits: `feat(engine): …`, `fix(pack): …`, `docs: …`.
- British spelling in code and prose: colour, recolour. The manifest key is `colours`.
- It is a sky, not a skybox; a pool of choices, not a set of variants.
- Errors are one line, prefixed with the program name, and never a traceback or a panic. The engine prints
  with `say!`, which ignores a closed stdout (`molokolive --scenes --list | head`), where `println!` would panic.
- Nothing derived from the game is committed. `rip/` holds the extracted files, recolours, dialogue and
  showreels; packs live outside the repository.
