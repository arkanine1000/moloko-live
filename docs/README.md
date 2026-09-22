# molokolive documentation

molokolive plays scenes from *Milk outside a bag of milk outside a bag of milk* behind the desktop. Two programs
share the work: `molokolive-pack` (Python) turns the game's files into scene packs once, and `molokolive` (Rust)
plays them. Nothing derived from the game is committed: the extracted files stay in the gitignored `rip/`, and the
packs are written to `~/.local/share/molokolive/packs`.

| Page | What it covers |
|---|---|
| [architecture.md](architecture.md) | how the pack builder and the engine divide the work, and the decisions behind that |
| [pack-format.md](pack-format.md) | the files of a scene pack and `manifest.json` |
| [game-assets.md](game-assets.md) | what the game ships: scenes, skies, resolution, timings, and the Ren'Py the tools read |
| [rendering.md](rendering.md) | how frames reach the screen, and the X11 and compositor pitfalls that shaped it |
| [pause.md](pause.md) | when the animation stops: covered desktop, battery, heat |
| [rotation-and-control.md](rotation-and-control.md) | scene order, autoplay, skies, and the control socket |
| [drift.md](drift.md) | the slow movement of skies and reflections |
| [palettes.md](palettes.md) | how colours are remapped, and how the default palette was made |
| [development.md](development.md) | building, testing, measuring, conventions |

The [README](../README.md) at the top of the repository is the user's guide.
