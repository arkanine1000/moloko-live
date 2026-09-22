# Drift

In the game the skies drift slowly, and so do the reflections in the mirror scenes. The wallpaper does the same,
in whole native pixels, redrawing only the pixels that actually change. `--no-drift` turns it off.

## The transforms

`script.rpy` defines two repeating transforms:

```
transform circle:            # skies
    ease 2 yoffset 2
    ease 2 xoffset 2
    ease 2 yoffset -2
    ease 2 xoffset -2
    repeat

transform circle2:           # cg_mirror_gg reflection pools
    ease 2 xoffset -2
    ease 2 yoffset -2
    ease 2 xoffset 0
    ease 2 yoffset 0
    repeat
```

Offsets are full-resolution pixels; two of them make one native pixel. Ren'Py's `ease` is
`0.5 - cos(pi t) / 2`.

## Steps

`renpy.parse_transform` turns a transform into a cycle of whole-pixel steps. An offset changes where the eased
full-resolution offset crosses half a native pixel, so a tween over two native pixels steps twice. The first pass
starts from rest; the cycle is the steady state after it, and a layer starts at the offset the cycle ends on.
`circle` gives 8 steps per 8 s cycle, swinging between -1 and +1 native pixel on each axis; `circle2` gives 4,
between 0 and -1.

The pack builder writes each layer's cycle into the manifest as `{"period": s, "steps": [[t, dx, dy], ...]}`.
The engine's `Drift` keeps the rhythm from each step's due time and, like animations, drops the backlog after a
stall of more than a second.

## Only the changed pixels

The sky shows only through a scene's transparent parts, and a move by one pixel changes only the pixels that
differ from their neighbour. At each step the engine recomposites the whole canvas into a scratch buffer,
compares it with the frame on screen in 16 px tiles, and draws only the tiles that differ. Per step, 0.1 to
1.8 % of the pixels change for skies and up to 3.7 % for reflections.

Those changes are thin and spread across the canvas, so drift frames may use up to 256 draws instead of the
usual 16: merged into 16 strips they would damage 40 to 90 % of the screen, uncapped 9 to 31 %. Drift alone
costs 0.2 to 0.3 % of a core and about 1 to 2 ms of X time per step.

`molokolive-showreel` reproduces the same steps, rounded to video frames.
