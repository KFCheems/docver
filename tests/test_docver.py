#!/usr/bin/env python3
"""Behavioural tests for docver / docdiff / doctext.

Each test states the *property* being protected, not just the mechanics — the
ones that matter are the invariants: restore must never overwrite, diff must be
word-level on CJK, and the store must stay a plain git repo.
"""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
BIN = os.path.join(ROOT, "bin")
sys.path.insert(0, BIN)
sys.path.insert(0, os.path.join(ROOT, "lib"))

import prosediff  # noqa: E402
import doctext  # noqa: E402
import docver  # noqa: E402


def make_docx(path, paragraphs, table=None):
    import docx
    d = docx.Document()
    for p in paragraphs:
        d.add_paragraph(p)
    if table:
        t = d.add_table(rows=len(table), cols=len(table[0]))
        for i, row in enumerate(table):
            for j, cell in enumerate(row):
                t.cell(i, j).text = cell
    d.save(path)
    return path


def read_bytes(path):
    with open(path, "rb") as fh:
        return fh.read()


def write_bytes(path, data):
    with open(path, "wb") as fh:
        fh.write(data)


def run(*args, cwd=None):
    res = subprocess.run([sys.executable, *args], capture_output=True, text=True, cwd=cwd)
    return res.returncode, res.stdout + res.stderr


class TestTokens(unittest.TestCase):
    def test_cjk_is_per_character(self):
        """A one-character Chinese edit must be a one-token edit."""
        self.assertEqual(prosediff.tokenize("甲方应付款"), ["甲", "方", "应", "付", "款"])

    def test_latin_is_per_word_and_spaces_preserved(self):
        self.assertEqual(prosediff.tokenize("pay  in full"), ["pay", "  ", "in", " ", "full"])

    def test_mixed_script_splits_at_the_boundary(self):
        toks = prosediff.tokenize("期限30日")
        self.assertEqual(toks, ["期", "限", "30", "日"])


class TestWordDiff(unittest.TestCase):
    def test_single_char_substitution_is_minimal(self):
        parts = prosediff.word_diff("甲方应三十日内付款", "甲方应六十日内付款")
        minus = "".join(t for k, t in parts if k == "-")
        plus = "".join(t for k, t in parts if k == "+")
        self.assertEqual((minus, plus), ("三", "六"))

    def test_insertion_only(self):
        parts = prosediff.word_diff("损坏。", "损坏、受潮。")
        self.assertEqual([k for k, _ in parts], ["=", "+", "="])

    def test_identical_lines_produce_no_markers(self):
        parts = prosediff.word_diff("同一段落", "同一段落")
        self.assertEqual({k for k, _ in parts}, {"="})


class TestAlignment(unittest.TestCase):
    """Alignment is the property that makes or breaks readability."""

    def test_inserted_paragraph_does_not_shift_later_pairs(self):
        """The bug that positional pairing causes: one insertion misaligns all."""
        old = "\n".join(["甲条款", "乙条款", "丙条款", "丁条款"])
        new = "\n".join(["甲条款", "新增的条款", "乙条款", "丙条款", "丁条款"])
        ops = prosediff.align(old, new)
        kinds = [k for k, _, _ in ops]
        self.assertEqual(kinds, ["equal", "insert", "equal", "equal", "equal"])

    def test_deleted_paragraph_does_not_shift_later_pairs(self):
        old = "\n".join(["甲条款", "乙条款", "多余条款", "丙条款"])
        new = "\n".join(["甲条款", "乙条款", "丙条款"])
        kinds = [k for k, _, _ in prosediff.align(old, new)]
        self.assertEqual(kinds, ["equal", "equal", "delete", "equal"])

    def test_unrelated_paragraphs_are_not_paired(self):
        old = "\n".join(["双方对商业秘密负有保密义务。"])
        new = "\n".join(["任何一方违反约定应支付违约金。"])
        kinds = [k for k, _, _ in prosediff.align(old, new)]
        self.assertEqual(kinds, ["delete", "insert"],
                         "dissimilar paragraphs must not be word-diffed together")

    def test_deletions_render_before_insertions(self):
        old = "\n".join(["完全不同的旧段落内容"])
        new = "\n".join(["Zzz qqq unrelated sentence here"])
        kinds = [k for k, _, _ in prosediff.align(old, new)]
        self.assertEqual(kinds, ["delete", "insert"], "document order, like git")

    def test_rewritten_sentence_is_paired_and_word_diffed(self):
        old = "\n".join(["甲方应在收到货物后三十日内支付全部款项。"])
        new = "\n".join(["甲方应在收到货物后六十日内支付全部款项。"])
        ops = prosediff.align(old, new)
        self.assertEqual([k for k, _, _ in ops], ["changed"])
        self.assertEqual(prosediff.summarize(ops)["modified"], 1)

    def test_no_change_yields_only_equal_ops(self):
        text = "一\n二\n三"
        self.assertFalse(prosediff.has_changes(prosediff.align(text, text)))

    def test_collapse_hides_document_head_and_tail(self):
        ops = [("equal", f"p{i}", f"p{i}") for i in range(40)]
        ops[20] = ("insert", None, "新增")
        shown = list(prosediff.collapse(ops, 2))
        self.assertLess(len(shown), 12)
        self.assertNotIn("p0", [o[1] for o in shown],
                         "leading unchanged paragraphs are pure noise")

    def test_collapse_with_zero_context_keeps_everything(self):
        ops = [("equal", "a", "a"), ("insert", None, "b"), ("equal", "c", "c")]
        self.assertEqual(len(list(prosediff.collapse(ops, 0))), 3)


