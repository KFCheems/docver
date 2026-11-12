#!/usr/bin/env python3
"""docver — local version control for Office documents.

Design notes, because this decides the UX:

* The store *is* a git repository. No parallel history format, no index files to
  drift out of sync. `git log`, `git fsck`, `git bundle` all keep working on it.
* Snapshots are whole files. Word files cannot be merged, so storing deltas
  would buy nothing and cost correctness.
* The textconv driver is registered in the repo's own config (not the global
  one), so `git diff` on docx works inside this repo and nowhere else.
* `restore` never overwrites a file in place. It writes next to it with a
  suffix. Rollback must not be the one operation that can destroy work.
"""
import argparse
import datetime as dt
import os
import re
import subprocess
import sys

# realpath, not abspath: docver is normally used through a symlink in
# ~/.local/bin, and the libraries live next to the real script.
ROOT = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
LIB = os.path.join(ROOT, "lib")
DOCTEXT = os.path.join(ROOT, "bin", "doctext.py")
sys.path.insert(0, LIB)
import prosediff  # noqa: E402
import doctext  # noqa: E402
from html import escape as html_escape  # noqa: E402

DOC_EXTS = (".docx", ".doc", ".odt", ".rtf", ".docm", ".dotx", ".wps")
ALL_EXTS = DOC_EXTS + (".md", ".markdown", ".txt", ".org", ".epub", ".html", ".htm")


# ---------------------------------------------------------------- git plumbing

def git(repo, *args, check=True, capture=True, text=True):
    cmd = ["git", "-C", repo, *args]
    res = subprocess.run(cmd, capture_output=capture, text=text)
    if check and res.returncode != 0:
        msg = (res.stderr or "").strip() or (res.stdout or "").strip()
        raise SystemExit(f"git {' '.join(args)}: {msg}")
    return res


def find_repo(start="."):
    res = subprocess.run(
        ["git", "-C", start, "rev-parse", "--show-toplevel"],
        capture_output=True, text=True,
    )
    if res.returncode != 0:
        raise SystemExit(
            "docver: not inside a document store.\n"
            "        run `docver init` here (or in a folder of documents) first."
        )
    return res.stdout.strip()


def ensure_identity(repo):
    """A commit with no author is a hard error; pick a sane local default."""
    if git(repo, "config", "user.email", check=False).stdout.strip():
        return
    git(repo, "config", "user.email", "docver@localhost")
    git(repo, "config", "user.name", os.environ.get("USER") or "docver")


def configure(repo):
    """Register the textconv drivers for every extension we can render."""
    # Without this, git escapes non-ASCII paths and every later lookup misses.
    git(repo, "config", "core.quotepath", "false")
    git(repo, "config", "diff.docx.textconv", f"python3 {DOCTEXT}")
    git(repo, "config", "diff.docx.binary", "true")
    git(repo, "config", "diff.odt.textconv", f"python3 {DOCTEXT}")
    git(repo, "config", "diff.odt.binary", "true")
    git(repo, "config", "diff.rtf.textconv", f"python3 {DOCTEXT}")
    git(repo, "config", "diff.rtf.binary", "true")
    attrs = os.path.join(repo, ".gitattributes")
    want = "".join(f"*.{e.lstrip('.')} diff={e.lstrip('.')}\n"
                   for e in ("docx", "odt", "rtf"))
    existing = ""
    if os.path.exists(attrs):
        with open(attrs, encoding="utf-8") as fh:
            existing = fh.read()
    if existing != want:
        with open(attrs, "w", encoding="utf-8") as fh:
            fh.write(want)
    # LibreOffice lock files and Word's ~$ backups must never be committed.
    ignore = os.path.join(repo, ".gitignore")
    if not os.path.exists(ignore):
        with open(ignore, "w", encoding="utf-8") as fh:
            fh.write("~$*\n.~lock.*#\n*.tmp\n")


