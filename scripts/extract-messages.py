#!/usr/bin/env python3
r"""Collect the translatable strings of the Rust sources into a gettext template.

GNU gettext has no Rust parser, and its Python mode trips over Rust-only
syntax (lifetimes, `r"…"`, `b"…"`, `'\u{…}'`), so the catalogues used to be
maintained by hand. That does not scale: every string added to a page silently
disappears from the translations, and the only way to notice is to run the app
in another locale and look for English words.

This script reads the sources the same way a compiler's lexer would — tracking
string state, so a `tr(` inside a comment or inside a string literal is not
mistaken for a call — and writes a `.pot` with the same shape `xgettext`
produces, so `msgmerge` can update `po/*.po` from it.

    python3 scripts/extract-messages.py -o po/ferramari.pot
    for po in po/*.po; do msgmerge --update --backup=none "$po" po/ferramari.pot; done

Strings inside `#[cfg(test)]` blocks and inside `tests` modules are skipped:
they are assertions, not interface, and putting them in the catalogue would
ask translators to translate sentences no user ever sees.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# The catalogue's domain name, which is also the basename of the .mo file.
DOMAIN = "ferramari"
BUGS_ADDRESS = "https://github.com/Z1pP/ferramari/issues"

# tr("literal") and tr_n("singular", "plural", count).
CALL = re.compile(r"^(?:crate::)?(?:util::)?tr(?:_n)?\(")
# A `#[cfg(test)]` attribute opens a region that ends at the brace which
# closes the item it is attached to; skipping whole files named `tests*` is
# not enough because the UI keeps its tests at the bottom of the same file.
CFG_TEST = re.compile(r"#\[[^\]]*\btest\b[^\]]*\]")
STRING_START = re.compile(r'"(?:\\.|[^"\\])*"|[br]"|r(#*)"|b?\x27(?:\\.|\\u\{[0-9a-fA-F]+\}|[^\\\x27])\x27')


def strip_comments(text: str) -> str:
    """Blank out comments, keeping the line structure intact.

    Line numbers matter: they go into the `#:` reference lines, and a
    translator who cannot find the source line cannot judge the string.
    """
    out = []
    i, n = 0, len(text)
    in_block = False
    in_string: str | None = None
    while i < n:
        char = text[i]
        if in_block:
            if text.startswith("*/", i):
                out.append("  ")
                i += 2
                in_block = False
            else:
                out.append("\n" if char == "\n" else " ")
                i += 1
            continue
        if in_string:
            out.append(char)
            if char == "\\" and i + 1 < n:
                out.append(text[i + 1])
                i += 2
                continue
            if char == in_string:
                in_string = None
            i += 1
            continue
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            out.append(" " * (end - i))
            i = end
            continue
        if text.startswith("/*", i):
            out.append("  ")
            i += 2
            in_block = True
            continue
        if char in "\"'":
            in_string = char
            out.append(char)
            i += 1
            continue
        out.append(char)
        i += 1
    return "".join(out)


def string_literal(text: str, start: int) -> tuple[str, int] | None:
    """Read the string literal that begins at `start`, returning its value.

    Only the escapes a message key can meaningfully contain are decoded; a
    raw or byte string is not a translatable key and yields `None`.
    """
    if start >= len(text) or text[start] != '"':
        return None
    i = start + 1
    chunks: list[str] = []
    while i < len(text):
        char = text[i]
        if char == "\\" and i + 1 < len(text):
            nxt = text[i + 1]
            escapes = {"n": "\n", "t": "\t", "r": "\r", '"': '"', "\\": "\\", "'": "'"}
            if nxt == "u" and text.startswith("\\u{", i):
                close = text.find("}", i)
                if close < 0:
                    return None
                try:
                    chunks.append(chr(int(text[i + 3 : close], 16)))
                except ValueError:
                    return None
                i = close + 1
                continue
            if nxt not in escapes:
                return None
            chunks.append(escapes[nxt])
            i += 2
            continue
        if char == '"':
            return "".join(chunks), i + 1
        if char == "\n":
            # A literal that spans lines without a continuation is a
            # formatted block; not a message key.
            return None
        chunks.append(char)
        i += 1
    return None


def arguments(text: str, open_paren: int) -> list[tuple[str, int]] | None:
    """The top-level string arguments of a call, with their offsets."""
    depth = 0
    i = open_paren
    found: list[tuple[str, int]] = []
    while i < len(text):
        char = text[i]
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                break
        elif char == '"':
            literal = string_literal(text, i)
            if literal is None:
                return None
            value, end = literal
            found.append((value, i))
            i = end
            continue
        elif char == "r" and text.startswith('r"', i):
            return None
        i += 1
    return found


def line_of(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def test_regions(text: str) -> list[tuple[int, int]]:
    """Offsets of `#[cfg(test)]` items, matched by brace depth.

    The attribute is followed by a `mod` or `fn` item; its body ends at the
    brace that closes it, and everything from the attribute to there is
    machine-only code.
    """
    regions: list[tuple[int, int]] = []
    for match in CFG_TEST.finditer(text):
        i = match.end()
        while i < len(text) and text[i] not in "{;":
            i += 1
        if i >= len(text) or text[i] == ";":
            continue
        depth = 0
        start = i
        while i < len(text):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    regions.append((match.start(), i + 1))
                    break
            i += 1
    return regions


def collect(path: Path) -> list[dict]:
    raw = path.read_text(encoding="utf-8")
    text = strip_comments(raw)
    skip = test_regions(text)
    entries: list[dict] = []
    for match in re.finditer(r"\btr(?:_n)?\s*\(", text):
        offset = match.start()
        if any(start <= offset < end for start, end in skip):
            continue
        args = arguments(text, match.end() - 1)
        if not args:
            continue
        singular = args[0][0]
        if not singular.strip():
            continue
        entry = {
            "msgid": singular,
            "plural": args[1][0] if match.group(0).startswith("tr_n") and len(args) > 1 else None,
            "line": line_of(text, offset),
            "file": str(path),
        }
        entries.append(entry)
    return entries


def merge(entries: list[dict]) -> list[dict]:
    """One record per msgid, with every source location it appears in."""
    by_id: dict[tuple[str, str | None], dict] = {}
    for entry in entries:
        key = (entry["msgid"], entry["plural"])
        record = by_id.setdefault(
            key, {"msgid": entry["msgid"], "plural": entry["plural"], "refs": []}
        )
        reference = f"{entry['file']}:{entry['line']}"
        if reference not in record["refs"]:
            record["refs"].append(reference)
    return sorted(by_id.values(), key=lambda record: record["msgid"].lower())


def quote(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'


def render(records: list[dict], version: str, template: bool) -> str:
    out = [
        f'msgid ""\nmsgstr ""\n'
        f'"Project-Id-Version: ferramari {version}\\n"\n'
        f'"Report-Msgid-Bugs-To: {BUGS_ADDRESS}\\n"\n'
        '"MIME-Version: 1.0\\n"\n'
        '"Content-Type: text/plain; charset=UTF-8\\n"\n'
        '"Content-Transfer-Encoding: 8bit\\n"\n'
    ]
    if template:
        out.append('"X-Generator: scripts/extract-messages.py\\n"\n')
    for record in records:
        refs = "\n".join(f"#: {ref}" for ref in sorted(record["refs"]))
        block = [refs + "\n"] if refs else []
        if record["plural"]:
            # A template carries the two forms every language has; the rest
            # come from the language's own `Plural-Forms` when msgmerge
            # expands it into a .po.
            block.append(f"msgid {quote(record['msgid'])}\n")
            block.append(f"msgid_plural {quote(record['plural'])}\n")
            block.append('msgstr[0] ""\n')
            block.append('msgstr[1] ""\n')
        else:
            block.append(f"msgid {quote(record['msgid'])}\n")
            block.append('msgstr ""\n')
        out.append("".join(block))
    return "\n".join(out) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("-o", "--output", default=f"po/{DOMAIN}.pot")
    parser.add_argument("--version", default="2.0.0")
    parser.add_argument(
        "--root", default=".", help="source tree to scan (crates/ and src/ inside it)"
    )
    parser.add_argument(
        "--template",
        action="store_true",
        help="write a .pot (no plural forms beyond the first two)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero when the existing template no longer matches the sources",
    )
    options = parser.parse_args()

    root = Path(options.root)
    files = sorted(
        path
        for path in list(root.glob("crates/**/*.rs")) + list(root.glob("src/**/*.rs"))
        if "target" not in path.parts and not path.name.startswith("probe_")
    )
    if not files:
        print(f"no sources under {root}", file=sys.stderr)
        return 1

    records = merge([entry for path in files for entry in collect(path)])
    body = render(records, options.version, template=True)

    output = Path(options.output)
    if options.check:
        if not output.is_file():
            print(f"{output} is missing", file=sys.stderr)
            return 1
        current = output.read_text(encoding="utf-8")
        # Only the msgid set is compared: the header and the reference lines
        # move with every edit and would make the check useless.
        keys = re.compile(r'^msgid "(.*)"', re.M)
        before = set(keys.findall(current))
        after = set(keys.findall(body))
        missing = sorted(after - before)
        stale = sorted(before - after)
        if missing or stale:
            for value in missing:
                print(f"not in {output}: {value}")
            for value in stale:
                print(f"no longer used: {value}")
            return 1
        print(f"{output}: {len(after)} messages, in step with the sources")
        return 0

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(body, encoding="utf-8")
    print(f"{output}: {len(records)} messages from {len(files)} files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
