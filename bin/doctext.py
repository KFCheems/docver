#!/usr/bin/env python3
"""docx/odt/doc -> plain text on stdout. The git textconv entry point.

git calls this with a path; all the work is in lib/doctext.py, which the diff
viewer also uses so that `git diff` and `docver diff` see identical text.
"""
import os
import sys

# realpath: this script is normally reached through a symlink in ~/.local/bin.
sys.path.insert(0, os.path.join(
    os.path.dirname(os.path.dirname(os.path.realpath(__file__))), "lib"))
import doctext  # noqa: E402

if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: doctext FILE")
    try:
        sys.stdout.write(doctext.convert(sys.argv[1]))
    except Exception as exc:  # boundary: a textconv that crashes confuses git
        sys.exit(f"doctext: {exc}")