def rel(repo, path):
    """Path relative to the repo root, with forward slashes (git's spelling)."""
    ap = os.path.abspath(path)
    rp = os.path.relpath(ap, repo)
    if rp.startswith(".."):
        raise SystemExit(f"docver: {path} is outside the store at {repo}")
    return rp.replace(os.sep, "/")


def tracked(repo):
    out = git(repo, "ls-files", "-z").stdout.split("\0")
    return [p for p in out if p and p.lower().endswith(ALL_EXTS)]


def resolve(repo, name):
    """Find a document by exact relative path, basename, or unique suffix."""
    name = name.replace(os.sep, "/")
    files = tracked(repo)
    if name in files:
        return name
    hits = [f for f in files if f == name or f.endswith("/" + name) or os.path.basename(f) == name]
    if len(hits) == 1:
        return hits[0]
    if not hits:
        raise SystemExit(f"docver: no document named {name!r} in the store "
                         f"({len(files)} tracked). `docver ls` lists them.")
    raise SystemExit("docver: ambiguous name, candidates:\n  " + "\n  ".join(sorted(hits)))


# ------------------------------------------------------------------- commands

def cmd_init(args):
    repo = os.path.abspath(args.path)
    os.makedirs(repo, exist_ok=True)
    if not os.path.isdir(os.path.join(repo, ".git")):
        git(repo, "init", "-q", "-b", "main", check=False)
        if not os.path.isdir(os.path.join(repo, ".git")):
            git(repo, "init", "-q")
    ensure_identity(repo)
    configure(repo)
    print(f"store ready: {repo}")
    print("  drop documents here, then:  docver save <file> -m 'first draft'")
    return 0


def cmd_save(args):
    repo = find_repo()
    ensure_identity(repo)
    configure(repo)
    targets = []
    for pat in args.files or ["."]:
        if pat == ".":
            for root, dirs, files in os.walk(repo):
                dirs[:] = [d for d in dirs if d != ".git"]
                for f in files:
                    if f.lower().endswith(ALL_EXTS):
                        targets.append(rel(repo, os.path.join(root, f)))
        else:
            targets.append(rel(repo, pat))

    new = [t for t in targets if os.path.exists(os.path.join(repo, t))]
    gone = [t for t in targets if not os.path.exists(os.path.join(repo, t))]
    if not new and not gone:
        print("nothing to save")
        return 0

    if new:
        git(repo, "add", "--", *new)
    # The store's own config belongs in the store, otherwise `git status` in a
    # freshly initialised store always shows two untracked files.
    for cfg in (".gitattributes", ".gitignore"):
        if os.path.exists(os.path.join(repo, cfg)):
            git(repo, "add", "--", cfg, check=False)
    if gone and args.prune:
        git(repo, "rm", "--cached", "-q", "--", *gone)

    staged = git(repo, "diff", "--cached", "--name-only", "-z").stdout
    if not staged.strip("\0"):
        print("nothing changed since last snapshot")
        return 0

    when = dt.datetime.now().strftime("%Y-%m-%d %H:%M")
    msg = args.message or f"snapshot {when}"
    git(repo, "commit", "-q", "-m", msg)
    rev = git(repo, "rev-parse", "--short", "HEAD").stdout.strip()
    files = [f for f in staged.split("\0") if f]
    print(f"[{rev}] {msg}")
    for f in files:
        print(f"  · {f}")
    return 0


def cmd_log(args):
    repo = find_repo()
    path = resolve(repo, args.file) if args.file else None
    fmt = "%h\x1f%ad\x1f%s"
    cmd = ["log", f"--format={fmt}", "--date=format:%Y-%m-%d %H:%M"]
    if path:
        cmd += ["--follow", "--", path]
    out = git(repo, *cmd).stdout.strip("\0").strip()
    if not out:
        print(f"no history for {path}" if path else "no snapshots yet")
        return 0
    rows = [ln.split("\x1f") for ln in out.split("\n") if ln]
    w = max(len(r[0]) for r in rows)
    print(f"history{' for ' + path if path else ''}  ({len(rows)} snapshot(s))")
    for rev, when, subj in rows:
        print(f"  {rev:<{w}}  {when}  {subj}")
    return 0


