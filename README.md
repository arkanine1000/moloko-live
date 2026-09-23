# moloko-live

Scenes from *Milk outside a bag of milk outside a bag of milk*, animated as your desktop wallpaper on X11.
They play with the game's own timings, with support for custom palettes.

It costs next to nothing. Only the pixels that change are drawn, and the animation pauses while windows cover the
desktop, while on battery power, and optionally while the CPU is hot.

*No game files are included. You need your own copy of the game.*

## Requirements

- Linux with X11, and a compositor such as picom
- The game, from Steam (app 1604000)
- Rust, to build the wallpaper
- Python 3.12 or newer, for the tools that prepare the scenes
- [uv](https://docs.astral.sh/uv/), which `tools/rip.sh` uses to run unrpa
- ffmpeg (only for showreel videos)

## Quick start

1. Install the tools into a virtual environment. The repository comes with a direnv setup for pyenv:

   ```sh
   direnv allow
   pip install -e .
   ```

2. Extract the game's files into `rip/`. If Steam keeps the game somewhere else, set `GAME` to its folder:

   ```sh
   tools/rip.sh
   ```

3. Build the scenes. They go to `~/.local/share/molokolive/packs`:

   ```sh
   molokolive-pack
   ```

4. Build the wallpaper and start it:

   ```sh
   cargo install --path engine --root ~/.local
   molokolive
   ```

## Controls

While the wallpaper runs, `molokolive` followed by a command controls it. 

| Command | What it does |
|---|---|
| `molokolive next` | show the next scene |
| `molokolive prev` | go back to the scene before |
| `molokolive sky-next` | change the current scene's sky |
| `molokolive sky-prev` | change it back |
| `molokolive pause` / `resume` | pause or resume the animation |
| `molokolive status` | show the scene, its sky, and whether it's paused |

By default, the wallpaper changes scene after every 60 seconds of animation, in random order, each time with a
random sky.

## Options

`molokolive --help` lists them all, and `--help-all` adds the advanced ones.

| Option | What it does |
|---|---|
| `--autoplay SECONDS` | how long each scene plays; `--autoplay off` stays on one scene (default: 60) |
| `--scenes A,B,...` | rotate through these scenes only |
| `--skip-scenes A,B,...` | leave these scenes out; `*` matches any text here and in `--scenes`, e.g. `--skip-scenes 'mini_cg_*'` |
| `--start-scene NAME` | start with this scene |
| `--shuffle` | play the scenes in random order instead of the order the list shows |
| `--persist-sky` | give each scene back the sky it had last time |
| `--no-drift` | keep the skies still |
| `--max-temp DEGREES` | also pause while the CPU is at least this hot, in °C |
| `--palette NAME` | colour palette (default: `neutral-lift`) |

`molokolive --scenes --list` shows the scenes: `cg_ceiling`, `cg_dream`, `cg_eyelash`, `cg_fall_close`,
`cg_fall_far`, `cg_firefly`, `cg_floor`, `cg_mirror`, `cg_mirror_brush`, `cg_pills`, `mini_cg_1`, `mini_cg_door`,
`mini_cg_eyes`, `mini_cg_momp` and `mini_cg_run`.

## Palettes

The default palette, `neutral-lift`, was fitted to scenes recoloured by hand. To make your own:

1. Paint over a 1920x1080 screenshot of a scene and save it under `rip/recolours/`.
2. Fit the palette: `molokolive-recolour rip/recolours/mine.png:cg_dream --name mine`
3. Rebuild the scenes with it: `molokolive-pack --palette mine neutral-lift`
4. Use it: `molokolive --palette mine`

## Tools

Each tool supplies its own `--help`.

| Tool | What it does |
|---|---|
| `tools/rip.sh` | extract the game's files into `rip/` (videos are skipped) |
| `molokolive-pack` | build the scenes the wallpaper plays |
| `molokolive-recolour` | fit a palette to recoloured scene screenshots |
| `molokolive-showreel` | render every scene into one video, as the wallpaper would play it |

## Repository

| Path | Contents |
|---|---|
| `engine/` | the wallpaper (Rust) |
| `src/molokolive/` | the tools (Python) |
| `tools/` | the extraction script |
| `palettes/` | the palettes the scenes are built with |
| `docs/` | how it all works: [docs/README.md](docs/README.md) |
| `rip/` | files extracted from the game, recolours and showreels; ignored by git |

Nothing from the game is ever committed: extracted files stay in `rip/`, and the scene packs live in
`~/.local/share/molokolive`.

---

## Attribution

Special thanks to Nikita Kryukov for his work on the Milk series. Get the games here, they're worth every penny:
 - [Milk inside a bag of milk inside a bag of milk](https://store.steampowered.com/app/1392820/)
 - [Milk outside a bag of milk outside a bag of milk](https://store.steampowered.com/app/1604000/)

**O! O! O!**
