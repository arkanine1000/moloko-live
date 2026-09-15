#!/usr/bin/env bash
# Rip image frames from "Milk outside a bag of milk outside a bag of milk" (Steam app 1604000).
# Extracts game/*.rpa into rip/raw, then copies every non-video file into rip/frames,
# keeping the archive's directory layout. Videos are listed in rip/skipped-videos.txt.
set -euo pipefail

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    cat <<'HELP'
usage: tools/rip.sh

Extract the files of "Milk outside a bag of milk outside a bag of milk" into rip/: the whole
archive into rip/raw, and every image, sound and script (no videos) into rip/frames.

environment:
  GAME   the game's folder
         (default: ~/.local/share/Steam/steamapps/common/Milk outside a bag of milk outside a bag of milk)
  OUT    where to extract to (default: rip/ in this repository)

example:
  GAME=/mnt/games/SteamLibrary/steamapps/common/"Milk outside a bag of milk outside a bag of milk" tools/rip.sh
HELP
    exit 0
fi

GAME="${GAME:-$HOME/.local/share/Steam/steamapps/common/Milk outside a bag of milk outside a bag of milk}"
OUT="${OUT:-$(dirname "$0")/../rip}"
VIDEO_RE='\.(webm|ogv|mp4|mkv|avi|mov|mpg|mpeg|m4v)$'

[ -d "$GAME/game" ] || { echo "game dir not found: $GAME" >&2; exit 1; }
mkdir -p "$OUT/raw" "$OUT/frames"

shopt -s nullglob
archives=("$GAME"/game/*.rpa)
[ ${#archives[@]} -gt 0 ] || { echo "no .rpa archives in $GAME/game" >&2; exit 1; }

for rpa in "${archives[@]}"; do
    echo "extracting $(basename "$rpa")"
    uvx unrpa -mp "$OUT/raw" "$rpa"
done

# Loose files next to the archives (some Ren'Py builds ship images unarchived).
(cd "$GAME/game" && find . -type f ! -name '*.rpa' ! -name '*.rpyc' ! -name '*.rpymc' -print0) |
    while IFS= read -r -d '' f; do mkdir -p "$OUT/raw/$(dirname "$f")"; cp -n "$GAME/game/$f" "$OUT/raw/$f"; done

: > "$OUT/skipped-videos.txt"
(cd "$OUT/raw" && find . -type f -print0) | while IFS= read -r -d '' f; do
    if [[ "${f,,}" =~ $VIDEO_RE ]]; then
        echo "$f" >> "$OUT/skipped-videos.txt"
    elif [[ "${f,,}" =~ \.(png|webp|jpe?g|gif|bmp|avif)$ ]]; then
        mkdir -p "$OUT/frames/$(dirname "$f")"
        cp -n "$OUT/raw/$f" "$OUT/frames/$f"
    fi
done

echo "frames: $(find "$OUT/frames" -type f | wc -l)  skipped videos: $(wc -l < "$OUT/skipped-videos.txt")"
