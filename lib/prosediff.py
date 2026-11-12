#!/usr/bin/env python3
"""Word-level diff engine for prose.

Three layers, because prose is not code:

  1. **anchor** — difflib finds paragraphs that are byte-identical. These are
     reliable anchors and cost nothing.
  2. **align** — inside each run of non-identical paragraphs, pair old and new
     paragraphs by *similarity*, not by position. Positional pairing breaks the
     moment one paragraph is inserted: every later pair is then off by one and
     the reader sees unrelated text compared against each other. The alignment
     is a Needleman–Wunsch DP maximising `similarity - THRESHOLD` per pair, so
     a pair is only formed when it is genuinely better than leaving both
     paragraphs unpaired (gaps are free).
  3. **tokenise** — inside a paired paragraph, diff words. CJK is one token per
     character (Chinese has no spaces); Latin is one token per word.

Both renderers (terminal, HTML) consume `align()`, so they can never disagree
about what changed.
"""
import difflib
import re

# One token = one CJK char, or one run of non-CJK non-space, or one space run.
_CJK = r"\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\u3040-\u30ff\uac00-\ud7af"
TOKEN_RE = re.compile(rf"[{_CJK}]|[^\s{_CJK}]+|\s+")

# Below this paragraph similarity, two paragraphs are treated as unrelated
# (shown as delete + insert) rather than word-diffed. 0.35 keeps rewritten
# sentences word-diffed while letting "totally different text" read cleanly.
PAIR_THRESHOLD = 0.35

RESET = "\x1b[0m"
BOLD = "\x1b[1m"
DIM = "\x1b[2m"
RED = "\x1b[31m"
GREEN = "\x1b[32m"
CYAN = "\x1b[36m"
STRIKE = "\x1b[9m"


class Style:
    """ANSI colouring that degrades to git-style markers when colour is off."""

    def __init__(self, enabled: bool):
        self.enabled = enabled

    def __call__(self, code: str, text: str) -> str:
        if not self.enabled or not text:
            return text
        return f"{code}{text}{RESET}"


def tokenize(line: str):
    return TOKEN_RE.findall(line)


def similarity(a: str, b: str) -> float:
    if not a or not b:
        return 0.0
    return difflib.SequenceMatcher(None, a, b, autojunk=False).ratio()


def word_diff(old: str, new: str):
    """Return [(kind, text)] with kind in {'=', '-', '+'} for one line pair."""
    a, b = tokenize(old), tokenize(new)
    sm = difflib.SequenceMatcher(None, a, b, autojunk=False)
    parts = []
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            parts.append(("=", "".join(a[i1:i2])))
        else:
            if i2 > i1:
                parts.append(("-", "".join(a[i1:i2])))
            if j2 > j1:
                parts.append(("+", "".join(b[j1:j2])))
    return parts


def _align_block(olds, news, threshold=PAIR_THRESHOLD):
    """Needleman–Wunsch over one run of non-identical paragraphs.

    Returns [(old or None, new or None)]. A pair contributes
    `similarity - threshold` to the score, a gap contributes 0 — so pairing
    only wins when it is genuinely more informative than two separate blocks.
    """
    n, m = len(olds), len(news)
    sim = [[similarity(olds[i], news[j]) for j in range(m)] for i in range(n)]
    # dp[i][j] = best score aligning olds[:i] with news[:j]
    dp = [[0.0] * (m + 1) for _ in range(n + 1)]
    back = [[None] * (m + 1) for _ in range(n + 1)]
    for i in range(1, n + 1):
        dp[i][0], back[i][0] = 0.0, "up"
    for j in range(1, m + 1):
        dp[0][j], back[0][j] = 0.0, "left"
    for i in range(1, n + 1):
        for j in range(1, m + 1):
            gain = sim[i - 1][j - 1] - threshold
            diag = dp[i - 1][j - 1] + gain if gain > 0 else float("-inf")
            up, left = dp[i - 1][j], dp[i][j - 1]
            best = max(diag, up, left)
            dp[i][j] = best
            # Tie-break towards pairing: a word-diffed pair shows more than a
            # delete followed by an insert.
            back[i][j] = "diag" if diag == best else ("up" if up >= left else "left")

    pairs = []
    i, j = n, m
    while i > 0 or j > 0:
        step = back[i][j] if (i > 0 and j > 0) else ("up" if i > 0 else "left")
        if step == "diag":
            pairs.append((olds[i - 1], news[j - 1]))
            i, j = i - 1, j - 1
        elif step == "up":
            pairs.append((olds[i - 1], None))
            i -= 1
        else:
            pairs.append((None, news[j - 1]))
            j -= 1
    pairs.reverse()
    return pairs


