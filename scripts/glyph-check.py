#!/usr/bin/env python3
"""Check that every icon name the UI uses is a glyph the installed font has.

The icon font is a system font, not a bundled asset, and Font Awesome's
codepoints live in the Private Use Area. Two failures follow from that and
both are silent:

* a name missing from `style::glyph` falls through to the default arm, so the
  UI draws a dot where a filmstrip was intended;
* a codepoint borrowed from Font Awesome 5 exists in the table but not in the
  4.7 font, so it draws *some other glyph* — a different wrong icon on a
  different machine.

Neither is visible in a screenshot unless you know what was meant to be there,
so the check reads the font's own charset with `fc-query` and compares it
against the table and every name the sources hand to an icon helper.

    python3 scripts/glyph-check.py
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Names that reach `glyph()` as a variable rather than a literal: the rail and
# the category tiles. Their arguments are enumerated below and checked too.
CALLS = re.compile(
    r'(?:s::)?(?:icon|icon_button|heading_icon|primary_icon|secondary_icon|tile_button|glyph)'
    r'\s*(?::<[^>]*>)?\s*\(\s*"([a-z0-9\-]+)"'
)
FIELD = re.compile(r'(?:icon|glyph_name|label_icon|icon_name)\s*[:=]\s*"([a-z0-9\-]+)"')
BRANCH = re.compile(r'\{\s*"([a-z0-9\-]+)"\s*\}')
TABLE = re.compile(r'"([a-z0-9\-]+)" => \'\u005cu\{([0-9a-fA-F]{4})\}\'')
FALLBACK = re.compile(r'_ => \'\u005cu\{([0-9a-fA-F]{4})\'')


def font_path() -> str | None:
    """The installed Font Awesome face, whichever of the two the system has."""
    for candidate in (
        "/usr/share/fonts/truetype/font-awesome/fontawesome-webfont.ttf",
        "/usr/share/fonts/opentype/font-awesome/FontAwesome.otf",
    ):
        if Path(candidate).is_file():
            return candidate
    found = subprocess.run(
        ["fc-match", "-f", "%{file}", "FontAwesome"],
        capture_output=True,
        text=True,
    )
    return found.stdout.strip() or None


def covered_ranges(path: str) -> list[tuple[int, int]]:
    """The font's own charset, as `fc-query` reports it: `f000-f00e f010 …`."""
    report = subprocess.run(
        ["fc-query", "--format=%{charset}", path],
        capture_output=True,
        text=True,
    )
    if report.returncode != 0:
        raise SystemExit(f"fc-query failed for {path}: {report.stderr.strip()}")
    ranges = []
    for part in report.stdout.split():
        if "-" in part:
            low, high = part.split("-", 1)
        else:
            low = high = part
        try:
            ranges.append((int(low, 16), int(high, 16)))
        except ValueError:
            continue
    return ranges


def referenced_names() -> set[str]:
    names: set[str] = set()
    for source in (ROOT / "crates/ferramari-ui/src/main.rs", ROOT / "crates/ferramari-ui/src/style.rs"):
        text = source.read_text(encoding="utf-8")
        names.update(CALLS.findall(text))
        names.update(FIELD.findall(text))
        names.update(BRANCH.findall(text))
    return names


def main() -> int:
    style = (ROOT / "crates/ferramari-ui/src/style.rs").read_text(encoding="utf-8")
    table = dict(TABLE.findall(style))
    fallback = FALLBACK.search(style)
    if not table:
        raise SystemExit("the glyph table was not found; check the pattern against style.rs")

    path = font_path()
    if path is None:
        print("Font Awesome is not installed; icons will render as fallback tofu.")
        return 1
    ranges = covered_ranges(path)

    def has(codepoint: int) -> bool:
        return any(low <= codepoint <= high for low, high in ranges)

    names = referenced_names()
    unknown = sorted(name for name in names if name not in table)
    absent = sorted(
        (name, codepoint)
        for name, codepoint in table.items()
        if not has(int(codepoint, 16))
    )

    print(f"font:   {path}")
    print(f"table:  {len(table)} names, {len(names)} referenced")
    if fallback:
        print(f"fallback: U+{fallback.group(1).upper()}")
    status = 0
    if unknown:
        status = 1
        print("\nNames the UI passes that the table does not know (they render as the fallback):")
        for name in unknown:
            print(f"  {name}")
    if absent:
        status = 1
        print("\nCodepoints this font cannot draw:")
        for name, codepoint in absent:
            print(f"  {name} -> U+{codepoint.upper()}")
    if status == 0:
        print("\nAll icon names resolve to glyphs the installed font has.")
    return status


if __name__ == "__main__":
    sys.exit(main())
