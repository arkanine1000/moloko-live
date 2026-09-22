#!/usr/bin/env bash
# Extract the files of "Milk outside a bag of milk outside a bag of milk" (Steam app 1604000): the whole archive
# into rip/raw, and every image into rip/frames, keeping the archive's directory layout. Videos are listed in
# rip/skipped-videos.txt and copied nowhere. Existing files are never overwritten.
set -euo pipefail

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    cat <<'HELP'
usage: tools/rip.sh

Extract the files of "Milk outside a bag of milk outside a bag of milk" into rip/: the whole
archive into rip/raw, and every image (no videos) into rip/frames. Needs uv, which fetches
unrpa the first time.

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
OUT="${OUT:-$(cd "$(dirname "$0")/.." && pwd)/rip}"
VIDEO_RE='\.(webm|ogv|mp4|mkv|avi|mov|mpg|mpeg|m4v)$'
IMAGE_RE='\.(png|webp|jpe?g|gif|bmp|avif)$'

[ -d "$GAME/game" ] || { echo "game dir not found: $GAME" >&2; exit 1; }
command -v uvx >/dev/null || { echo "uvx not found: install uv (https://docs.astral.sh/uv/) to run unrpa" >&2; exit 1; }
mkdir -p "$OUT/raw" "$OUT/frames"

shopt -s nullglob
archives=("$GAME"/game/*.rpa)
[ ${#archives[@]} -gt 0 ] || { echo "no .rpa archives in $GAME/game" >&2; exit 1; }

for rpa in "${archives[@]}"; do
    echo "extracting $(basename "$rpa")"
    uvx unrpa -mp "$OUT/raw" "$rpa"
done

# Loose files next to the archives (some Ren'Py builds ship images unarchived).
while IFS= read -r -d '' f; do
    mkdir -p "$OUT/raw/$(dirname "$f")"
    cp -n "$GAME/game/$f" "$OUT/raw/$f"
done < <(cd "$GAME/game" && find . -type f ! -name '*.rpa' ! -name '*.rpyc' ! -name '*.rpymc' -print0)

: > "$OUT/skipped-videos.txt"
copied=0 kept=0
while IFS= read -r -d '' f; do
    if [[ "${f,,}" =~ $VIDEO_RE ]]; then
        echo "$f" >> "$OUT/skipped-videos.txt"
    elif [[ "${f,,}" =~ $IMAGE_RE ]]; then
        if [ -e "$OUT/frames/$f" ]; then
            kept=$((kept + 1))
        else
            mkdir -p "$OUT/frames/$(dirname "$f")"
            cp "$OUT/raw/$f" "$OUT/frames/$f"
            copied=$((copied + 1))
        fi
    fi
done < <(cd "$OUT/raw" && find . -type f -print0)

echo "frames: $copied copied, $kept already there  skipped videos: $(wc -l < "$OUT/skipped-videos.txt")"
