//! Word-level diff for prose.
//!
//! Three layers, because prose is not code:
//!
//! 1. **anchor** — an LCS over paragraphs finds the ones that are byte-identical.
//!    They are reliable anchors and cost nothing to trust.
//! 2. **align** — inside each run of non-identical paragraphs, pair old against
//!    new by *similarity*, never by position. Positional pairing breaks the
//!    moment one paragraph is inserted: every later pair is then off by one and
//!    the reader is shown unrelated text compared against unrelated text. The
//!    alignment is a Needleman–Wunsch DP that scores a pair `similarity -
//!    THRESHOLD` and a gap 0, so a pair only forms when it is genuinely more
//!    informative than leaving both paragraphs unpaired.
//! 3. **tokenise** — inside a paired paragraph, diff words: one token per CJK
//!    character (Chinese has no spaces), one per Latin word.
//!
//! Similarity is Dice over adjacent-token bigrams rather than an LCS over
//! characters. `align` calls it once per (old, new) pair, so a character-level
//! LCS turns the whole alignment into O(n²m²) — on a document that shares no
//! anchors at all, that is the difference between instant and a stall. Bigram
//! Dice is O(n log n) and moves in the same direction.

/// Below this similarity two paragraphs are treated as unrelated — shown as a
/// deletion followed by an insertion — instead of word-diffed into a sentence
/// that exists in neither version.
pub const PAIR_THRESHOLD: f32 = 0.35;

/// A token is one CJK character, or one run of non-CJK non-space, or one run
/// of spaces. Ranges cover CJK ideographs, compatibility ideographs, kana and
/// hangul — the scripts that do not put spaces between words.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{f900}'..='\u{faff}'
        | '\u{3040}'..='\u{30ff}'
        | '\u{ac00}'..='\u{d7af}')
}

