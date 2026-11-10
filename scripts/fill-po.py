#!/usr/bin/env python3
"""Fill the untranslated entries of a .po file from a reviewed table.

The scope is deliberately narrow: an entry is touched only when it is
`#, fuzzy` (msgmerge matched a similar msgid and guessed) or when its msgstr
is empty. Everything a translator already finished is left alone, because the
file's existing wording is the project's decision, not this script's.

Why the guesses need replacing at all: msgmerge can only offer the closest
msgid it has seen, which is how `Nothing is playing` arrived carrying
`Сейчас воспроизводится` — the opposite meaning. A fuzzy entry is not shown to
the user (gettext falls back to English), so it is a to-do rather than a
defect; but a to-do nobody can see never gets done.

    python3 scripts/fill-po.py po/ru.po --table scripts/translations-ru.py
    python3 scripts/fill-po.py po/ru.po --table scripts/translations-ru.py --list-missing

The table is a module defining `TRANSLATIONS: dict[str, str]`, keyed by the
exact msgid. Plurals may carry a second entry under
`msgid + "\\x00plural"`; without it every form gets the same wording, which is
correct for the nouns Russian declines identically (`видео`) and merely
imprecise for the rest.
"""
import argparse
import importlib.util
import re
import sys
from pathlib import Path

# A long msgid is folded by gettext into an empty first line plus continuation
# lines, so reading only `msgid "…"` returns "" for those entries and they
# look exactly like the file header.
CONTINUED = re.compile(r'^"(.*)"$', re.M)
# One block's whole run of `msgstr` lines, so a plural group is replaced in a
# single pass. Substituting line by line would insert the full group at every
# index and leave the entry with duplicated, invalid forms.
MSGSTR_RUN = re.compile(r'^msgstr(?:\[\d+\])? ".*"(?:\nmsgstr(?:\[\d+\])? ".*")*', re.M)
OBSOLETE = re.compile(r"^#~")


