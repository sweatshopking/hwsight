#!/usr/bin/env python3
"""Prints the data texts (descriptions, fixes, software) a language hasn't translated
yet, as lines ready to paste under [text] in hwsight-core/data/text-<language>.toml:

    "English text, exactly as in the data file" = ""

Fill in the right-hand side; never edit the left. Usage:
    python3 -I tools/data-text-missing.py de
"""
import json
import sys
import tomllib
from pathlib import Path

data = Path(__file__).resolve().parent.parent / "hwsight-core" / "data"
language = sys.argv[1]
descriptions = tomllib.loads((data / "descriptions.toml").read_text())
fixes = tomllib.loads((data / "fixes.toml").read_text())
english = [entry["text"] for entry in descriptions.get("describe", [])]
english += list(descriptions.get("category", {}).values())
english += [fix["text"] for fix in fixes.get("fix", {}).values()]
software = tomllib.loads((data / "software.toml").read_text())
for program in software.get("software", []):
    english += [program["what"]] + ([program["get"]] if "get" in program else [])
done = tomllib.loads((data / f"text-{language}.toml").read_text()).get("text", {})
seen = set()
for text in english:
    if text not in done and text not in seen:
        seen.add(text)
        # A JSON string is a valid TOML basic string.
        print(f"{json.dumps(text, ensure_ascii=False)} = \"\"")