class TestDocumentDiff(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)

    def _render(self, a_paras, b_paras, context=0):
        a = make_docx(os.path.join(self.tmp, "a.docx"), a_paras)
        b = make_docx(os.path.join(self.tmp, "b.docx"), b_paras)
        ops = prosediff.align(doctext.convert(a), doctext.convert(b))
        lines, stats = prosediff.render(ops, prosediff.Style(False), context)
        return "\n".join(lines), stats

    def test_changed_paragraph_is_edited_not_replaced(self):
        text, stats = self._render(["甲方应在收到货物后三十日内支付全部款项。"],
                                   ["甲方应在收到货物后六十日内支付全部款项。"])
        self.assertEqual(stats["modified"], 1)
        self.assertEqual(stats["added"], 0)
        self.assertIn("[-三-]{+六+}", text)

    def test_unrelated_paragraph_reads_as_delete_plus_insert(self):
        text, stats = self._render(["完全不同的第一段内容"], ["Zzz qqq unrelated sentence here"])
        self.assertEqual(stats["removed"], 1)
        self.assertEqual(stats["added"], 1)
        self.assertTrue(text.startswith("- 完全"), text)

    def test_identical_documents_report_no_change(self):
        a = make_docx(os.path.join(self.tmp, "a.docx"), ["条款一", "条款二"])
        ops = prosediff.align(doctext.convert(a), doctext.convert(a))
        self.assertFalse(prosediff.has_changes(ops))

    def test_table_cells_are_diffed_as_text_not_rules(self):
        a = make_docx(os.path.join(self.tmp, "a.docx"), ["表"], table=[["金额", "100000"]])
        b = make_docx(os.path.join(self.tmp, "b.docx"), ["表"], table=[["金额", "120000"]])
        old = doctext.convert(a)
        self.assertNotIn("---", old, "table rule lines must be stripped by normalize()")
        ops = prosediff.align(old, doctext.convert(b))
        lines, _ = prosediff.render(ops, prosediff.Style(False), 0)
        self.assertIn("[-100000-]{+120000+}", "\n".join(lines))

    def test_context_collapses_long_unchanged_runs(self):
        paras = [f"第{i}段" for i in range(60)]
        changed = list(paras)
        changed[30] = "改动过的段落"
        text, _ = self._render(paras, changed, context=2)
        self.assertLess(len(text.split("\n")), 20)

    def test_colour_and_plain_output_carry_the_same_information(self):
        a = make_docx(os.path.join(self.tmp, "a.docx"), ["原文本"])
        b = make_docx(os.path.join(self.tmp, "b.docx"), ["新文本"])
        ops = prosediff.align(doctext.convert(a), doctext.convert(b))
        plain, _ = prosediff.render(ops, prosediff.Style(False), 0)
        colour, _ = prosediff.render(ops, prosediff.Style(True), 0)
        plain, colour = "\n".join(plain), "\n".join(colour)
        for ch in ("原", "新", "文本"):
            self.assertIn(ch, plain)
            self.assertIn(ch, colour)
        self.assertIn("[-原-]{+新+}", plain)

    def test_html_and_terminal_agree_on_the_same_ops(self):
        import htmlview
        a = make_docx(os.path.join(self.tmp, "a.docx"), ["甲方应在三十日内付款", "旧条款"])
        b = make_docx(os.path.join(self.tmp, "b.docx"), ["甲方应在六十日内付款", "新条款"])
        ops = prosediff.align(doctext.convert(a), doctext.convert(b))
        page = htmlview.render(ops, "t", "meta", context=0)
        self.assertIn("<del>三</del>", page)
        self.assertIn("<ins>六</ins>", page)
        self.assertEqual(page.count("<tr"), sum(1 for _ in prosediff.collapse(ops, 0)))

    def test_html_escapes_document_text(self):
        import htmlview
        ops = [("insert", None, "<script>alert(1)</script>")]
        page = htmlview.render(ops, "t", "m", context=0)
        self.assertNotIn("<script>alert(1)</script>", page)
        self.assertIn("&lt;script&gt;", page)


