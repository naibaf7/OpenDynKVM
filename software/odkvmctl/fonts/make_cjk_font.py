#!/usr/bin/env python3
"""Makes `NotoSansSC-UI.otf`, the Chinese text font of opendyn-kvm, from Noto Sans CJK SC (Adobe and Google; SIL OFL
1.1, `OFL-NotoSansCJK.txt` next to this script).

The window's text font (`AdwaitaSans-UI.ttf`, `make_ui_font.py`) is Latin only, and egui's own fonts have no Chinese
either. This one follows it in the font list, cut down to what the Chinese translation writes:

- **One weight:** the variable font fixed at wght 400 (regular, as the Latin text).
- **Only the characters used:** every character of `../i18n/zh/odkvmctl.ftl` from U+2E80 on (CJK and full-width
  punctuation). Letters, digits and the rest stay with the Latin font. A test in `look.rs` checks the fonts against
  the translations: run this again after a Chinese message gains a character.

HarfBuzz's `hb-subset` does the work: fontTools 4.68 fails to instance this CFF2 font (and to turn it into CFF).
fontTools only finds the face and checks the result.

    uv run software/odkvmctl/fonts/make_cjk_font.py [/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc]

Needs `hb-subset` (HarfBuzz 6 or later; Fedora's `harfbuzz-utils`) and fontTools (in the repository's environment, DEVELOPMENT.md). The source:
Fedora's `google-noto-sans-cjk-vf-fonts` 2.004, or https://github.com/notofonts/noto-cjk
(Sans/Variable/OTC/NotoSansCJK-VF.otf.ttc), face "Noto Sans CJK SC".
"""

import subprocess
import sys
from pathlib import Path

from fontTools.ttLib import TTCollection, TTFont

SOURCE = "/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc"
FACE = "Noto Sans CJK SC"
HERE = Path(__file__).resolve().parent
OUT = HERE / "NotoSansSC-UI.otf"
TRANSLATION = HERE.parent / "i18n" / "zh" / "odkvmctl.ftl"
FIRST = 0x2E80  # CJK Radicals Supplement: everything from here on is for this font


def main() -> None:
    source = sys.argv[1] if len(sys.argv) > 1 else SOURCE
    names = [f["name"].getDebugName(4) for f in TTCollection(source, lazy=True).fonts]
    if FACE not in names:
        sys.exit(f"no face {FACE!r} in {source}")
    face = names.index(FACE)

    text = TRANSLATION.read_text(encoding="utf-8")
    wanted = sorted({ord(c) for c in text if ord(c) >= FIRST})
    unicodes = ",".join(f"{c:X}" for c in wanted)
    subprocess.run(
        [
            "hb-subset",
            source,
            f"--face-index={face}",
            "--variations=wght=400",
            f"--unicodes={unicodes}",
            "--layout-features=*",
            "--name-IDs=*",
            "--name-languages=*",
            "--no-hinting",
            f"--output-file={OUT}",
        ],
        check=True,
    )

    font = TTFont(OUT, recalcTimestamp=False)  # keep the source's dates: the same input, the same file
    missing = [chr(c) for c in wanted if c not in font.getBestCmap()]
    if missing:
        sys.exit(f"{OUT.name} lacks {''.join(missing)!r}")
    if "fvar" in font:
        sys.exit(f"{OUT.name} is still variable")
    # hb-subset keeps the names of the variable font's default instance (Thin): name the weight it now has.
    for record in font["name"].names:
        if record.nameID in (3, 6, 17):
            record.string = record.toUnicode().replace("Thin", "Regular")
    font.save(OUT)
    print(f"{OUT.name}: {len(wanted)} characters, {OUT.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
