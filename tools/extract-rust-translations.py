#!/usr/bin/env python3
"""Adds every tr!("...") string in the Rust crates to the .pot file (adapted from DriveDeck)."""
import ast
import json
import re
import sys
from pathlib import Path


def quoted(text):
    return ast.literal_eval(text)


def read_ids(path):
    ids = set()
    field = None
    value = ""
    for line in path.read_text().splitlines():
        if not line:
            if field == "msgid":
                ids.add(value)
            field = None
            value = ""
        elif line.startswith("msgid "):
            field = "msgid"
            value = quoted(line[6:])
        elif line.startswith("msgid_plural "):
            field = "plural"
        elif line.startswith('"') and field:
            if field == "msgid":
                value += quoted(line)
    if field == "msgid":
        ids.add(value)
    return ids


def call_arguments(source, start):
    closers = [")"]
    quoted = False
    escaped = False
    begin = start
    arguments = []
    for index in range(start, len(source)):
        character = source[index]
        if quoted:
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                quoted = False
            continue
        if character == '"':
            quoted = True
        elif character in "([{":
            closers.append({"(": ")", "[": "]", "{": "}"}[character])
        elif character in ")]}":
            if closers[-1] != character:
                continue
            closers.pop()
            if not closers:
                arguments.append(source[begin:index].strip())
                return arguments
        elif character == "," and len(closers) == 1:
            arguments.append(source[begin:index].strip())
            begin = index + 1
    return []


def main():
    pot_path = Path(sys.argv[1])
    ids = read_ids(pot_path)
    additions = []
    call = re.compile(r"(?<![A-Za-z0-9_])tr!\s*\(")
    sources = []
    for crate in ("hwsight-core", "hwsight-cli", "hwsight-gui"):
        sources += sorted(Path(crate, "src").rglob("*.rs"))
    for source in sources:
        # Stop at the test module only: `#[cfg(test)]` can also sit on a single item
        # earlier in a file, and cutting there would drop every string after it.
        text = re.split(r"^#\[cfg\(test\)\]\s*\nmod ", source.read_text(), maxsplit=1, flags=re.M)[0]
        # comments (doc comments too) are not code: blank them out, keeping line count
        text = re.sub(r"^\s*//.*$", "", text, flags=re.M)
        for match in call.finditer(text):
            arguments = call_arguments(text, match.end())
            if not arguments:
                continue
            expression = arguments[0]
            literal = re.match(r'^("(?:\\.|[^"\\])*")', expression)
            if literal:
                singular = quoted(literal.group(1))
                plural_match = re.match(
                    r'^\s*\|\s*("(?:\\.|[^"\\])*")\s*%',
                    expression[literal.end() :],
                )
                plural = quoted(plural_match.group(1)) if plural_match else None
                messages = [(singular, plural)]
            else:
                messages = [
                    (quoted(value), None)
                    for value in re.findall(r'"(?:\\.|[^"\\])*"', expression)
                ]
            for singular, plural in messages:
                if singular and singular not in ids:
                    additions.append((singular, plural))
                    ids.add(singular)
    if additions:
        with pot_path.open("a") as pot:
            for singular, plural in additions:
                pot.write(f"\nmsgid {json.dumps(singular, ensure_ascii=False)}\n")
                if plural:
                    pot.write(
                        f"msgid_plural {json.dumps(plural, ensure_ascii=False)}\n"
                        'msgstr[0] ""\nmsgstr[1] ""\n'
                    )
                else:
                    pot.write('msgstr ""\n')


if __name__ == "__main__":
    main()
