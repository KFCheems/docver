#!/usr/bin/env python3
"""Office document -> plain text.

This is the *conversion* layer only: it knows how to get honest text out of a
binary document and nothing about diffing. Both consumers share it — the git
textconv driver (path in, text out) and the diff viewer (bytes in, text out,
so it can diff a `git show` blob without ever writing a temp file).
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile

# --track-changes=all keeps Word's own revision marks visible instead of
# silently accepting them; --wrap=none makes one paragraph per line, which is
# the property the paragraph aligner depends on.
PANDOC = ["pandoc", "-t", "plain", "--wrap=none", "--track-changes=all"]
PANDOC_FROM = {
    ".docx": "docx", ".docm": "docx", ".dotx": "docx",
    ".odt": "odt", ".rtf": "rtf", ".epub": "epub", ".html": "html", ".htm": "html",
}
RAW = {".md": "utf-8", ".markdown": "utf-8", ".txt": "utf-8", ".org": "utf-8"}
# Formats only LibreOffice can open; pandoc has no reader for them.
SOFFICE_ONLY = {".doc", ".dot", ".wps", ".odp", ".ppt", ".pptx", ".xls", ".xlsx", ".pages"}


_RULE_TOKEN = re.compile(r"^[-=_+|]{3,}$")


def _is_rule(line: str) -> bool:
    """pandoc draws tables as runs of dashes, possibly several cells wide."""
    parts = line.split()
    if not parts:
        return False
    return all(_RULE_TOKEN.match(p) for p in parts) and sum(len(p) for p in parts) >= 6


def normalize(text: str) -> str:
    """One paragraph per line, no filler.

    pandoc emits a blank line between blocks and draws tables with runs of
    dashes. Both are pure noise for line-based alignment: the blank lines make
    a one-paragraph edit look like two, and the rule lines get aligned as if
    they were prose. Drop them; keep the paragraph text itself untouched.
    """
    out = []
    for raw in text.split("\n"):
        line = raw.rstrip()
        if not line.strip():
            continue
        if _is_rule(line):
            continue
        out.append(line)
    return "\n".join(out)


def _pandoc(args, stdin_bytes=None):
    out = subprocess.run(PANDOC + args, input=stdin_bytes, capture_output=True)
    if out.returncode != 0:
        sys.stderr.write(out.stderr.decode("utf-8", "replace"))
        raise RuntimeError("pandoc failed")
    return out.stdout.decode("utf-8", "replace")


def _via_soffice(data: bytes, ext: str) -> str:
    soffice = shutil.which("soffice") or shutil.which("libreoffice")
    if not soffice:
        raise RuntimeError(f"cannot read {ext}: pandoc has no reader and LibreOffice is missing")
    with tempfile.TemporaryDirectory() as td:
        src = os.path.join(td, "in" + ext)
        with open(src, "wb") as fh:
            fh.write(data)
        subprocess.run(
            [soffice, "--headless", "--convert-to", "docx", "--outdir", td, src],
            capture_output=True, check=False,
        )
        converted = os.path.join(td, "in.docx")
        if not os.path.exists(converted):
            raise RuntimeError(f"LibreOffice could not convert {ext}")
        with open(converted, "rb") as fh:
            return _pandoc(["-f", "docx"], fh.read())


def convert_bytes(data: bytes, ext: str) -> str:
    ext = ext.lower()
    if ext in RAW:
        return data.decode(RAW[ext], "replace")
    if ext in PANDOC_FROM:
        return normalize(_pandoc(["-f", PANDOC_FROM[ext]], data))
    if ext in SOFFICE_ONLY:
        return normalize(_via_soffice(data, ext))
    return data.decode("utf-8", "replace")


def convert(path: str) -> str:
    ext = os.path.splitext(path)[1].lower()
    if ext in RAW:
        with open(path, "rb") as fh:
            return fh.read().decode(RAW[ext], "replace")
    if ext in PANDOC_FROM:
        return normalize(_pandoc(["-f", PANDOC_FROM[ext], path]))
    if ext in SOFFICE_ONLY:
        with open(path, "rb") as fh:
            return normalize(_via_soffice(fh.read(), ext))
    with open(path, "rb") as fh:
        return fh.read().decode("utf-8", "replace")


if __name__ == "__main__":
    sys.stdout.write(convert(sys.argv[1]))
