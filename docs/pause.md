# Pausing

Animation runs only while all three hold: the desktop can be seen, the machine is on mains power, and the CPU is
below `--max-temp`. Otherwise every timer stops, the last frame stays on screen, and the process blocks in
`poll` until an event arrives. `pause.rs` gathers the three; `i3.rs`, `power.rs` and `thermal.rs` watch them.

## Visible

The engine subscribes to i3's workspace, window, output and shutdown events on one IPC connection and answers
each event with a tree query on a second one. The desktop counts as covered when a visible workspace holds any
tiled window, or a container is fullscreen (workspaces themselves always report `fullscreen_mode` 1, so only
containers count; global fullscreen covers every output). Floating windows leave most of the desktop visible and
do not count.

With i3's `smart_gaps on`, one tiled window fills the workspace and hides the wallpaper completely, which is why
a single tiled window means covered.

The socket path comes from `$I3SOCK` or the root window's `I3_SOCKET_PATH` property. When i3 restarts, the
connection is lost; the engine keeps the last known state and reconnects two seconds later. `--ignore-covered`
skips i3 entirely.

Measured: covered, the engine uses 0 % CPU with zero context switches over ten seconds. Animation resumes about
4 ms after switching to an empty workspace and pauses about 3 ms after switching back.

## On mains

A netlink socket for kernel uevents says when a power supply changed; the engine then reads
`/sys/class/power_supply/*/online` for every supply of type `Mains`. A machine without a mains supply counts as
on mains. `--ignore-battery` skips this.

## Cool enough

`--max-temp DEGREES` reads the `x86_pkg_temp` thermal zone every five seconds, but only while nothing else stops
the engine, so a covered desktop on battery wakes for nothing. Stopped for heat, the engine resumes once the
temperature is five degrees below the limit. There is no limit by default.

## Resuming

Animations continue where they were: the current image stays and its hold starts over, so nothing is replayed.
The same rule applies after a stall of more than a second (a suspend, a stopped process): the next step counts
from now rather than replaying the backlog. Autoplay counts only animated time, so a covered desktop does not
use it up.