def _blob(repo, rev, path):
    res = subprocess.run(["git", "-C", repo, "show", f"{rev}:{path}"], capture_output=True)
    if res.returncode != 0:
        raise SystemExit(f"docver: {path} does not exist at {rev}")
    return res.stdout


def cmd_diff(args):
    repo = find_repo()
    path = resolve(repo, args.file)
    cur = os.path.join(repo, path)
    to_rev = args.to or ("WORKTREE" if os.path.exists(cur) else "HEAD")

    if args.from_rev:
        old_rev = args.from_rev
    else:
        # Default comparison is the *previous* snapshot, not HEAD: that is the
        # question people actually ask ("what changed since last save?").
        out = git(repo, "log", "--format=%h", "-2", "--", path).stdout.split()
        if not out:
            raise SystemExit(f"docver: {path} has no committed history to compare against")
        old_rev = out[1] if len(out) > 1 else out[0]

    old_bytes = _blob(repo, old_rev, path)
    if to_rev == "WORKTREE":
        if not os.path.exists(cur):
            raise SystemExit(f"docver: {path} is missing from the working tree; "
                             f"use --to <rev> to compare two snapshots")
        with open(cur, "rb") as fh:
            new_bytes = fh.read()
        new_label = "working copy"
    else:
        new_bytes = _blob(repo, to_rev, path)
        new_label = to_rev

    ext = os.path.splitext(path)[1]
    old_text = doctext.convert_bytes(old_bytes, ext)
    new_text = doctext.convert_bytes(new_bytes, ext)

    ops = prosediff.align(old_text, new_text)
    changed = prosediff.has_changes(ops)
    stats = prosediff.summarize(ops)

    color = not args.no_color and sys.stdout.isatty() and os.environ.get("NO_COLOR") is None
    st = prosediff.Style(color)
    if not args.html_only:
        if not args.stat:
            lines, _ = prosediff.render(ops, st, args.context)
            for ln in lines:
                print(ln)
            print()
        if changed:
            print(st(prosediff.BOLD, f"{old_rev} → {new_label}: {prosediff.summary_text(stats)}"))
        else:
            print(st(prosediff.DIM, f"{old_rev} → {new_label}: identical"))

    if args.html:
        import htmlview
        target = args.html if args.html != "-" else "diff.html"
        target = target if os.path.isabs(target) else os.path.join(repo, target)
        meta = (f"{html_escape(old_rev)} → {html_escape(new_label)} · "
                f"{html_escape(path)} · "
                f"{html_escape(prosediff.summary_text(stats)) if changed else 'identical'}")
        htmlview.write(target, ops, path, meta, context=max(args.context, 3))
        print(f"wrote {target}")
        if args.open:
            opener = "xdg-open" if sys.platform.startswith("linux") else "open"
            subprocess.run([opener, target], check=False)

    return 1 if changed else 0


def rev_label(repo, rev):
    """A revision as a filename-safe label, preferring the short sha.

    `restore 合同.docx HEAD~1` should write `合同.5dd700c.docx`, not
    `合同.HEAD-1.docx` — the sha is what you would quote to anyone else.
    """
    res = git(repo, "rev-parse", "--short", rev, check=False)
    sha = res.stdout.strip() if res.returncode == 0 else ""
    if sha and re.fullmatch(r"[0-9a-f]{4,40}", sha):
        return sha
    return re.sub(r"[^0-9A-Za-z._-]+", "-", rev).strip("-") or "rev"


