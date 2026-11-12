//! The reading surface: a redline laid out like a printed comparison.
//!
//! Layout happens once per (document pair, width, font), not per frame. Every
//! paragraph is shaped into a galley up front, row heights are cached, and the
//! scroll area only *paints* the rows inside the viewport. A 300-paragraph
//! contract then costs the same per frame as a 3-paragraph one — the reason a
//! redline stays smooth while you scrub is that nothing is being laid out at
//! all, only drawn.
//!
//! The gutter numbers the *old* document, so a paragraph that exists only in
//! the new version gets a blank number instead of a number that pretends it was
//! always there.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Align2, Color32, FontId, Galley, Rect, Stroke, TextFormat, Ui, Vec2};

use crate::engine::prosediff::{self, Mark, Op, WordDiff};
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Unified,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Equal,
    /// Same paragraph, edited in place — word-level marks inside it.
    Changed,
    /// A paragraph removed and another added where it stood. Paired only in
    /// side-by-side, where the two versions face each other.
    Pair,
    Insert,
    Delete,
    Gap,
}

/// What to lay out, after the ops have been folded into rows.
enum Plan {
    Eq(String),
    Chg(String, String),
    Rew(String, String),
    Del(String),
    Ins(String),
    Gap(usize),
}

pub struct Row {
    kind: Kind,
    /// Old-document paragraph number; 0 for inserted paragraphs.
    number: usize,
    left: Option<Arc<Galley>>,
    right: Option<Arc<Galley>>,
    gap: usize,
    top: f32,
    height: f32,
}

pub struct Redline {
    rows: Vec<Row>,
    total_height: f32,
    outer_width: f32,
    col_width: f32,
    /// Identifies the layout inputs, so a rebuild only happens when one moves.
    key: u64,
    stats: prosediff::Stats,
}

const GUTTER: f32 = 40.0;
const COL_GAP: f32 = 28.0;
const ROW_PAD_Y: f32 = 3.0;
const MIN_ROW: f32 = 22.0;

fn hash_bytes(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x1000_0000_01b3);
    }
}

impl Redline {
    pub fn stats(&self) -> prosediff::Stats {
        self.stats
    }

    fn needed_width(&self) -> f32 {
        GUTTER * 2.0 + self.col_width * 2.0 + COL_GAP
    }

    /// Rebuild only when an input changed. `width` is the width available for
    /// one column in side-by-side mode, or the whole text width in unified mode.
    pub fn ensure(
        &mut self,
        ctx: &egui::Context,
        ops: &[Op],
        theme: &Theme,
        side: Side,
        width: f32,
        context: usize,
    ) -> bool {
        let mut key = 0u64;
        hash_bytes(&mut key, &width.to_bits().to_le_bytes());
        hash_bytes(&mut key, &(side as u8).to_le_bytes());
        hash_bytes(&mut key, &(context as u64).to_le_bytes());
        hash_bytes(&mut key, &[theme.mode as u8]);
        hash_bytes(&mut key, &[ops.len() as u8]);
        if let Some(first) = ops.first() {
            hash_bytes(&mut key, first_text(first).as_bytes());
        }
        if let Some(last) = ops.last() {
            hash_bytes(&mut key, last_text(last).as_bytes());
        }
        if key == self.key && self.outer_width == width {
            return false;
        }
        self.key = key;
        self.outer_width = width;
        self.stats = prosediff::summarize(ops);
        self.build(ctx, ops, theme, side, width, context);
        true
    }