def quote(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def unquote(value: str) -> str:
    return value.replace('\\"', '"').replace("\\\\", "\\")


def is_obsolete(block: str) -> bool:
    """Whether msgmerge parked the entry as no longer used (`#~` lines)."""
    body = [line for line in block.splitlines() if line.strip() and not line.startswith("#:")]
    return bool(body) and all(OBSOLETE.match(line) for line in body)


def msgid_of(block: str) -> str | None:
    """The full msgid of an entry, folding gettext's continuation lines.

    Returns `None` for a block with no msgid, and `""` for the header, whose
    msgid is empty and which `is_header` then recognises.
    """
    lines = block.splitlines()
    for index, line in enumerate(lines):
        if not line.startswith("msgid "):
            continue
        value = line[len("msgid ") :]
        if not (value.startswith('"') and value.endswith('"') and len(value) >= 2):
            return None
        parts = [value[1:-1]]
        # Continuation lines follow immediately and are bare quoted strings.
        for follow in lines[index + 1 :]:
            if not re.match(r'^".*"$', follow):
                break
            parts.append(follow[1:-1])
        return unquote("".join(parts))
    return None


def is_header(block: str) -> bool:
    """The file's own metadata block: `msgid ""` with no folded text.

    Its msgstr is a run of quoted continuation lines, which the entry scan
    below cannot read, and it is never a missing translation.
    """
    return msgid_of(block) == ""


def needs_work(block: str) -> bool:
    if is_obsolete(block) or is_header(block):
        return False
    if msgid_of(block) is None:
        return False
    if "#, fuzzy" in block:
        return True
    match = MSGSTR_RUN.search(block)
    return match is None or all(
        not unquote(part) for part in re.findall(r'"(.*)"', match.group(0))
    )


def forms_are_flat(block: str) -> bool:
    """Whether a plural entry repeats one string across every form.

    `msgmerge` does this when it brings a plural entry forward: it has no way
    to know the language's forms, so it copies the first one. For a language
    like Japanese that is correct and must be left alone; for Russian it is
    wrong — `1 предмет`, `2 предмета`, `9 предметов` are three different
    words — and the entry reads as filled while showing the wrong case to
    every user. It is detected here rather than by the fuzzy flag because the
    flag is already gone by the time the entry looks complete.
    """
    if "msgid_plural" not in block:
        return False
    values = [
        unquote(value)
        for value in re.findall(r'^msgstr\[\d+\] "(.*)"', block, re.M)
    ]
    return len(values) > 1 and len(set(values)) == 1 and bool(values[0])


def apply_block(block: str, table: dict[str, str]) -> str | None:
    msgid = msgid_of(block)
    if msgid is None or msgid not in table:
        return None
    value = table[msgid]

    if "msgid_plural" in block:
        indexes = re.findall(r'^msgstr\[(\d+)\] ".*"$', block, re.M)
        if not indexes:
            return None
        # Form 1 is the translation of `msgid_plural` itself, which the table
        # holds under that msgid as an ordinary entry. Languages with three or
        # four forms (Russian, Czech) need one more string beyond that; the
        # table supplies it as `msgid + "\x00many"`, and languages that do not
        # distinguish simply leave it out.
        plural_id = re.search(r'^msgid_plural "(.*)"', block, re.M)
        second = (
            table.get(unquote(plural_id.group(1)), value) if plural_id else value
        )
        third = table.get(msgid + "\x00many", second)
        by_index = {"0": value, "1": second}

        def form(index: str) -> str:
            return by_index.get(index, third)

        replacement = "\n".join(f"msgstr[{index}] {quote(form(index))}" for index in indexes)
        block = MSGSTR_RUN.sub(lambda _match: replacement, block, count=1)
    else:
        if not MSGSTR_RUN.search(block):
            return None
        block = MSGSTR_RUN.sub(lambda _match: f"msgstr {quote(value)}", block, count=1)

    # The flag is what keeps the entry out of the compiled catalogue, and the
    # reason it was a guess in the first place.
    block = re.sub(r"^#, fuzzy\n", "", block, flags=re.M)
    block = re.sub(r"^#, *\n", "", block, flags=re.M)
    return block


def load_table(path: str) -> dict[str, str]:
    spec = importlib.util.spec_from_file_location("translations", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load table {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    table = getattr(module, "TRANSLATIONS", None)
    if not isinstance(table, dict):
        raise SystemExit(f"{path} defines no TRANSLATIONS dict")
    return table


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("po")
    parser.add_argument("--table", required=True, help="module defining TRANSLATIONS")
    parser.add_argument(
        "--list-missing",
        action="store_true",
        help="print the msgids that still need work and change nothing",
    )
    parser.add_argument(
        "--list-untranslated",
        action="store_true",
        help="print every msgid the table does not cover, and change nothing",
    )
    options = parser.parse_args()

    table = load_table(options.table)
    path = Path(options.po)
    blocks = path.read_text(encoding="utf-8").split("\n\n")

    if options.list_missing or options.list_untranslated:
        for block in blocks:
            if is_header(block) or is_obsolete(block):
                continue
            msgid = msgid_of(block)
            if msgid is None:
                continue
            if options.list_missing:
                if needs_work(block):
                    print(msgid)
            elif msgid not in table:
                print(msgid)
        return 0

    def worth_rewriting(block: str) -> bool:
        """Whether this entry is one the table is allowed to touch.

        A flat plural block counts as needing work only when the table itself
        declares a further form for it. Japanese has one plural form, so its
        repeated `msgstr[0]`/`msgstr[1]` is the right answer and rewriting it
        every run would be churn, not a fix; the same is true of the Russian
        words that do not decline (`видео`).
        """
        if needs_work(block):
            return True
        if not forms_are_flat(block):
            return False
        return (msgid_of(block) or "") + "\x00many" in table

    filled = 0
    skipped: list[str] = []
    out = []
    for block in blocks:
        if not worth_rewriting(block):
            out.append(block)
            continue
        result = apply_block(block, table)
        if result is None:
            skipped.append(msgid_of(block) or "")
            out.append(block)
        else:
            filled += 1
            out.append(result)

    path.write_text("\n\n".join(out), encoding="utf-8")
    print(f"{path}: filled {filled} entries")
    if skipped:
        print(f"{len(skipped)} still open:")
        for msgid in skipped:
            print(f"  {msgid}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
