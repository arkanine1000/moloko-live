#!/usr/bin/env python3
"""Extract Milk-chan's spoken English dialogue from the ripped Ren'Py translation files.

Reads rip/raw/tl/english/*.rpy dialogue blocks (menu choices live in separate `strings`
tables and are never read). Narrators and bare narration (her inner monologue) are skipped.
Each line is matched back to the shipped Russian source through the translator comment
(`# gg "..."`), which gives the real source line and the sprite on screen at that point.

  python tools/dialogue.py [--rip DIR] [--no-tel]

Writes rip/dialogue/milkchan_en.jsonl (one object per line) and milkchan_en.txt.
"""
import argparse, collections, json, re, sys
from pathlib import Path

RIP = Path(__file__).resolve().parent.parent / "rip"
# gg: main sprite speaker · ggz: eyelash scene · ngg/ngg2: milk flashback · ggm: shop ending
# tel: phone ending, messages she types (dash-prefixed like speech, but not gg-tagged)
SPEAKERS = {"gg", "ggz", "ngg", "ngg2", "ggm", "tel"}

SOURCE_REF = re.compile(r"^#\s*game/(\S+?):(\d+)\s*$")
BLOCK = re.compile(r"^translate english ((\w+?)(?:_[0-9a-f]{8}(?:_\d+)?)?):\s*$")
STRING = r'"((?:[^"\\]|\\.)*)"'
SAY = re.compile(rf"^\s*([a-z_]\w*)\s*{STRING}\s*(?:#.*)?$")  # translators left trailing notes
COMMENTED_SAY = re.compile(rf"^\s*#\s*([a-z_]\w*)\s*{STRING}")
SOURCE_SAY = re.compile(rf"^\s*([a-z_]\w*)\s*{STRING}")
SHOW, HIDE, SCENE = re.compile(r"^\s*show\s+(gg_\w+)"), re.compile(r"^\s*hide\s+(gg_\w+)"), re.compile(r"^\s*scene\b")
SPRITE = re.compile(r"^gg_(\w+?)_(\d+)(_ad|_oa)?$")
POSE = {None: "arms_crossed", "_ad": "arms_down", "_oa": "one_arm"}
TAGS = re.compile(r"\{[^{}]*\}")


def unescape(s):
    return s.replace('\\"', '"').replace("\\n", "\n")


def scan_source(path, speakers):
    """(speaker, text) -> [(line, sprite)] in file order. The sprite is the most recently shown
    gg_* image still on screen; a linear scan, so branches can occasionally leave a stale one."""
    index, shown = collections.defaultdict(list), []
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if m := SHOW.match(line):
            if m.group(1) in shown:
                shown.remove(m.group(1))
            shown.append(m.group(1))
        elif m := HIDE.match(line):
            if m.group(1) in shown:
                shown.remove(m.group(1))
        elif SCENE.match(line):
            shown.clear()
        elif (m := SOURCE_SAY.match(line)) and m.group(1) in speakers:
            index[(m.group(1), m.group(2))].append((n, shown[-1] if shown else None))
    return index


def extract(raw, speakers):
    rows, sources, unmatched = [], {}, 0
    for f in sorted((raw / "tl" / "english").glob("*.rpy")):
        lines = f.read_text(encoding="utf-8").splitlines()
        ref = None
        for i, line in enumerate(lines):
            if m := SOURCE_REF.match(line):
                ref = (m.group(1), int(m.group(2)))
                continue
            if not (m := BLOCK.match(line)) or line.startswith("translate english strings"):
                continue
            block_id, label, original = m.group(1), m.group(2), None
            for body in lines[i + 1:]:
                if body.startswith(("translate ", "# game/")):
                    break
                if c := COMMENTED_SAY.match(body):
                    original = (c.group(1), c.group(2))
                    continue
                if (s := SAY.match(body)) and s.group(1) in speakers:
                    if ref[0] not in sources:
                        sources[ref[0]] = scan_source(raw / ref[0], speakers)
                    hits = sources[ref[0]].get(original) if original else None
                    src_line, sprite = hits.pop(0) if hits else (None, None)
                    unmatched += src_line is None
                    sm = SPRITE.match(sprite) if sprite else None
                    text = unescape(s.group(2))
                    rows.append({"id": block_id, "file": ref[0], "tl_line": ref[1], "src_line": src_line, "label": label,
                                 "speaker": s.group(1), "sprite": sprite,
                                 "pose": POSE[sm.group(3)] if sm else None,
                                 "emotion": sm.group(1) if sm else None, "mood": sm.group(2) if sm else None,
                                 "text": TAGS.sub("", text).strip(), "raw": text})
    rows.sort(key=lambda r: (r["file"], r["src_line"] or 0, r["tl_line"]))
    return rows, unmatched


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rip", type=Path, default=RIP)
    ap.add_argument("--no-tel", action="store_true", help="skip the phone ending's typed messages")
    args = ap.parse_args()

    speakers = SPEAKERS - {"tel"} if args.no_tel else SPEAKERS
    rows, unmatched = extract(args.rip / "raw", speakers)
    if not rows:
        sys.exit("no dialogue found")
    out = args.rip / "dialogue"
    out.mkdir(exist_ok=True)
    with open(out / "milkchan_en.jsonl", "w", encoding="utf-8") as fh:
        fh.writelines(json.dumps(r, ensure_ascii=False) + "\n" for r in rows)
    (out / "milkchan_en.txt").write_text("\n".join(r["text"] for r in rows) + "\n", encoding="utf-8")

    counts = collections.Counter(r["speaker"] for r in rows)
    tagged = sum(1 for r in rows if r["sprite"])
    print(f"{len(rows)} lines -> {out}/milkchan_en.{{jsonl,txt}}  by speaker: {dict(counts)}")
    print(f"matched to source: {len(rows) - unmatched}/{len(rows)}  with sprite: {tagged}")


if __name__ == "__main__":
    main()