    fn build(
        &mut self,
        ctx: &egui::Context,
        ops: &[Op],
        theme: &Theme,
        side: Side,
        width: f32,
        context: usize,
    ) {
        let body = theme.body();
        let collapsed = prosediff::collapse(ops, context);
        // One column in side-by-side, the full width in unified. Everything
        // below shapes text to this width, so the two modes never disagree
        // about where a line breaks.
        let col = match side {
            Side::Unified => (width - GUTTER * 2.0).max(120.0),
            _ => ((width - GUTTER * 2.0 - COL_GAP) * 0.5).max(120.0),
        };
        self.col_width = col;
        let width = col;

        // In side-by-side, a deleted paragraph followed by an inserted one is
        // almost always the same paragraph rewritten: Word could not merge it,
        // the author retyped it. Pairing them puts the two versions on one line
        // where the eye can compare them, instead of two rows the reader has to
        // find up and down the page.
        let plan = plan_ops(collapsed, side);

        let mut rows = Vec::with_capacity(plan.len());
        let mut y = 0.0f32;
        let mut number = 0usize;

        for step in &plan {
            let mut row = Row {
                kind: Kind::Equal,
                number: 0,
                left: None,
                right: None,
                gap: 0,
                top: y,
                height: MIN_ROW,
            };
            let shape = |job: LayoutJob| ctx.fonts_mut(|f| f.layout_job(job));

            match step {
                Plan::Eq(text) => {
                    number += 1;
                    row.kind = Kind::Equal;
                    row.number = number;
                    let g = shape(plain(theme, text, &body, width));
                    row.height = (g.size().y + ROW_PAD_Y * 2.0).max(MIN_ROW);
                    match side {
                        Side::Unified => row.left = Some(g),
                        _ => {
                            row.left = Some(g.clone());
                            row.right = Some(g);
                        }
                    }
                }
                Plan::Chg(old, new) => {
                    number += 1;
                    row.kind = Kind::Changed;
                    row.number = number;
                    let d = prosediff::word_diff(old, new);
                    let (l, r) = match side {
                        Side::Unified => (shape(inline(theme, &d, &body, width)), None),
                        _ => (
                            shape(marked(theme, &d.old, Side::Left, &body, width)),
                            Some(shape(marked(theme, &d.new, Side::Right, &body, width))),
                        ),
                    };
                    row.height = row_height(&l, &r);
                    row.left = Some(l);
                    row.right = r;
                }
                Plan::Rew(old, new) => {
                    number += 1;
                    row.kind = Kind::Pair;
                    row.number = number;
                    let one = |mark: Mark, text: &str, side: Side| {
                        let runs = vec![(mark, text.to_owned())];
                        shape(marked(theme, &runs, side, &body, width))
                    };
                    let l = one(Mark::Del, old, Side::Left);
                    let r = one(Mark::Ins, new, Side::Right);
                    row.height = row_height(&l, &Some(r.clone()));
                    row.left = Some(l);
                    row.right = Some(r);
                }
                Plan::Del(text) => {
                    number += 1;
                    row.kind = Kind::Delete;
                    row.number = number;
                    // `inline` renders `WordDiff::inline`, which a bare run does
                    // not populate — a deleted paragraph would vanish from the
                    // unified view. Feed the run straight to the same shaper.
                    let runs = vec![(Mark::Del, text.clone())];
                    let job = marked(theme, &runs, Side::Left, &body, width);
                    let g = shape(job);
                    row.height = (g.size().y + ROW_PAD_Y * 2.0).max(MIN_ROW);
                    row.left = Some(g);
                }
                Plan::Ins(text) => {
                    row.kind = Kind::Insert;
                    let runs = vec![(Mark::Ins, text.clone())];
                    let job = marked(theme, &runs, Side::Right, &body, width);
                    let g = shape(job);
                    row.height = (g.size().y + ROW_PAD_Y * 2.0).max(MIN_ROW);
                    if side == Side::Unified {
                        row.left = Some(g);
                    } else {
                        row.right = Some(g);
                    }
                }
                Plan::Gap(n) => {
                    row.kind = Kind::Gap;
                    row.gap = *n;
                    row.height = 26.0;
                }
            }
            y += row.height;
            rows.push(row);
        }

        self.rows = rows;
        self.total_height = y;
    }

