# Architecture

Everything expensive happens once, offline. The engine only composites indexed images, converts the pixels that
changed through a lookup table, and hands them to the X server.

```
molokolive-pack (Python, offline)                 molokolive (Rust, runtime)
  rip/raw/*.rpy  +  rip/frames/images/            ┌────────────────────────────────────────────────┐
  palettes/*.hex, *.json                          │ pack      manifest, indexed layers, LUTs       │
        │                                          │ timeline  steps, holds, choice pools, drift    │
        ▼                                          │ scene     index compositing, dirty rectangles  │
  <packs>/<scene>/                                 │ render    LUT -> BGRX at native size           │
    manifest.json   layers, animations, drift      │ output    MIT-SHM upload, RENDER scaling       │
    images/*.png    8-bit indexed, 960x540         │ pause     i3 IPC, netlink power, sysfs thermal │
    luts/*.bin      256 x BGRA per palette         │ control   Unix socket: next, prev, pause ...   │
    preview/*.png                                  │ engine    poll(fds) with a deadline, no polling│
                                                   └────────────────────────────────────────────────┘
```

## Decisions

**Rust for the engine, Python for the tools.** The engine runs all day and must cost nothing while idle, so it is
a small binary with five dependencies and no runtime. The tools run once and want numpy and Pillow.

**One index space per scene.** All of a scene's layers, its sky pool included, share one palette of at most 255
colours plus index 0 for transparency. The most colourful scene uses 75. Layers are stored as indices, compositing
picks the topmost non-zero index per pixel, and a palette is a 256-entry lookup table per scene. Changing the
palette never touches layer data.

**Colour science at pack time.** Each (scene, palette) pair is baked into a final lookup table by the pack builder.
The engine does no colour maths.

**Draw only what changed.** A timeline step marks its layer's rectangles dirty; the engine recomposites those
rectangles, converts them through the lookup table, uploads them at native size and lets the X server scale them.
See [rendering.md](rendering.md).

**Paused is the default state.** Animation runs only while the desktop is visible, the machine is on mains power
and the CPU is below the limit. When it stops, timers stop too and the process blocks on its file descriptors.
See [pause.md](pause.md).

**Binary alpha.** Nearly all of the game's art has alpha 0 or 255. The few soft-edge pixels are snapped at pack
time so that index compositing stays exact.

## Modules of the engine

| File | Responsibility |
|---|---|
| `main.rs` | one binary: a client of the socket with a command, the engine otherwise |
| `cli.rs` | the options and help text |
| `engine.rs` | the loop: animate, draw, wait, events, commands, pause |
| `show.rs` | the scene on screen and its geometry |
| `scenes.rs` | the packs directory, `--scenes` and `--skip-scenes` with wildcards, the listing |
| `rotation.rs` | shuffled or in-order scene sequence with a history for `prev` |
| `frame.rs` | merging dirty rectangles into few draws, statistics |
| `pack.rs`, `pack/` | manifest loading, the loaded scene, animation and drift timing |
| `geometry.rs` | rectangles and the canvas-to-screen mapping |
| `render.rs` | index to BGRX conversion |
| `output.rs` | X11: the desktop window or root pixmap, MIT-SHM, RENDER |
| `pause.rs`, `i3.rs`, `power.rs`, `thermal.rs` | the three reasons to stop |
| `control.rs` | the Unix socket and its commands |

Dependencies: `x11rb` (with `render` and `shm`), `rustix` (poll, memfd, mmap, netlink), `png`, `serde` and
`serde_json`, `clap`.

## Modules of the tools

| File | Responsibility |
|---|---|
| `pack.py` | `molokolive-pack`: reads the game's images and scripts, writes packs |
| `scenes.py` | the scene table: which images and skies make each scene |
| `renpy.py` | the subset of Ren'Py the scenes use, parsed into dataclasses |
| `pixels.py` | grid sampling, zoom, dirty rectangles |
| `colour.py` | OKLab, hex colours, the palette kinds |
| `recolour.py` | `molokolive-recolour`: fits a lift rule to hand-recoloured screenshots |
| `showreel.py` | `molokolive-showreel`: renders packs to video with the engine's rules |
| `paths.py`, `cli.py` | shared locations, help formatting and error handling |
