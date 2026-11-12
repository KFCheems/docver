#!/usr/bin/env python3
"""Side-by-side HTML rendering of a prose diff.

Terminal output is good for a quick look; a redlined contract needs to be read
at leisure, scrolled, and shown to someone else. This produces one
self-contained page (no network, no assets) with the same alignment the
terminal uses: paragraphs paired by similarity, changed words marked inside
the pair.

Deliberately no tracked-changes .docx: faking Word revision XML would produce
a file Word may silently repair, and a broken contract document is worse than
a page of HTML.
"""
import html
import os

import prosediff

PAGE = """<!DOCTYPE html>
<html lang="zh"><head><meta charset="utf-8">
<title>{title}</title>
<style>
:root {{
  --bg:#fbfaf8; --fg:#1f1d1b; --dim:#8a847e; --line:#e3ded8; --rail:#cfc9c2;
  --add-bg:#e6f4ea; --add-fg:#17603a; --del-bg:#fceceb; --del-fg:#9c2620;
}}
@media (prefers-color-scheme: dark) {{
  :root {{
    --bg:#171614; --fg:#e8e3dc; --dim:#8a847e; --line:#302d29; --rail:#4a453f;
    --add-bg:#162e21; --add-fg:#83d9a3; --del-bg:#351d1b; --del-fg:#f2a49d;
  }}
}}
* {{ box-sizing:border-box }}
html {{ -webkit-text-size-adjust:100% }}
body {{
  margin:0; padding:2.4rem clamp(1rem,4vw,4.5rem) 7rem; background:var(--bg);
  color:var(--fg); font:16px/1.78 -apple-system,"Segoe UI","Noto Sans CJK SC",
  "Source Han Sans SC",sans-serif; font-feature-settings:"palt" 1;
}}
header {{ max-width:1200px; margin:0 auto 2rem; }}
h1 {{ font-size:1.02rem; font-weight:600; margin:0 0 .45rem; letter-spacing:.01em }}
.meta {{ color:var(--dim); font-size:.8rem; font-variant-numeric:tabular-nums }}
table {{ border-collapse:collapse; width:100%; max-width:1200px; margin:0 auto;
  table-layout:fixed }}
col.g {{ width:2.9rem }}
td {{ vertical-align:top; padding:.5rem .85rem .5rem .2rem; overflow-wrap:anywhere;
  border-left:1px solid var(--line) }}
td:first-child {{ border-left:0 }}
td.gutter {{ color:var(--rail); font-size:.7rem; text-align:right; padding:0 .55rem 0 0;
  font-variant-numeric:tabular-nums; user-select:none; line-height:2.1 }}
tr.equal td.text {{ color:var(--dim) }}
tr.changed td.left {{ background:var(--del-bg) }}
tr.changed td.right {{ background:var(--add-bg) }}
tr.changed td.gutter {{ color:var(--dim) }}
tr.gap td {{ text-align:center; color:var(--dim); font-size:.72rem; padding:.35rem;
  letter-spacing:.06em; border-left:0 }}
del {{ background:rgba(0,0,0,.045); color:var(--del-fg); text-decoration:line-through;
  text-decoration-thickness:1px; border-radius:2px; padding:0 1px }}
ins {{ background:rgba(255,255,255,.55); color:var(--add-fg); text-decoration:none;
  border-radius:2px; padding:0 1px }}
@media (prefers-color-scheme: dark) {{
  del {{ background:rgba(0,0,0,.25) }}
  ins {{ background:rgba(255,255,255,.07) }}
}}
.legend {{ display:flex; gap:1.3rem; margin:1.1rem auto 0; max-width:1200px;
  font-size:.76rem; color:var(--dim) }}
.legend i {{ width:.75rem; height:.75rem; border-radius:2px; display:inline-block;
  margin-right:.35rem; vertical-align:-1px }}
</style></head>
<body>
<header><h1>{title}</h1><div class="meta">{meta}</div></header>
<table><colgroup><col class="g"><col><col></colgroup>
{rows}
</table>
<div class="legend">
  <span><i style="background:var(--del-bg)"></i>removed</span>
  <span><i style="background:var(--add-bg)"></i>added</span>
  <span>dim rows are unchanged</span>
</div>
</body></html>
"""


def _mark_pair(old: str, new: str):
    """Return the two cells' inner HTML for a changed paragraph pair."""
    parts = prosediff.word_diff(old, new)
    left, right = [], []
    for kind, text in parts:
        esc = html.escape(text)
        if kind == "=":
            left.append(esc)
            right.append(esc)
        elif kind == "-":
            left.append(f"<del>{esc}</del>")
        else:
            right.append(f"<ins>{esc}</ins>")
    return "".join(left), "".join(right)


def _rows(ops, context):
    """Turn alignment ops into display rows.

    Side-by-side wants a delete and the insert that follows it on the SAME row:
    the reader's eye compares left against right. The terminal wants them one
    after another (document order), so this regrouping lives here, not in the
    engine.
    """
    seq = list(prosediff.collapse(ops, context))
    out, i = [], 0
    while i < len(seq):
        op = seq[i]
        if op[0] != "delete":
            out.append(op)
            i += 1
            continue
        dels = []
        while i < len(seq) and seq[i][0] == "delete":
            dels.append(seq[i][1])
            i += 1
        ins = []
        while i < len(seq) and seq[i][0] == "insert":
            ins.append(seq[i][2])
            i += 1
        for k in range(max(len(dels), len(ins))):
            out.append(("pair",
                        dels[k] if k < len(dels) else None,
                        ins[k] if k < len(ins) else None))
    return out


def render(ops, title: str, meta: str, context: int = 3) -> str:
    rows = []
    no = nn = 0
    for kind, old, new in _rows(ops, context):
        if kind == "equal":
            no += 1
            nn += 1
            esc = html.escape(old)
            rows.append(f'<tr class="equal"><td class="gutter">{no}</td>'
                        f'<td class="text">{esc}</td><td class="text">{esc}</td></tr>')
        elif kind == "gap":
            rows.append(f'<tr class="gap"><td colspan="3">⋯ {old} unchanged '
                        f'paragraph(s)</td></tr>')
            no += old
            nn += old
        elif kind == "changed":
            left, right = _mark_pair(old, new)
            no += 1
            nn += 1
            rows.append(f'<tr class="changed"><td class="gutter">{no}</td>'
                        f'<td class="left">{left}</td>'
                        f'<td class="right">{right}</td></tr>')
        else:  # pair: deleted paragraph on the left, inserted one on the right
            # The gutter numbers the *old* document, so a pure insertion has no
            # number of its own — printing the previous one twice reads as a
            # duplicated paragraph.
            gut = ""
            cells = []
            if old is not None:
                no += 1
                gut = str(no)
                cells.append(f'<td class="left"><del>{html.escape(old)}</del></td>')
            else:
                cells.append("<td></td>")
            if new is not None:
                nn += 1
                cells.append(f'<td class="right"><ins>{html.escape(new)}</ins></td>')
            else:
                cells.append("<td></td>")
            rows.append(f'<tr class="changed"><td class="gutter">{gut}</td>'
                        + "".join(cells) + "</tr>")
    return PAGE.format(title=html.escape(title), meta=meta, rows="\n".join(rows))


def write(path: str, ops, title: str, meta: str, context: int = 3) -> str:
    out = render(ops, title, meta, context=context)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(out)
    return os.path.abspath(path)