    /// Paint the visible slice. Returns the y offset the scroll area should use
    /// if the caller asked to jump somewhere.
    pub fn show(&self, ui: &mut Ui, theme: &Theme, scroll_to: Option<usize>) -> f32 {
        // One column was laid out to `outer_width`; the row reserves exactly
        // what that layout needs, so the scroll area never grows a horizontal
        // scrollbar and the second column never slides under the first.
        let width = self.outer_width.max(self.needed_width());
        let full = Vec2::new(width, self.total_height.max(1.0));
        let mut area = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("redline");

        if let Some(idx) = scroll_to {
            if let Some(row) = self.rows.get(idx) {
                area = area.vertical_scroll_offset((row.top - 80.0).max(0.0));
            }
        }

        let mut visible_top = 0.0f32;
        area.show_viewport(ui, |ui, viewport| {
            let (_, rect) = ui.allocate_space(full);
            visible_top = rect.top();
            let origin = rect.min;

            // `viewport` arrives in content coordinates (min == ZERO means the
            // top of the document), while `rect` is on screen. They must not be
            // intersected directly — that would clip the content to the wrong
            // box, which is how the right-hand column used to get cut off.
            let screen = Rect::from_min_max(
                origin + viewport.min.to_vec2(),
                origin + viewport.max.to_vec2(),
            );
            let clip = screen.intersect(rect).intersect(ui.clip_rect());

            // Binary search the first row that can touch the viewport.
            let start = self
                .rows
                .partition_point(|r| r.top + r.height < viewport.top())
                .saturating_sub(1);
            let end = self
                .rows
                .partition_point(|r| r.top < viewport.bottom() + 1.0)
                .min(self.rows.len());

            let painter = ui.painter().with_clip_rect(clip);

            let col = self.col_width;
            let _ = width;

            for row in &self.rows[start..end] {
                let y = origin.y + row.top;
                let band = Rect::from_min_size(egui::pos2(origin.x, y), Vec2::new(self.outer_width, row.height));

                match row.kind {
                    Kind::Gap => {
                        let cy = y + row.height * 0.5;
                        let stroke = Stroke::new(1.0, theme.rule);
                        painter.hline(band.x_range(), cy, stroke);
                        let text = format!("{} 段未改动", row.gap);
                        let g = painter.layout_no_wrap(text, theme.ui(11.5), theme.faint);
                        let pad = egui::vec2(10.0, 0.0);
                        let box_rect = Rect::from_center_size(
                            egui::pos2(band.center().x, cy),
                            g.size() + pad * 2.0,
                        );
                        painter.rect_filled(box_rect, 3.0, theme.paper);
                        painter.galley(
                            egui::pos2(box_rect.center().x - g.size().x * 0.5, cy - g.size().y * 0.5),
                            g,
                            theme.faint,
                        );
                    }
                    Kind::Equal => {
                        // Unchanged text is present on both sides but carries no
                        // information; dimming it is what lets the eye find the
                        // changes without reading the document.
                        self.paint_body(&painter, row, theme, band, col, 0.55);
                    }
                    Kind::Changed | Kind::Pair | Kind::Delete | Kind::Insert => {
                        self.paint_body(&painter, row, theme, band, col, 1.0)
                    }
                }

                // Gutter number, in the old document's numbering.
                if row.number > 0 {
                    painter.text(
                        egui::pos2(origin.x + GUTTER - 12.0, y + row.height * 0.5),
                        Align2::RIGHT_CENTER,
                        row.number.to_string(),
                        theme.num(11.0),
                        theme.faint.gamma_multiply(0.85),
                    );
                }
            }
        });
        visible_top
    }

