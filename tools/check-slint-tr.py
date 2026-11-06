#!/usr/bin/env python3
"""Lists Slint string literals a user can read that don't go through @tr(...).
Ignored: comments, imports, font names, image paths, and single lowercase words (internal values such
as "ok" or "neutral", and the app name "hwsight")."""
import re
import sys
from pathlib import Path

literal = re.compile(r'"(?:\\.|[^"\\])*"')
hits = []
for path in sorted(Path(sys.argv[1]).rglob("*.slint")):
    for number, line in enumerate(path.read_text().splitlines(), 1):
        code = re.sub(r'//.*$', '', re.sub(r'"(?:\\.|[^"\\])*"', lambda m: m.group(0).replace("//", "\0"), line))
        if re.search(r'\b(import|font-family)\b', code):
            continue
        for match in literal.finditer(code):
            text = match.group(0)[1:-1]
            if not re.search(r'[A-Za-z]', text) or re.fullmatch(r'[a-z][a-z0-9-]*', text):
                continue
            if code[: match.start()].rstrip().endswith(("@tr(", "@image-url(")):
                continue
            hits.append(f"{path}:{number}: {line.strip()}")
            break
print("\n".join(hits))
sys.exit(1 if hits else 0)
