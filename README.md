# docver

Local version history and word-level diff for Word/Office documents. Nothing
leaves the machine, and the history is a plain git repository, so `git log`,
`git bundle` and `git fsck` keep working on it.

Two front ends over one engine:

- **`docver`** — a command-line tool (Python). Snapshots, diffs, restores,
  exit codes that script.
- **`docver-gui`** — a native desktop window (Rust + egui). The same history,
  read as a redline on paper.

| | |
| --- | --- |
| `bin/`, `lib/`, `tests/` | the CLI |
| `gui/` | the desktop app (own `README.md`, own `install.sh`) |

## Why not Word's "track changes"

Track changes is for one document travelling between people. It does not help
when you have `合同_v3_最终_真的最终.docx` scattered across folders. docver
answers the other question — *what did this document look like yesterday, and
what exactly changed?* — and it works on any file Word or LibreOffice can open.

## CLI setup

```bash
ln -s "$PWD/bin/docver.py" ~/.local/bin/docver
ln -s "$PWD/bin/docdiff.py" ~/.local/bin/docdiff
mkdir ~/docs && cd ~/docs && docver init .
```

`docver init` registers a git textconv driver **in this repository only**, so
`git diff` inside the store renders Word documents as prose instead of
"Binary files differ".

## Daily use

```bash
cp ~/Downloads/合同.docx .                    # edit in Word, save over it
docver save 合同.docx -m "按甲方意见改价格"    # snapshot
docver diff 合同.docx                          # previous snapshot → working copy
docver diff 合同.docx --html --open            # same, as a side-by-side page
docver log 合同.docx                           # every snapshot, newest first
docver restore 合同.docx 3f2a1b9               # write 合同.3f2a1b9.docx beside it
```

`restore` never overwrites the file you are working on — it writes
`合同.3f2a1b9.docx` next to it. Rollback should not be the one command that can
lose work.

## Commands

| command | meaning |
| --- | --- |
| `docver init [dir]` | make `dir` a store (idempotent) |
| `docver save [files…] [-m msg]` | snapshot; no args = every document in the store |
| `docver diff FILE [--from R] [--to R]` | word-level diff; default previous snapshot → working copy |
| `docver diff FILE --html [--open]` | also write a side-by-side HTML page |
| `docver diff FILE --stat` | just the summary line |
| `docver log [FILE]` | snapshot history |
| `docver show FILE REV [-o out]` | extract a past version (refuses to clobber without `--force`) |
| `docver restore FILE REV` | write a past version beside the file |
| `docver ls` | documents and their snapshot counts |
| `docver git …` | raw git in the store |

Revisions are anything git understands: `HEAD~2`, a short sha, a tag. The file
written by `restore`/`show` is always named after the resolved short sha, so
`restore 合同.docx HEAD~1` gives you `合同.5dd700c.docx` — the name you would
quote to someone else. `docver diff FILE --from v1 --to v2` compares two
snapshots directly. Exit code follows `diff(1)`: 0 identical, 1 differs,
2 error — so it scripts.

## GUI

```bash
cd gui && ./install.sh && docver-gui ~/docs/合同.docx
```

See [`gui/README.md`](gui/README.md) for the layout, the font pitfalls and the
build flags.

## Formats

Word (`.docx/.docm/.dotx/.doc/.dot`), LibreOffice/OpenDocument (`.odt`), `.rtf`,
WPS, plus `.md/.txt/.org/.epub/.html`. `.docx/.odt/.rtf` go through pandoc;
older binary formats are converted by LibreOffice first, then pandoc. The GUI
reads `.docx/.odt/.rtf` itself (no external process) and only falls back to
LibreOffice for the legacy formats.

## The diff engine

Prose is not code, so the engine (`lib/prosediff.py`, mirrored in
`gui/src/engine/prosediff.rs`) works in three layers:

1. **anchors** — identical paragraphs found by `difflib`.
2. **alignment** — inside a run of non-identical paragraphs, old and new are
   paired by similarity with a Needleman–Wunsch DP, scoring each pair
   `similarity − 0.35` and leaving gaps free. Pairing by position instead
   (the obvious approach) breaks the moment one paragraph is inserted: every
   later pair is then off by one and the page compares unrelated clauses.
3. **tokens** — inside a pair, one token per CJK character, one per Latin word.

Both renderers consume the same alignment, so the terminal, the HTML page and
the GUI can never disagree about what changed.

## Standalone diff

`docdiff a.docx b.docx` diffs any two files without a store — useful for a
one-off "did the other side change anything?" check.

## Tests

```bash
python3 -m unittest discover -s tests     # 46 tests, CLI + engine
cd gui && cargo test --release            # 18 tests, Rust engine
```

## Requirements

CLI: Python 3.9+, `pandoc`, and LibreOffice only for the legacy binary formats.
GUI: a Rust toolchain; the binary itself needs no Python and no pandoc.