    fn paint_body(
        &self,
        painter: &egui::Painter,
        row: &Row,
        theme: &Theme,
        band: Rect,
        col: f32,
        alpha: f32,
    ) {
        let left_x = band.left() + GUTTER;
        let right_x = left_x + col + COL_GAP;
        let pad = Vec2::new(4.0, ROW_PAD_Y);

        let paint_side = |galley: &Arc<Galley>, x: f32, tint: Option<Color32>, rule: Option<Color32>| {
            let h = galley.size().y;
            let top = band.top() + 1.0;
            let tall = h + ROW_PAD_Y * 2.0;
            if let Some(c) = tint {
                painter.rect_filled(
                    Rect::from_min_size(egui::pos2(x - 6.0, top), Vec2::new(col + 12.0, tall)),
                    2.0,
                    c.gamma_multiply(alpha),
                );
            }
            if let Some(c) = rule {
                // The marker lives in the margin, clear of the first glyph, so a
                // change reads as a pencil line beside the text rather than a
                // coloured bar struck through it.
                painter.rect_filled(
                    Rect::from_min_size(egui::pos2(x - 11.0, top), Vec2::new(2.0, tall)),
                    1.0,
                    c,
                );
            }
            painter.galley(egui::pos2(x, band.top() + pad.y), galley.clone(), theme.ink.gamma_multiply(alpha));
        };

        match row.kind {
            Kind::Equal => {
                if let Some(g) = &row.left {
                    paint_side(g, left_x, None, None);
                }
                if let Some(g) = &row.right {
                    paint_side(g, right_x, None, None);
                }
            }
            Kind::Changed | Kind::Pair => {
                if let Some(g) = &row.left {
                    paint_side(g, left_x, Some(theme.del_bg), Some(theme.del_ink));
                }
                if let Some(g) = &row.right {
                    paint_side(g, right_x, Some(theme.ins_bg), Some(theme.ins_ink));
                }
            }
            Kind::Delete => {
                if let Some(g) = &row.left {
                    paint_side(g, left_x, Some(theme.del_bg), Some(theme.del_ink));
                }
            }
            Kind::Insert => {
                // Unified has no right column, so an insertion lives on the
                // left; side-by-side puts it where it belongs, on the new side.
                if let Some(g) = &row.right {
                    paint_side(g, right_x, Some(theme.ins_bg), Some(theme.ins_ink));
                } else if let Some(g) = &row.left {
                    paint_side(g, left_x, Some(theme.ins_bg), Some(theme.ins_ink));
                }
            }
            Kind::Gap => {}
        }
    }

    /// Index of the next changed row after `from`, wrapping to the first.
    pub fn next_change(&self, from: usize) -> Option<usize> {
        let after = self.rows.iter().enumerate().skip(from + 1).find(|(_, r)| r.kind.is_change());
        after.or_else(|| self.rows.iter().enumerate().find(|(_, r)| r.kind.is_change())).map(|(i, _)| i)
    }

    pub fn prev_change(&self, from: usize) -> Option<usize> {
        let before = self.rows[..from.min(self.rows.len())]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| r.kind.is_change());
        before
            .or_else(|| self.rows.iter().enumerate().rev().find(|(_, r)| r.kind.is_change()))
            .map(|(i, _)| i)
    }

}

impl Kind {
    fn is_change(&self) -> bool {
        matches!(self, Kind::Changed | Kind::Pair | Kind::Insert | Kind::Delete)
    }
}

impl Default for Redline {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            total_height: 0.0,
            outer_width: -1.0,
            col_width: 0.0,
            key: 0,
            stats: Default::default(),
        }
    }
}

