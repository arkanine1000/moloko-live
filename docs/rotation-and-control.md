# Scene rotation and control

One binary does both jobs. `molokolive [OPTIONS]` is the engine; `molokolive COMMAND` connects to the running
engine's socket, sends one line and prints the one-line reply.

## Order

`rotation.rs` keeps the sequence. With `--shuffle on` (the default) the scenes are a shuffled deck: every scene
shows once before the deck is dealt again, and the scene on screen is never dealt next. With `--shuffle off` the
scenes play in the order `--scenes --list` shows, wrapping around. Either way `prev` walks back through the
last 100 scenes shown and `next` walks forward again before dealing new ones.

`--scenes A,B,...` and `--skip-scenes A,B,...` choose the scenes, both with `*` wildcards; `--scenes --list`
previews the selection. `--start-scene NAME` picks the first scene, which must be in the rotation.

## Autoplay

`--autoplay SECONDS` (60 by default, `off` to stay on one scene) changes scene after that much animation time.
Time spent paused does not count. A manual change restarts the clock.

## Skies

Every scene change picks a random sky from the scene's pool. `--persist-sky` makes each scene keep the sky it
last had, manual steps included. `sky-next` and `sky-prev` step through the pool in order; `--sky N` starts with
sky N, where N is the game's file number as `status` shows it.

Scene changes are hard cuts. The previous scene's pixmap, picture and shared memory are freed; thirty fast changes
in a row leave one mapping and the same resident size.

## The socket

Path: `$XDG_RUNTIME_DIR/molokolive.sock`, falling back to the temp directory. The engine binds it right after
publishing its window, which has ended the previous engine, so the path is free. A client connects, writes one
line, reads one line, and closes. A client that sends nothing within 200 ms is dropped.

| Command | Reply |
|---|---|
| `next`, `prev` | the new scene and its picks, e.g. `cg_floor (sky 20)`; `... (no earlier scene)` at the start of the history |
| `sky-next`, `sky-prev` | the scene and its new sky, or `NAME has no sky` |
| `pause`, `resume` | the pause state, e.g. `paused: by hand, on battery` or `animating` |
| `status` | `cg_floor (sky 20): animating; next scene after 42 s more animation`, or `...: paused: desktop covered` |

A reply starting with `error: ` is printed on stderr by the client, which then exits with status 1. The pause
reasons, in a fixed order, are `by hand`, `desktop covered`, `on battery` and `too hot (N °C)`.

## i3

```
exec --no-startup-id molokolive --output window
bindsym $mod+semicolon exec --no-startup-id molokolive prev
bindsym $mod+apostrophe exec --no-startup-id molokolive next
bindsym $mod+Shift+semicolon exec --no-startup-id molokolive sky-prev
bindsym $mod+Shift+apostrophe exec --no-startup-id molokolive sky-next
```

`exec` rather than `exec_always`, so an i3 reload keeps the rotation. `--output window` rather than `auto`,
because picom may not own the screen yet when i3 starts the wallpaper (see [rendering.md](rendering.md)). The
usual wallpaper command (feh) stays in the config: its picture shows whenever molokolive is not running.
