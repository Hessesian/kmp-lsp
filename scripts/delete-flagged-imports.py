#!/usr/bin/env python3
"""Delete every import line flagged by `kmp-lsp unused-imports` from a real
checkout, so the result can be compiled to validate detector precision.

This is the strongest available precision check for a "safe to delete"
diagnostic: don't sample flags, delete all of them and try to build. See
docs/superpowers/specs/2026-07-28-unused-import-diagnostic-design.md for the
methodology and the nowInAndroid/Moneta results this produced.

Usage:
    kmp-lsp unused-imports --root /path/to/project > flags.txt
    scripts/delete-flagged-imports.py /path/to/project flags.txt

Only touches working-tree files -- run on a clean checkout (verify `git
status` first) so the deletions are trivially reversible with `git checkout
--` afterwards. Never commits anything itself.
"""
import re
import sys
from collections import defaultdict
from pathlib import Path

PATTERN = re.compile(r"^(\S+):(\d+) \[unused-import\]: (\S+)$")


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <project-root> <flags-file>", file=sys.stderr)
        return 2
    root = Path(sys.argv[1])
    flags_file = Path(sys.argv[2])

    by_file = defaultdict(list)
    with open(flags_file) as f:
        for line in f:
            m = PATTERN.match(line.rstrip("\n"))
            if not m:
                continue
            rel_path, line_no, fqn = m.groups()
            by_file[rel_path].append((int(line_no), fqn))

    total_flags = sum(len(v) for v in by_file.values())
    print(f"{len(by_file)} files, {total_flags} flagged imports", file=sys.stderr)

    deleted = 0
    skipped = []
    for rel_path, entries in by_file.items():
        full_path = root / rel_path
        text = full_path.read_text()
        lines = text.split("\n")
        # Delete bottom-to-top so earlier deletions don't shift later line numbers.
        for line_no, fqn in sorted(entries, reverse=True):
            idx = line_no - 1
            if idx >= len(lines):
                skipped.append((rel_path, line_no, fqn, "line out of range"))
                continue
            actual = lines[idx].strip()
            if not actual.startswith("import ") or fqn not in actual:
                skipped.append((rel_path, line_no, fqn, f"mismatch: {actual!r}"))
                continue
            del lines[idx]
            deleted += 1
        full_path.write_text("\n".join(lines))

    print(f"deleted {deleted} import lines", file=sys.stderr)
    if skipped:
        print(f"SKIPPED {len(skipped)} (left untouched):", file=sys.stderr)
        for rel_path, line_no, fqn, reason in skipped:
            print(f"  {rel_path}:{line_no} {fqn} -- {reason}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