/// Fold the op stream into rows.
///
/// Unified keeps the document order — deletions, then insertions, the way git
/// prints a patch. Side-by-side pairs each `delete` run with the `insert` run
/// that follows it, so a replaced paragraph occupies one row instead of two.
fn plan_ops(ops: Vec<Op>, side: Side) -> Vec<Plan> {
    if side == Side::Unified {
        return ops
            .into_iter()
            .map(|op| match op {
                Op::Equal(t) => Plan::Eq(t),
                Op::Changed(o, n) => Plan::Chg(o, n),
                Op::Delete(t) => Plan::Del(t),
                Op::Insert(t) => Plan::Ins(t),
                Op::Gap(n) => Plan::Gap(n),
            })
            .collect();
    }

    let mut out = Vec::with_capacity(ops.len());
    let mut i = 0;
    while i < ops.len() {
        if matches!(ops[i], Op::Delete(_)) {
            let j = ops[i..]
                .iter()
                .position(|o| !matches!(o, Op::Delete(_)))
                .map_or(ops.len(), |k| i + k);
            let k = ops[j..]
                .iter()
                .position(|o| !matches!(o, Op::Insert(_)))
                .map_or(ops.len(), |m| j + m);
            let dels: Vec<&String> = ops[i..j]
                .iter()
                .filter_map(|o| if let Op::Delete(t) = o { Some(t) } else { None })
                .collect();
            let inss: Vec<&String> = ops[j..k]
                .iter()
                .filter_map(|o| if let Op::Insert(t) = o { Some(t) } else { None })
                .collect();
            for t in 0..dels.len().max(inss.len()) {
                match (dels.get(t), inss.get(t)) {
                    // Only pair when the engine itself would have paired them.
                    // Position alone is not evidence: a clause deleted from the
                    // top of a run and an unrelated clause inserted at its end
                    // must not be shown as one paragraph rewritten.
                    (Some(d), Some(n))
                        if prosediff::similarity(d, n) >= prosediff::PAIR_THRESHOLD =>
                    {
                        out.push(Plan::Rew((*d).clone(), (*n).clone()))
                    }
                    (Some(d), Some(n)) => {
                        out.push(Plan::Del((*d).clone()));
                        out.push(Plan::Ins((*n).clone()));
                    }
                    (Some(d), None) => out.push(Plan::Del((*d).clone())),
                    (None, Some(n)) => out.push(Plan::Ins((*n).clone())),
                    (None, None) => {}
                }
            }
            i = k;
            continue;
        }
        match &ops[i] {
            Op::Equal(t) => out.push(Plan::Eq(t.clone())),
            Op::Changed(o, n) => out.push(Plan::Chg(o.clone(), n.clone())),
            Op::Insert(t) => out.push(Plan::Ins(t.clone())),
            Op::Delete(_) => unreachable!("handled above"),
            Op::Gap(n) => out.push(Plan::Gap(*n)),
        }
        i += 1;
    }
    out
}

fn row_height(left: &Arc<Galley>, right: &Option<Arc<Galley>>) -> f32 {
    let r = right.as_ref().map(|g| g.size().y).unwrap_or(0.0);
    (left.size().y.max(r) + ROW_PAD_Y * 2.0).max(MIN_ROW)
}

fn first_text(op: &Op) -> String {
    match op {
        Op::Equal(t) | Op::Insert(t) | Op::Delete(t) => t.clone(),
        Op::Changed(a, _) => a.clone(),
        Op::Gap(_) => String::new(),
    }
}

fn last_text(op: &Op) -> String {
    match op {
        Op::Equal(t) | Op::Insert(t) | Op::Delete(t) => t.clone(),
        Op::Changed(_, b) => b.clone(),
        Op::Gap(_) => String::new(),
    }
}

fn plain(theme: &Theme, text: &str, font: &FontId, width: f32) -> LayoutJob {
    LayoutJob::simple(text.to_owned(), font.clone(), theme.ink, width)
}

/// One side of a changed paragraph, with word-level marks.
fn marked(theme: &Theme, runs: &[(Mark, String)], side: Side, font: &FontId, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    for (mark, text) in runs {
        let fmt = match mark {
            Mark::Same => TextFormat {
                font_id: font.clone(),
                color: theme.ink,
                ..Default::default()
            },
            Mark::Del => TextFormat {
                font_id: font.clone(),
                color: theme.del_ink,
                background: theme.del_bg,
                strikethrough: theme.del_strike(),
                ..Default::default()
            },
            Mark::Ins => TextFormat {
                font_id: font.clone(),
                color: theme.ins_ink,
                background: theme.ins_bg,
                underline: Stroke::new(1.0, theme.ins_ink.gamma_multiply(0.5)),
                ..Default::default()
            },
        };
        let _ = side;
        job.append(text, 0.0, fmt);
    }
    job
}

/// Both sides interleaved on one line — deletions first, as a unified diff.
fn inline(theme: &Theme, d: &WordDiff, font: &FontId, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    for (mark, text) in &d.inline {
        let fmt = match mark {
            Mark::Same => TextFormat { font_id: font.clone(), color: theme.ink, ..Default::default() },
            Mark::Del => TextFormat {
                font_id: font.clone(),
                color: theme.del_ink,
                background: theme.del_bg,
                strikethrough: theme.del_strike(),
                ..Default::default()
            },
            Mark::Ins => TextFormat {
                font_id: font.clone(),
                color: theme.ins_ink,
                background: theme.ins_bg,
                ..Default::default()
            },
        };
        job.append(text, 0.0, fmt);
    }
    job
}