pub fn tokenize(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut class = 0u8;
    for (i, c) in s.char_indices() {
        if is_cjk(c) {
            if let Some(st) = start.take() {
                out.push(&s[st..i]);
            }
            out.push(&s[i..i + c.len_utf8()]);
            class = 0;
            continue;
        }
        let m = if c.is_whitespace() { 2 } else { 1 };
        match start {
            None => {
                start = Some(i);
                class = m;
            }
            Some(st) if m != class => {
                out.push(&s[st..i]);
                start = Some(i);
                class = m;
            }
            Some(_) => {}
        }
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

fn bigrams(tokens: &[&str]) -> Vec<u64> {
    let mut v: Vec<u64> = tokens
        .windows(2)
        .map(|w| {
            let mut h = fnv(w[0].as_bytes());
            h ^= fnv(w[1].as_bytes()).rotate_left(17);
            h
        })
        .collect();
    v.sort_unstable();
    v
}

/// Dice coefficient over adjacent-token bigrams, in [0, 1].
pub fn similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let (ta, tb) = (tokenize(a), tokenize(b));
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    if ta.len() < 2 || tb.len() < 2 {
        // Too short for bigrams. Fall back to shared-token ratio rather than
        // declaring everything unlike: "Hello" against "Hello world" is one
        // insertion, not a rewrite, and a one-token paragraph should still
        // pair so the reader sees the insertion in place.
        let common = ta.iter().filter(|t| tb.contains(t)).count();
        return (2.0 * common as f32) / (ta.len() + tb.len()) as f32;
    }
    let (ga, gb) = (bigrams(&ta), bigrams(&tb));
    let (mut i, mut j, mut hit) = (0usize, 0usize, 0usize);
    while i < ga.len() && j < gb.len() {
        match ga[i].cmp(&gb[j]) {
            std::cmp::Ordering::Equal => {
                hit += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    (2.0 * hit as f32) / (ga.len() + gb.len()) as f32
}

/// Longest common subsequence of two token lists, as matched index pairs.
/// Returns `None` when the inputs are too large for a quadratic table.
fn lcs_tokens(a: &[&str], b: &[&str]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return Some(Vec::new());
    }
    if n.saturating_mul(m) > 4_000_000 {
        return None;
    }
    let mut dp = vec![0u16; (n + 1) * (m + 1)];
    let w = m + 1;
    for i in 1..=n {
        for j in 1..=m {
            dp[i * w + j] = if a[i - 1] == b[j - 1] {
                dp[(i - 1) * w + (j - 1)] + 1
            } else {
                dp[(i - 1) * w + j].max(dp[i * w + (j - 1)])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[(i - 1) * w + j] >= dp[i * w + (j - 1)] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    Some(pairs)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Same,
    Del,
    Ins,
}

/// Inline word diff of one paragraph pair.
#[derive(Clone, Debug, Default)]
pub struct WordDiff {
    /// Old side: runs of `Same` and `Del`.
    pub old: Vec<(Mark, String)>,
    /// New side: runs of `Same` and `Ins`.
    pub new: Vec<(Mark, String)>,
    /// Both sides interleaved, deletions first — what a unified view reads.
    pub inline: Vec<(Mark, String)>,
}

fn push_run(v: &mut Vec<(Mark, String)>, mark: Mark, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = v.last_mut() {
        if last.0 == mark {
            last.1.push_str(text);
            return;
        }
    }
    v.push((mark, text.to_string()));
}

pub fn word_diff(old: &str, new: &str) -> WordDiff {
    let (a, b) = (tokenize(old), tokenize(new));
    let mut d = WordDiff::default();
    let pairs = lcs_tokens(&a, &b);
    let mut ai = 0usize;
    let mut bi = 0usize;
    match pairs {
        None => {
            // Degenerate paragraph: report it whole rather than pretend to a
            // word-level answer we did not compute.
            push_run(&mut d.old, Mark::Del, old);
            push_run(&mut d.new, Mark::Ins, new);
            push_run(&mut d.inline, Mark::Del, old);
            push_run(&mut d.inline, Mark::Ins, new);
        }
        Some(pairs) => {
            for (pi, pj) in pairs {
                if pi > ai {
                    push_run(&mut d.old, Mark::Del, &a[ai..pi].concat());
                    push_run(&mut d.inline, Mark::Del, &a[ai..pi].concat());
                }
                if pj > bi {
                    push_run(&mut d.new, Mark::Ins, &b[bi..pj].concat());
                    push_run(&mut d.inline, Mark::Ins, &b[bi..pj].concat());
                }
                push_run(&mut d.old, Mark::Same, a[pi]);
                push_run(&mut d.new, Mark::Same, b[pj]);
                push_run(&mut d.inline, Mark::Same, a[pi]);
                ai = pi + 1;
                bi = pj + 1;
            }
            if ai < a.len() {
                push_run(&mut d.old, Mark::Del, &a[ai..].concat());
                push_run(&mut d.inline, Mark::Del, &a[ai..].concat());
            }
            if bi < b.len() {
                push_run(&mut d.new, Mark::Ins, &b[bi..].concat());
                push_run(&mut d.inline, Mark::Ins, &b[bi..].concat());
            }
        }
    }
    d
}

/// One aligned unit of the document.
#[derive(Clone, Debug)]
pub enum Op {
    Equal(String),
    Changed(String, String),
    Insert(String),
    Delete(String),
    /// `n` unchanged paragraphs left out of the view.
    Gap(usize),
}

impl Op {
    pub fn is_change(&self) -> bool {
        !matches!(self, Op::Equal(_) | Op::Gap(_))
    }
}

fn align_block(olds: &[&str], news: &[&str], out: &mut Vec<Op>) {
    let (n, m) = (olds.len(), news.len());
    // dp[i][j] = best score aligning olds[..i] with news[..j].
    let w = m + 1;
    let mut dp = vec![0f32; (n + 1) * w];
    let mut back = vec![0u8; (n + 1) * w]; // 1 = diag, 2 = up, 3 = left
    for i in 1..=n {
        back[i * w] = 2;
    }
    for j in 1..=m {
        back[j] = 3;
    }
    for i in 1..=n {
        for j in 1..=m {
            let gain = similarity(olds[i - 1], news[j - 1]) - PAIR_THRESHOLD;
            let diag = if gain > 0.0 { dp[(i - 1) * w + (j - 1)] + gain } else { f32::NEG_INFINITY };
            let up = dp[(i - 1) * w + j];
            let left = dp[i * w + (j - 1)];
            let (best, dir) = if diag >= up && diag >= left {
                (diag, 1)
            } else if up >= left {
                (up, 2)
            } else {
                (left, 3)
            };
            dp[i * w + j] = best;
            back[i * w + j] = dir;
        }
    }

    let mut block: Vec<Op> = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let dir = back[i * w + j];
        match dir {
            1 if i > 0 && j > 0 => {
                block.push(Op::Changed(olds[i - 1].into(), news[j - 1].into()));
                i -= 1;
                j -= 1;
            }
            2 if i > 0 => {
                block.push(Op::Delete(olds[i - 1].into()));
                i -= 1;
            }
            _ if j > 0 => {
                block.push(Op::Insert(news[j - 1].into()));
                j -= 1;
            }
            _ => break,
        }
    }
    block.reverse();
    out.extend(deletes_first(block));
}

/// Within a run of adjacent insert/delete ops, show deletions first.
///
/// git does the same, and for a replaced passage "- old / + new" reads in
/// document order; interleaved, it reads as if the new text came first.
/// `Changed` pairs keep their position — they are already paired.
fn deletes_first(block: Vec<Op>) -> Vec<Op> {
    let mut out = Vec::with_capacity(block.len());
    let mut i = 0;
    while i < block.len() {
        if matches!(block[i], Op::Changed(..)) {
            out.push(block[i].clone());
            i += 1;
            continue;
        }
        let mut j = i;
        while j < block.len() && !matches!(block[j], Op::Changed(..)) {
            j += 1;
        }
        out.extend(block[i..j].iter().filter(|o| matches!(o, Op::Delete(_))).cloned());
        out.extend(block[i..j].iter().filter(|o| matches!(o, Op::Insert(_))).cloned());
        i = j;
    }
    out
}

/// Paragraph-level LCS. `None` when the table would be unreasonable.
fn lcs_lines(a: &[&str], b: &[&str]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > 4_000_000 {
        return None;
    }
    if n == 0 || m == 0 {
        return Some(Vec::new());
    }
    let w = m + 1;
    let mut dp = vec![0u32; (n + 1) * w];
    for i in 1..=n {
        for j in 1..=m {
            dp[i * w + j] = if a[i - 1] == b[j - 1] {
                dp[(i - 1) * w + (j - 1)] + 1
            } else {
                dp[(i - 1) * w + j].max(dp[i * w + (j - 1)])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[(i - 1) * w + j] >= dp[i * w + (j - 1)] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    Some(pairs)
}

pub fn align(old: &str, new: &str) -> Vec<Op> {
    let a: Vec<&str> = old.split('\n').filter(|l| !l.is_empty()).collect();
    let b: Vec<&str> = new.split('\n').filter(|l| !l.is_empty()).collect();
    let mut ops = Vec::new();

    let anchors = match lcs_lines(&a, &b) {
        Some(p) => p,
        None => {
            // Huge document: treat it as one run and let similarity do the work.
            align_block(&a, &b, &mut ops);
            return ops;
        }
    };

    let (mut ai, mut bi) = (0usize, 0usize);
    for (pa, pb) in anchors {
        if pa > ai || pb > bi {
            align_block(&a[ai..pa], &b[bi..pb], &mut ops);
        }
        ops.push(Op::Equal(a[pa].to_string()));
        ai = pa + 1;
        bi = pb + 1;
    }
    if ai < a.len() || bi < b.len() {
        align_block(&a[ai..], &b[bi..], &mut ops);
    }
    ops
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub modified: usize,
    pub added: usize,
    pub removed: usize,
}

impl Stats {
    pub fn total(&self) -> usize {
        self.modified + self.added + self.removed
    }
}

pub fn summarize(ops: &[Op]) -> Stats {
    let mut s = Stats::default();
    for op in ops {
        match op {
            Op::Changed(..) => s.modified += 1,
            Op::Insert(_) => s.added += 1,
            Op::Delete(_) => s.removed += 1,
            _ => {}
        }
    }
    s
}

pub fn has_changes(ops: &[Op]) -> bool {
    ops.iter().any(|o| o.is_change())
}

/// Collapse long unchanged runs to `context` paragraphs on each side.
///
/// The head and tail of a document carry no information, so they are trimmed
/// away entirely rather than kept as context — that is the difference between
/// a redline that opens on the first change and one that opens on the title.
pub fn collapse(ops: &[Op], context: usize) -> Vec<Op> {
    if context == 0 {
        return ops.to_vec();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        if !matches!(ops[i], Op::Equal(_)) {
            out.push(ops[i].clone());
            i += 1;
            continue;
        }
        let mut j = i;
        while j < ops.len() && matches!(ops[j], Op::Equal(_)) {
            j += 1;
        }
        let run = &ops[i..j];
        let (at_start, at_end) = (i == 0, j == ops.len());
        let head = if at_start { 0 } else { context };
        let tail = if at_end { 0 } else { context };
        if run.len() <= head + tail {
            out.extend(run.iter().cloned());
        } else {
            out.extend(run[..head].iter().cloned());
            // No marker for a trimmed head/tail: that is the document edge,
            // not an elision the reader should try to open.
            if !at_start && !at_end {
                out.push(Op::Gap(run.len() - head - tail));
            }
            out.extend(run[run.len() - tail..].iter().cloned());
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(ops: &[Op]) -> Vec<&'static str> {
        ops.iter()
            .map(|o| match o {
                Op::Equal(_) => "=",
                Op::Changed(..) => "~",
                Op::Insert(_) => "+",
                Op::Delete(_) => "-",
                Op::Gap(_) => "…",
            })
            .collect()
    }

    #[test]
    fn identical_is_all_equal() {
        let ops = align("a\nb\nc", "a\nb\nc");
        assert_eq!(kinds(&ops), vec!["=", "=", "="]);
        assert!(!has_changes(&ops));
    }

    #[test]
    fn insertion_does_not_shift_later_pairs() {
        let old = "第一条\n本合同期限为一年。\n第三条\n价格一百元。";
        let new = "第一条\n本合同期限为一年。\n第二条 保密\n乙方应保密。\n第三条\n价格一百元。";
        let ops = align(old, new);
        assert_eq!(kinds(&ops), vec!["=", "=", "+", "+", "=", "="]);
    }

    #[test]
    fn similar_paragraphs_pair_and_word_diff() {
        let ops = align("价格为一百元。", "价格为二百元。");
        assert_eq!(kinds(&ops), vec!["~"]);
        match &ops[0] {
            Op::Changed(o, n) => {
                assert!(o.contains('一') && n.contains('二'));
                let d = word_diff(o, n);
                assert!(d.old.iter().any(|(m, _)| *m == Mark::Del));
                assert!(d.new.iter().any(|(m, _)| *m == Mark::Ins));
            }
            _ => panic!("expected a paired change"),
        }
    }

    #[test]
    fn unrelated_paragraphs_are_not_paired() {
        let ops = align("完全无关的一句话。", "Another entirely different line.");
        assert_eq!(kinds(&ops), vec!["-", "+"]);
    }

    #[test]
    fn deletes_come_before_inserts() {
        let ops = align("甲\n乙", "丙\n丁");
        assert_eq!(kinds(&ops), vec!["-", "-", "+", "+"]);
    }

    #[test]
    fn collapse_trims_document_edges_entirely() {
        let ops = align(
            "标题\n第一段。\n第二段。\n第三段。\n价格为一百元。",
            "标题\n第一段。\n第二段。\n第三段。\n价格为二百元。",
        );
        // context=1 keeps one neighbour, and the head of the document is
        // trimmed away entirely: the redline opens on the first change.
        assert_eq!(kinds(&collapse(&ops, 1)), vec!["=", "~"]);
    }

    #[test]
    fn collapse_marks_interior_gaps() {
        // Changes at both ends, ten identical paragraphs in between: the run is
        // interior, so it must leave a marker rather than vanish.
        let mut old = String::from("价格为一百元。\n");
        let mut new = String::from("价格为二百元。\n");
        for i in 0..10 {
            old.push_str(&format!("中间段落{i}。\n"));
            new.push_str(&format!("中间段落{i}。\n"));
        }
        old.push_str("保密期限为三年。");
        new.push_str("保密期限为五年。");
        let ops = align(&old, &new);
        let c = collapse(&ops, 1);
        assert_eq!(kinds(&c), vec!["~", "=", "…", "=", "~"]);
        assert!(matches!(c[2], Op::Gap(8)));
    }

    #[test]
    fn one_word_paragraph_pairs_with_its_longer_form() {
        let ops = align("Hello", "Hello world");
        assert_eq!(kinds(&ops), vec!["~"]);
        let d = word_diff("Hello", "Hello world");
        assert_eq!(d.new.last().unwrap().0, Mark::Ins);
    }

    #[test]
    fn cjk_tokenises_per_character() {
        assert_eq!(tokenize("价格为一百元"), vec!["价", "格", "为", "一", "百", "元"]);
        assert_eq!(tokenize("hello world"), vec!["hello", " ", "world"]);
        assert_eq!(tokenize("共 100 元"), vec!["共", " ", "100", " ", "元"]);
    }

    #[test]
    fn stats_count_each_kind() {
        let ops = align("价格为一百元。\n保密。", "价格为二百元。\n保密。\n附则。");
        let s = summarize(&ops);
        assert_eq!((s.modified, s.added, s.removed), (1, 1, 0));
        assert_eq!(s.total(), 2);
    }
}