class TestCli(unittest.TestCase):
    """End-to-end: the commands people actually type."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.docx_v1 = make_docx(os.path.join(self.tmp, "v1.docx"), ["第一条 价格", "壹佰万元"])
        self.docx_v2 = make_docx(os.path.join(self.tmp, "v2.docx"), ["第一条 价格", "壹佰伍拾万元"])
        self.store = os.path.join(self.tmp, "store")
        os.makedirs(self.store)
        self.run_cli("init", self.store)

    def run_cli(self, *argv, cwd=None):
        import io, contextlib
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            try:
                code = docver.main(list(argv))
            except SystemExit as exc:
                code = exc.code or 0
        return code, buf.getvalue()

    def _seed(self, name="合同.docx"):
        dest = os.path.join(self.store, name)
        shutil.copy(self.docx_v1, dest)
        # Capture the cwd BEFORE chdir: the cleanup runs after the temp dir is
        # gone, so restoring to os.getcwd() taken afterwards would strand every
        # later test inside a deleted directory.
        here = os.getcwd()
        self.addCleanup(os.chdir, here)
        os.chdir(self.store)
        self.run_cli("save", name, "-m", "初稿", cwd=self.store)
        return name

    def test_init_registers_textconv_inside_the_store_only(self):
        attrs = os.path.join(self.store, ".gitattributes")
        with open(attrs, encoding="utf-8") as fh:
            self.assertIn("*.docx diff=docx", fh.read())
        out = subprocess.run(["git", "-C", self.store, "config", "core.quotepath"],
                             capture_output=True, text=True).stdout.strip()
        self.assertEqual(out, "false")

    def test_save_then_diff_reports_the_edit(self):
        name = self._seed()
        shutil.copy(self.docx_v2, os.path.join(self.store, name))
        code, out = self.run_cli("diff", name, "--no-color", "--stat", cwd=self.store)
        self.assertEqual(code, 1, "diff must exit 1 when documents differ")
        self.assertIn("1 paragraph(s) edited", out)

    def test_diff_of_unchanged_document_exits_zero(self):
        name = self._seed()
        code, out = self.run_cli("diff", name, "--no-color", cwd=self.store)
        self.assertEqual(code, 0)
        self.assertIn("identical", out)

    def test_diff_on_untracked_name_fails_with_guidance(self):
        self._seed()
        with self.assertRaises(SystemExit) as ctx:
            docver.main(["diff", "nope.docx"])
        self.assertIn("no document named", str(ctx.exception))

    def test_restore_writes_beside_the_file_and_never_clobbers(self):
        name = self._seed()
        rev = subprocess.run(["git", "-C", self.store, "rev-parse", "--short", "HEAD"],
                             capture_output=True, text=True).stdout.strip()
        shutil.copy(self.docx_v2, os.path.join(self.store, name))
        self.run_cli("save", name, "-m", "修订", cwd=self.store)
        code, out = self.run_cli("restore", name, rev, cwd=self.store)
        self.assertEqual(code, 0)
        restored = os.path.join(self.store, f"合同.{rev}.docx")
        self.assertTrue(os.path.exists(restored), out)
        with open(restored, "rb") as fh:
            self.assertEqual(fh.read(), open(self.docx_v1, "rb").read())
        with open(os.path.join(self.store, name), "rb") as fh:
            self.assertEqual(fh.read(), open(self.docx_v2, "rb").read(),
                             "the working file must be untouched")

    def test_restore_names_the_file_after_the_sha_not_the_rev_expression(self):
        name = self._seed()
        shutil.copy(self.docx_v2, os.path.join(self.store, name))
        self.run_cli("save", name, "-m", "修订", cwd=self.store)
        code, out = self.run_cli("restore", name, "HEAD~1", cwd=self.store)
        self.assertEqual(code, 0)
        sha = subprocess.run(["git", "-C", self.store, "rev-parse", "--short", "HEAD~1"],
                             capture_output=True, text=True).stdout.strip()
        self.assertTrue(os.path.exists(os.path.join(self.store, f"合同.{sha}.docx")), out)

    def test_show_refuses_to_overwrite_without_force(self):
        name = self._seed()
        target = os.path.join(self.store, "out.docx")
        self.run_cli("show", name, "HEAD", "-o", target, cwd=self.store)
        with self.assertRaises(SystemExit) as ctx:
            docver.main(["show", name, "HEAD", "-o", target])
        self.assertIn("--force", str(ctx.exception))

    def test_html_flag_writes_a_page(self):
        name = self._seed()
        shutil.copy(self.docx_v2, os.path.join(self.store, name))
        code, out = self.run_cli("diff", name, "--html", "d.html", "--html-only",
                                 "--no-color", cwd=self.store)
        self.assertEqual(code, 1)
        page = os.path.join(self.store, "d.html")
        self.assertTrue(os.path.exists(page))
        with open(page, encoding="utf-8") as fh:
            page = fh.read()
        self.assertIn("<ins>伍拾</ins>", page)
        self.assertIn("壹佰万元", page)

    def test_git_diff_inside_the_store_renders_prose(self):
        name = self._seed()
        shutil.copy(self.docx_v2, os.path.join(self.store, name))
        out = subprocess.run(["git", "-C", self.store, "diff", "--", name],
                             capture_output=True, text=True).stdout
        self.assertIn("壹佰伍拾万元", out, out)
        self.assertNotIn("Binary files", out)


class TestStore(unittest.TestCase):
    """The store is a git repo; these are the operations users bet their work on."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.docver = os.path.join(BIN, "docver.py")
        run(self.docver, "init", ".", cwd=self.tmp)

    def dv(self, *args):
        return run(self.docver, *args, cwd=self.tmp)

    def test_save_creates_one_snapshot_per_call(self):
        make_docx(os.path.join(self.tmp, "c.docx"), ["初稿"])
        self.dv("save", "-m", "one")
        make_docx(os.path.join(self.tmp, "c.docx"), ["初稿改"])
        self.dv("save", "-m", "two")
        code, out = self.dv("log", "c.docx")
        self.assertEqual(code, 0)
        self.assertIn("2 snapshot(s)", out)
        self.assertIn("one", out)
        self.assertIn("two", out)

    def test_unchanged_save_is_a_noop(self):
        make_docx(os.path.join(self.tmp, "c.docx"), ["same"])
        self.dv("save", "-m", "one")
        code, out = self.dv("save", "-m", "again")
        self.assertEqual(code, 0)
        self.assertIn("nothing changed", out)

    def test_restore_never_overwrites_the_current_file(self):
        p = os.path.join(self.tmp, "c.docx")
        make_docx(p, ["初稿"])
        self.dv("save", "-m", "one")
        make_docx(p, ["第二版"])
        self.dv("save", "-m", "two")
        before = read_bytes(p)
        code, out = self.dv("restore", "c.docx", "HEAD~1")
        self.assertEqual(code, 0)
        self.assertEqual(read_bytes(p), before, "current file must be untouched")
        sha = subprocess.run(["git", "-C", self.tmp, "rev-parse", "--short", "HEAD~1"],
                             capture_output=True, text=True).stdout.strip()
        restored = [f for f in os.listdir(self.tmp) if f.startswith(f"c.{sha}")]
        self.assertEqual(len(restored), 1, os.listdir(self.tmp))
        self.assertNotEqual(read_bytes(os.path.join(self.tmp, restored[0])), before)

    def test_show_refuses_to_clobber_without_force(self):
        make_docx(os.path.join(self.tmp, "c.docx"), ["x"])
        self.dv("save", "-m", "one")
        target = os.path.join(self.tmp, "out.docx")
        write_bytes(target, b"occupied")
        code, out = self.dv("show", "c.docx", "HEAD", "-o", target)
        self.assertNotEqual(code, 0)
        self.assertEqual(read_bytes(target), b"occupied")

    def test_diff_against_working_copy_detects_edits(self):
        p = os.path.join(self.tmp, "c.docx")
        make_docx(p, ["甲方应在三十日内付款"])
        self.dv("save", "-m", "one")
        make_docx(p, ["甲方应在六十日内付款"])
        code, out = self.dv("diff", "c.docx", "--no-color")
        self.assertEqual(code, 1, "differing documents must exit non-zero like diff(1)")
        self.assertIn("[-三-]{+六+}", out)

    def test_diff_between_two_revisions(self):
        p = os.path.join(self.tmp, "c.docx")
        make_docx(p, ["v1"])
        self.dv("save", "-m", "one")
        make_docx(p, ["v2"])
        self.dv("save", "-m", "two")
        code, out = self.dv("diff", "c.docx", "--from", "HEAD~1", "--to", "HEAD", "--no-color")
        self.assertEqual(code, 1)
        self.assertIn("v1", out)
        self.assertIn("v2", out)

    def test_diff_of_identical_revision_exits_zero(self):
        p = os.path.join(self.tmp, "c.docx")
        make_docx(p, ["same"])
        self.dv("save", "-m", "one")
        code, out = self.dv("diff", "c.docx", "--from", "HEAD", "--to", "HEAD")
        self.assertEqual(code, 0)
        self.assertIn("identical", out)

    def test_document_resolves_by_basename_and_reports_ambiguity(self):
        os.makedirs(os.path.join(self.tmp, "a"))
        os.makedirs(os.path.join(self.tmp, "b"))
        make_docx(os.path.join(self.tmp, "a", "x.docx"), ["1"])
        make_docx(os.path.join(self.tmp, "b", "x.docx"), ["2"])
        self.dv("save", "-m", "both")
        code, out = self.dv("log", "x.docx")
        self.assertNotEqual(code, 0)
        self.assertIn("ambiguous", out)
        code, out = self.dv("log", "a/x.docx")
        self.assertEqual(code, 0)

    def test_store_stays_a_plain_git_repo(self):
        p = os.path.join(self.tmp, "c.docx")
        make_docx(p, ["x"])
        self.dv("save", "-m", "one")
        res = subprocess.run(["git", "-C", self.tmp, "cat-file", "-p", "HEAD:c.docx"],
                             capture_output=True)
        self.assertEqual(res.returncode, 0, "snapshot must be a real git blob")
        self.assertTrue(res.stdout.startswith(b"PK"), "docx bytes stored verbatim")

    def test_git_diff_on_docx_uses_the_textconv_driver(self):
        p = os.path.join(self.tmp, "c.docx")
        make_docx(p, ["甲方应在三十日内付款"])
        self.dv("save", "-m", "one")
        make_docx(p, ["甲方应在六十日内付款"])
        res = subprocess.run(["git", "-C", self.tmp, "diff", "--", "c.docx"],
                             capture_output=True, text=True)
        self.assertIn("甲方应在", res.stdout,
                      "textconv must make git diff show prose, not 'Binary files differ'")

    def test_lock_and_backup_files_are_ignored(self):
        make_docx(os.path.join(self.tmp, "c.docx"), ["x"])
        write_bytes(os.path.join(self.tmp, "~$c.docx"), b"lock")
        self.dv("save", "-m", "one")
        code, out = self.dv("ls")
        self.assertNotIn("~$c.docx", out)

    def test_non_ascii_filenames_round_trip(self):
        p = os.path.join(self.tmp, "合同草案.docx")
        make_docx(p, ["中文文件名"])
        self.dv("save", "-m", "one")
        code, out = self.dv("ls")
        self.assertEqual(code, 0)
        self.assertIn("合同草案.docx", out)
        code, out = self.dv("log", "合同草案.docx")
        self.assertIn("1 snapshot(s)", out)


class TestTextLayer(unittest.TestCase):
    def test_plain_text_and_markdown_need_no_conversion(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        p = os.path.join(tmp, "n.md")
        with open(p, "w", encoding="utf-8") as fh:
            fh.write("# 标题\n\n正文\n")
        self.assertIn("标题", doctext.convert(p))

    def test_normalize_strips_blank_lines_and_rules(self):
        raw = "一\n\n二\n----- -----\n三\n"
        self.assertEqual(doctext.normalize(raw), "一\n二\n三")

    def test_normalize_keeps_dash_text_that_is_not_a_rule(self):
        raw = "价格 - 100\n"
        self.assertEqual(doctext.normalize(raw), "价格 - 100")


if __name__ == "__main__":
    unittest.main(verbosity=2)