def cmd_show(args):
    repo = find_repo()
    path = resolve(repo, args.file)
    data = _blob(repo, args.rev, path)
    out = args.out
    if not out:
        stem, ext = os.path.splitext(os.path.basename(path))
        out = os.path.join(os.path.dirname(os.path.abspath(path)) or ".",
                           f"{stem}.{rev_label(repo, args.rev)}{ext}")
    if os.path.exists(out) and not args.force:
        raise SystemExit(f"docver: {out} exists (use --force to overwrite)")
    with open(out, "wb") as fh:
        fh.write(data)
    print(f"wrote {out}  ({len(data)} bytes, from {args.rev})")
    return 0


def cmd_restore(args):
    args.out = args.out
    args.force = True
    return cmd_show(args)


def cmd_ls(args):
    repo = find_repo()
    files = tracked(repo)
    if not files:
        print("store is empty")
        return 0
    for f in sorted(files):
        n = git(repo, "rev-list", "--count", "HEAD", "--", f, check=False).stdout.strip() or "0"
        when = git(repo, "log", "-1", "--format=%ad", "--date=format:%Y-%m-%d %H:%M",
                   "--", f, check=False).stdout.strip()
        status = ""
        if os.path.exists(os.path.join(repo, f)):
            dirty = git(repo, "status", "--porcelain", "--", f).stdout.strip()
            status = "  (unsaved changes)" if dirty else ""
        print(f"  {f}  [{n} snapshot(s), last {when or '—'}]{status}")
    return 0


def cmd_git(args):
    """Escape hatch: run git in the store with textconv already configured."""
    repo = find_repo()
    configure(repo)
    return subprocess.run(["git", "-C", repo, *args.extra]).returncode


def build_parser():
    ap = argparse.ArgumentParser(
        prog="docver",
        description="Local, self-controlled version history for Word/Office documents.",
        epilog="Typical flow: docver init ~/docs && cd ~/docs && "
               "docver save contract.docx -m 'signed copy' && docver diff contract.docx",
    )
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("init", help="create a store in this folder")
    p.add_argument("path", nargs="?", default=".")
    p.set_defaults(func=cmd_init)

    p = sub.add_parser("save", help="snapshot documents (commit)")
    p.add_argument("files", nargs="*", help="files to snapshot (default: all)")
    p.add_argument("-m", "--message", help="snapshot message")
    p.add_argument("--prune", action="store_true", help="also record deletions")
    p.set_defaults(func=cmd_save)

    p = sub.add_parser("log", help="list snapshots")
    p.add_argument("file", nargs="?")
    p.set_defaults(func=cmd_log)

    p = sub.add_parser("diff", help="compare two versions of a document")
    p.add_argument("file")
    p.add_argument("--from", dest="from_rev", help="old revision (default: previous snapshot)")
    p.add_argument("--to", help="new revision (default: working copy)")
    p.add_argument("-U", "--context", type=int, default=2)
    p.add_argument("--no-color", action="store_true")
    p.add_argument("--html", nargs="?", const="diff.html", metavar="PATH",
                   help="also write a side-by-side HTML page (default: diff.html)")
    p.add_argument("--html-only", action="store_true", help="write the page, skip terminal output")
    p.add_argument("--stat", action="store_true", help="only print the change summary")
    p.add_argument("--open", action="store_true", help="open the HTML page afterwards")
    p.set_defaults(func=cmd_diff)

    p = sub.add_parser("show", help="extract a past version to a file")
    p.add_argument("file")
    p.add_argument("rev", nargs="?", default="HEAD")
    p.add_argument("-o", "--out")
    p.add_argument("--force", action="store_true")
    p.set_defaults(func=cmd_show)

    p = sub.add_parser("restore", help="write a past version next to the current file")
    p.add_argument("file")
    p.add_argument("rev", nargs="?", default="HEAD")
    p.add_argument("-o", "--out")
    p.set_defaults(func=cmd_restore)

    p = sub.add_parser("ls", help="list documents in the store")
    p.set_defaults(func=cmd_ls)

    p = sub.add_parser("git", help="run raw git in the store")
    p.add_argument("extra", nargs=argparse.REMAINDER)
    p.set_defaults(func=cmd_git)
    return ap


def main(argv=None):
    args = build_parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
