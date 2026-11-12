#!/usr/bin/env python3
"""Makes `AdwaitaSans-UI.ttf`, the text font of opendyn-kvm, from Adwaita Sans (GNOME's UI font, derived from Inter;
SIL OFL 1.1, no Reserved Font Name; `OFL.txt` next to this script).

egui's own font is Ubuntu *Light*, thin and faint as dark text on a light background. This is the regular weight,
cut down to what the window writes:

- **One weight:** the variable font fixed at wght 400 and the text optical size (opsz 14).
- **Even digits:** the tabular figures (Inter's `tnum`) take the digits' places, so a live value (5.05 V, 1200 mA)
  doesn't shift the text after it as it changes. egui can't ask for a feature; the font answers with them by default.
- **Latin only:** Basic Latin, Latin-1, Latin Extended-A/B and combining accents (English, French, German, Norwegian
  and names), punctuation, currency, letterlike symbols, arrows and math operators, and every other character the
  translations (`../i18n/*/odkvmctl.ftl`) use, such as French's "nᵒ". Everything else (symbols, shapes, emoji, other
  scripts) falls through to egui's own fonts, which the window's icons are chosen from. A test in `look.rs` checks
  the font against the translations: run this again after adding a character it lacks.
- The kerning, contextual alternates and other default features stay: egui shapes text with harfrust.

    python3 -I software/odkvmctl/fonts/make_ui_font.py [/usr/share/fonts/adwaita-sans-fonts/AdwaitaSans-Regular.ttf]

Needs fontTools (`pip install fonttools`). The source: Fedora's `adwaita-sans-fonts` 51.0, or
https://gitlab.gnome.org/GNOME/adwaita-fonts (fonts/AdwaitaSans-Regular.ttf).
"""

import sys
from pathlib import Path

from fontTools.subset import Options, Subsetter
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

SOURCE = "/usr/share/fonts/adwaita-sans-fonts/AdwaitaSans-Regular.ttf"
HERE = Path(__file__).resolve().parent
OUT = HERE / "AdwaitaSans-UI.ttf"
TRANSLATIONS = HERE.parent / "i18n"

RANGES = [
    (0x0020, 0x007E),  # Basic Latin
    (0x00A0, 0x00FF),  # Latin-1 Supplement
    (0x0100, 0x024F),  # Latin Extended-A and -B
    (0x0300, 0x036F),  # Combining diacritical marks
    (0x2000, 0x206F),  # General punctuation
    (0x20A0, 0x20CF),  # Currency
    (0x2100, 0x214F),  # Letterlike symbols
    (0x2190, 0x21FF),  # Arrows
    (0x2200, 0x22FF),  # Mathematical operators
]


def in_ranges(code: int) -> bool:
    return any(lo <= code <= hi for lo, hi in RANGES)


def tabular_digits(font: TTFont) -> None:
    """Points the cmap's digits at the glyphs the `tnum` feature would substitute."""
    gsub = font["GSUB"].table
    subst = {}
    for record in gsub.FeatureList.FeatureRecord:
        if record.FeatureTag != "tnum":
            continue
        for index in record.Feature.LookupListIndex:
            for sub in gsub.LookupList.Lookup[index].SubTable:
                sub = getattr(sub, "ExtSubTable", sub)
                subst.update(getattr(sub, "mapping", {}))
    if not subst:
        sys.exit("no tnum substitutions in the source font")
    changed = 0
    for table in font["cmap"].tables:
        for code in range(ord("0"), ord("9") + 1):
            glyph = table.cmap.get(code)
            if glyph in subst:
                table.cmap[code] = subst[glyph]
                changed += 1
    if not changed:
        sys.exit("the digits have no tabular forms")


def main() -> None:
    source = sys.argv[1] if len(sys.argv) > 1 else SOURCE
    # The source's timestamp, so the same source gives the same file.
    font = TTFont(source, recalcTimestamp=False)
    font = instantiateVariableFont(font, {"wght": 400, "opsz": 14})
    tabular_digits(font)
    options = Options()
    options.layout_features = ["*"]
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.notdef_outline = True
    options.drop_tables += ["STAT"]
    subsetter = Subsetter(options)
    wanted = {c for lo, hi in RANGES for c in range(lo, hi + 1)}
    for ftl in sorted(TRANSLATIONS.glob("*/odkvmctl.ftl")):
        wanted |= {ord(ch) for ch in ftl.read_text(encoding="utf-8") if ch.isprintable()}
    lacking = sorted(c for c in wanted if c > 0x7E and c not in font.getBestCmap() and not in_ranges(c))
    if lacking:
        print("not in the source font (egui's fonts will draw them):", " ".join(chr(c) for c in lacking))
    subsetter.populate(unicodes=sorted(wanted))
    subsetter.subset(font)
    font.save(OUT)
    print(f"{OUT}: {OUT.stat().st_size} bytes, {len(font.getGlyphOrder())} glyphs")


if __name__ == "__main__":
    main()
