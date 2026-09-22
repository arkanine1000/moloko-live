# molokolive

Animated scenes from Nikita Kryukov's *Milk outside a bag of milk outside a bag of milk* as your desktop wallpaper,
on X11 with i3. The scenes play with the game's own timings, in a cool, desaturated palette, and the skies drift
slowly the way they do in the game.

It is built to cost next to nothing: it draws only the pixels that change, and it pauses while windows cover the
desktop, while the laptop runs on battery, and (if you ask) while the CPU is hot.

No game files are included. You need your own copy of the game.

## Requirements

- Linux with X11, the i3 window manager, and a compositor such as picom
- The game, from Steam (app 1604000)
- Rust, to build the wallpaper
- Python 3, for the tools that prepare the scenes
- ffmpeg, only if you want to render a showreel video

## Quick start

1. Set up Python. The repository uses direnv and pyenv:

   ```sh
   pyenv install 3.14.6
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

4. Install the wallpaper and start it:

   ```sh
   cargo install --path engine --root ~/.local
   molokolive
   ```

## Controls

While the wallpaper runs, `molokolive` followed by a command controls it. The keys are the ones in the
[i3 setup](#i3) below.

| Command | What it does | Key |
|---|---|---|
| `molokolive next` | show the next scene | Alt+' |
| `molokolive prev` | go back to the scene before | Alt+; |
| `molokolive sky-next` | change the current scene's sky | Alt+Shift+' |
| `molokolive sky-prev` | change it back | Alt+Shift+; |
| `molokolive pause` / `resume` | pause or resume the animation | |
| `molokolive status` | show the scene, its sky, and whether it's paused | |

On its own, the wallpaper changes scene after every 60 seconds of animation, in random order, each time with a
random sky.

## Options

The ones you're most likely to want. `molokolive --help` lists them all, and `--help-all` adds the advanced ones.

| Option | What it does |
|---|---|
| `--autoplay SECONDS` | how long each scene plays; `--autoplay off` stays on one scene (default: 60) |
| `--scenes A,B,...` | rotate through these scenes only |
| `--skip-scenes A,B,...` | leave these scenes out; `*` matches any text here and in `--scenes`, e.g. `--skip-scenes 'mini_cg_*'` |
| `--start-scene NAME` | start with this scene |
| `--persist-sky` | give each scene back the sky it had last time |
| `--no-drift` | keep the skies still |
| `--max-temp DEGREES` | also pause while the CPU is at least this hot, in °C |
| `--palette NAME` | colour palette (default: `neutral-lift`) |

`molokolive --scenes --list` shows the scenes you can use (combined with `--scenes` or `--skip-scenes`, the ones
they pick): `cg_ceiling`, `cg_dream`, `cg_eyelash`,
`cg_fall_close`, `cg_fall_far`, `cg_firefly`, `cg_floor`, `cg_mirror`, `cg_mirror_brush`, `cg_pills`, `mini_cg_1`,
`mini_cg_door`, `mini_cg_eyes`, `mini_cg_momp` and `mini_cg_run`.

## i3

Add this to your i3 config:

```
exec --no-startup-id molokolive --output window
bindsym $mod+semicolon exec --no-startup-id molokolive prev
bindsym $mod+apostrophe exec --no-startup-id molokolive next
bindsym $mod+Shift+semicolon exec --no-startup-id molokolive sky-prev
bindsym $mod+Shift+apostrophe exec --no-startup-id molokolive sky-next
```

- `exec` starts the wallpaper once per login, so reloading i3 doesn't restart the scene rotation.
- `--output window` is needed because picom may not be running yet at the moment i3 starts the wallpaper.
- Keep your usual wallpaper command (for example feh): its picture shows whenever molokolive isn't running.

## Palettes

The default palette, `neutral-lift`, comes from scenes recoloured by hand: `molokolive-recolour` matches each
recoloured screenshot to its scene and fits a rule for the colours, which then applies to every scene.

To make your own palette:

1. Paint over a 1920x1080 screenshot of a scene and save it under `rip/recolours/`.
2. Fit the palette: `molokolive-recolour rip/recolours/mine.png:cg_dream --name mine`
3. Rebuild the scenes with it: `molokolive-pack --palette mine neutral-lift`
4. Use it: `molokolive --palette mine`

## Tools

Each tool explains itself with `--help`.

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
| `src/molokolive/`, `tools/` | the tools above |
| `palettes/` | palettes: `*.hex` colour lists, `*.json` colour maps and fitted rules |
| `rip/` | files extracted from the game, recolours and showreels; ignored by git |

Nothing from the game is ever committed: extracted files stay in `rip/`, and the scene packs live in
`~/.local/share/molokolive`.
Commits follow scoped [Conventional Commits](https://www.conventionalcommits.org/), for example
`feat(engine): pause on battery`.