def align(old_text: str, new_text: str):
    """Align two documents into ops: (kind, old_para, new_para).

    kind is 'equal', 'changed' (word-diffable pair), 'insert' or 'delete'.
    Both renderers consume this list; it is the single source of truth about
    what changed.
    """
    a = old_text.split("\n")
    b = new_text.split("\n")
    sm = difflib.SequenceMatcher(None, a, b, autojunk=False)
    ops = []
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            for k in range(i2 - i1):
                ops.append(("equal", a[i1 + k], b[j1 + k]))
            continue
        olds = a[i1:i2]
        news = b[j1:j2]
        block = []
        for o, n in _align_block(olds, news):
            if o is not None and n is not None:
                block.append(("changed", o, n))
            elif o is not None:
                block.append(("delete", o, None))
            else:
                block.append(("insert", None, n))
        ops.extend(_deletes_first(block))
    return ops


def _deletes_first(block):
    """Within a run of adjacent delete/insert ops, show deletions first.

    git does the same, and for a replaced passage "- old / + new" reads in
    document order; interleaved it reads as if the new text came first.
    'changed' pairs keep their position — they are already paired.
    """
    out, i = [], 0
    while i < len(block):
        if block[i][0] == "changed":
            out.append(block[i])
            i += 1
            continue
        j = i
        while j < len(block) and block[j][0] in ("delete", "insert"):
            j += 1
        run = block[i:j]
        out.extend([op for op in run if op[0] == "delete"])
        out.extend([op for op in run if op[0] == "insert"])
        i = j
    return out


def summarize(ops):
    """Counts used by both renderers, so the numbers always agree."""
    stats = {"modified": 0, "added": 0, "removed": 0}
    for kind, _, _ in ops:
        if kind == "changed":
            stats["modified"] += 1
        elif kind == "insert":
            stats["added"] += 1
        elif kind == "delete":
            stats["removed"] += 1
    return stats


def summary_text(stats):
    bits = []
    if stats["modified"]:
        bits.append(f"{stats['modified']} paragraph(s) edited")
    if stats["added"]:
        bits.append(f"{stats['added']} added")
    if stats["removed"]:
        bits.append(f"{stats['removed']} removed")
    return ", ".join(bits) or "changed"


def has_changes(ops):
    return any(kind != "equal" for kind, _, _ in ops)


def collapse(ops, context: int):
    """Yield ops with long equal runs collapsed to `context` on each side.

    Yields ('gap', n, None) for elided runs. context=0 means keep everything.
    """
    if context <= 0:
        for op in ops:
            yield op
        return
    i = 0
    while i < len(ops):
        if ops[i][0] != "equal":
            yield ops[i]
            i += 1
            continue
        j = i
        while j < len(ops) and ops[j][0] == "equal":
            j += 1
        run = ops[i:j]
        at_start, at_end = i == 0, j == len(ops)
        # Unchanged head/tail of the document carries no information, so it is
        # trimmed harder than an interior run.
        head = 0 if at_start else context
        tail = 0 if at_end else context
        if len(run) <= head + tail:
            for op in run:
                yield op
            i = j
            continue
        else:
            for op in run[:head]:
                yield op
            # No marker for trimmed head/tail: that is context, not elision.
            if not at_start and not at_end:
                yield ("gap", len(run) - head - tail, None)
            for op in run[len(run) - tail:]:
                yield op
        i = j


# ------------------------------------------------------------------ rendering

def render_pair(old: str, new: str, st: Style, indent: str = "  ") -> str:
    """One changed paragraph as a single line with inline markers."""
    out, buf_kind, buf = [], None, []
    for kind, text in word_diff(old, new):
        if kind != buf_kind and buf:
            out.append((buf_kind, "".join(buf)))
            buf = []
        buf_kind = kind
        buf.append(text)
    if buf:
        out.append((buf_kind, "".join(buf)))

    line = []
    for kind, text in out:
        if kind == "-":
            line.append(st(RED + STRIKE, text) if st.enabled else "[-" + text + "-]")
        elif kind == "+":
            line.append(st(GREEN, text) if st.enabled else "{+" + text + "+}")
        else:
            line.append(text)
    return indent + "".join(line).replace("\n", " ")


def render(ops, st: Style, context: int = 2):
    """Terminal rendering. Returns (lines, stats)."""
    lines = []
    for kind, old, new in collapse(ops, context):
        if kind == "equal":
            lines.append(st(DIM, f"  {old}"))
        elif kind == "gap":
            lines.append(st(DIM, f"  ⋮  ({old} unchanged paragraph(s))"))
        elif kind == "delete":
            lines.append(st(RED + STRIKE, f"- {old}"))
        elif kind == "insert":
            lines.append(st(GREEN, f"+ {new}"))
        else:
            lines.append(render_pair(old, new, st))
    return lines, summarize(ops)
