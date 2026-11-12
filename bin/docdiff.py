#!/usr/bin/env python3
"""docdiff — word-level diff of Office documents, for the terminal.

The engine lives in lib/prosediff.py so `docver diff` renders identically; this
file is only argument handling and I/O.
"""
import argparse
import os
import subprocess
import sys

# realpath: this script is used through a symlink in ~/.local/bin.
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(
    os.path.realpath(__file__))), "lib"))
import prosediff  # noqa: E402
import doctext  # noqa: E402

BOLD, DIM = prosediff.BOLD, prosediff.DIM


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="docdiff",
        description="Word-level diff of Office documents / text files.",
    )
    ap.add_argument("old")
    ap.add_argument("new")
    ap.add_argument("--no-color", action="store_true", help="plain output, no ANSI")
    ap.add_argument("-U", "--context", type=int, default=2,
                    help="unchanged paragraphs shown around each change (0 = all, default 2)")
    ap.add_argument("--stat", action="store_true", help="only print the change summary")
    ap.add_argument("--html", nargs="?", const="diff.html", metavar="PATH",
                    help="also write a side-by-side HTML page")
    ap.add_argument("--open", action="store_true", help="open the HTML page afterwards")
    args = ap.parse_args(argv)

    color = not args.no_color and sys.stdout.isatty() and os.environ.get("NO_COLOR") is None
    st = prosediff.Style(color)

    try:
        old_text = doctext.convert(args.old)
        new_text = doctext.convert(args.new)
    except Exception as exc:  # boundary: report and fail, don't traceback
        sys.stderr.write(f"docdiff: {exc}\n")
        return 2

    ops = prosediff.align(old_text, new_text)
    changed = prosediff.has_changes(ops)

    if not args.stat:
        lines, stats = prosediff.render(ops, st, args.context)
        for ln in lines:
            print(ln)
    else:
        stats = prosediff.summarize(ops)
        lines = []

    name_o, name_n = os.path.basename(args.old), os.path.basename(args.new)
    if changed:
        print()
        print(st(BOLD, f"{name_o} → {name_n}: {prosediff.summary_text(stats)}"))
    else:
        print(st(DIM, f"{name_o} == {name_n}: identical"))

    if args.html:
        import htmlview
        target = os.path.abspath(args.html)
        htmlview.write(target, ops, os.path.basename(args.old),
                       f"{name_o} → {name_n} · {prosediff.summary_text(stats) if changed else 'identical'}",
                       context=max(args.context, 3))
        print(f"wrote {target}")
        if args.open:
            opener = "xdg-open" if sys.platform.startswith("linux") else "open"
            subprocess.run([opener, target], check=False)
    return 1 if changed else 0


if __name__ == "__main__":
    raise SystemExit(main())
