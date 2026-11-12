//! The window.
//!
//! Document-first, not dashboard-first: there is no overview screen, no charts
//! of commit counts, no cards. You open a store, you pick a document, and you
//! are looking at the redline. Every toolbar button maps onto one CLI verb, so
//! nothing exists here that `docver` cannot do from a shell — a GUI that can do
//! more than its CLI is a GUI you cannot script, and a GUI that can do less is
//! one you have to leave.

use std::path::PathBuf;

use egui::{RichText, Sense, Stroke, Vec2};

use egui::Rect;

use crate::engine::prosediff::{self, Op};
use crate::engine::store::{Snapshot, Store};
use crate::engine::text;
use crate::theme::{Mode, Theme};
use crate::view::{Redline, Side};

pub struct App {
    theme: Theme,
    store: Option<Store>,
    /// Path typed by the user, kept even when the store fails to open so the
    /// error can be shown next to what was typed.
    path_input: String,
    error: Option<String>,
    documents: Vec<String>,
    selected: Option<String>,
    history: Vec<Snapshot>,
    /// Index into `history` (newest first). `None` means the working copy.
    from: Option<usize>,
    to: Option<usize>,
    ops: Vec<Op>,
    redline: Redline,
    side: Side,
    context: usize,
    /// Set when the palette changed, so the redline is re-shaped on the next
    /// frame. A style change is applied by egui at the *start* of a frame, and
    /// that rebuild replaces the font atlas — galleys shaped before it point
    /// into an atlas that no longer exists and paint as garbage.
    retheme: bool,
    /// Row index to scroll to on the next frame, from next/prev change.
    scroll_to: Option<usize>,
    /// Cached text of both sides, so a redraw never re-parses a zip.
    cache: Option<(String, String, String)>,
    message: String,
    status: String,
    serif_ok: bool,
    sans_ok: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (sans_ok, serif_ok) = crate::theme::install_fonts(&cc.egui_ctx);
        let theme = Theme::new(Mode::Light);
        theme.apply(&cc.egui_ctx);

        let mut app = Self {
            theme,
            store: None,
            path_input: std::env::args().nth(1).unwrap_or_else(|| {
                std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()
            }),
            error: None,
            documents: Vec::new(),
            selected: None,
            history: Vec::new(),
            from: Some(1),
            to: Some(0),
            ops: Vec::new(),
            redline: Redline::default(),
            side: Side::Left,
            context: 2,
            retheme: false,
            scroll_to: None,
            cache: None,
            message: String::new(),
            status: String::new(),
            serif_ok,
            sans_ok,
        };
        app.side = Side::Left;
        let p = app.path_input.clone();
        if !p.is_empty() {
            // Launched without an argument: whatever the shell's cwd happens to
            // be is not a choice the user made, so a failed guess is not an
            // error worth a red line.
            let quiet = std::env::args().nth(1).is_none();
            app.open_opt(&p, quiet);
        }
        app
    }

    // ------------------------------------------------------------- store ops

    fn open(&mut self, path: &str) {
        self.open_opt(path, false);
    }

    fn open_opt(&mut self, path: &str, quiet: bool) {
        // Absolute from here on: a relative "." in the path field and in the
        // error line tells the reader nothing about what was actually opened.
        let typed = PathBuf::from(shellexpand(path));
        let p = typed.canonicalize().unwrap_or(typed);
        let store = Store::discover(&p).or_else(|| {
            if p.is_dir() {
                Store::discover(&p)
            } else {
                None
            }
        });
        match store {
            Some(s) => {
                self.error = None;
                self.path_input = s.root().display().to_string();
                self.documents = s.documents();
                // A file argument means "show me this one", not "show me the
                // first file in the repo".
                let wanted = p
                    .strip_prefix(s.root())
                    .ok()
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .filter(|w| self.documents.iter().any(|d| d == w));
                self.store = Some(s);
                self.history.clear();
                self.ops.clear();
                self.cache = None;
                self.selected = wanted.or_else(|| self.documents.first().cloned());
                self.reload_history();
            }
            None => {
                // Not a store yet: say so, but offer to make one rather than
                // leaving the user with a dead end.
                self.store = None;
                self.documents.clear();
                if quiet {
                    // Nothing was asked for: leave the field empty so its hint
                    // shows, rather than pre-filling a path that is not a store.
                    self.path_input.clear();
                } else {
                    self.path_input = p.display().to_string();
                    self.error = Some(format!(
                        "{} 不是文档仓库。可以新建一个，或选择已有仓库。",
                        p.display()
                    ));
                }
            }
        }
    }

    fn init_here(&mut self) {
        let p = PathBuf::from(shellexpand(&self.path_input));
        match Store::init(&p) {
            Ok(s) => {
                self.path_input = s.root().display().to_string();
                self.store = Some(s);
                self.error = None;
                self.documents = self.store.as_ref().map(|s| s.documents()).unwrap_or_default();
                self.selected = self.documents.first().cloned();
                self.status = "已建立仓库".into();
                self.reload_history();
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn reload_history(&mut self) {
        self.history = match (&self.store, &self.selected) {
            (Some(s), Some(doc)) => s.history(doc),
            _ => Vec::new(),
        };
        self.cache = None;
        // Default comparison: with two or more snapshots, the previous one
        // against the newest; with exactly one, that snapshot against the
        // working copy. Never "nothing against the working copy" — an empty
        // left side makes every paragraph read as an insertion, which is
        // true and useless.
        match self.history.len() {
            0 => {
                self.from = None;
                self.to = None;
            }
            1 => {
                self.from = Some(0);
                self.to = None;
            }
            _ => {
                self.from = Some(1);
                self.to = Some(0);
            }
        }
        self.recompute();
    }

    fn select(&mut self, doc: String) {
        self.selected = Some(doc);
        self.reload_history();
    }

    fn recompute(&mut self) {
        self.ops.clear();
        let (Some(store), Some(doc)) = (self.store.as_ref(), self.selected.clone()) else {
            return;
        };
        if self.from.is_none() && self.to.is_none() {
            self.redline = Redline::default();
            return;
        }

        let left = match self.from {
            Some(i) => self.history.get(i).and_then(|s| store.blob_at(s.id, &doc)),
            None => None,
        };
        let right = match self.to {
            Some(i) => self.history.get(i).and_then(|s| store.blob_at(s.id, &doc)),
            None => store.worktree_bytes(&doc),
        };

        let ext = text::ext_of(&doc);
        let left_text = match &left {
            Some(bytes) => match text::to_text(bytes, &ext) {
                Ok(t) => t,
                Err(e) => {
                    self.error = Some(format!("读取旧版本失败：{e}"));
                    return;
                }
            },
            None => String::new(),
        };
        let right_text = match &right {
            Some(bytes) => match text::to_text(bytes, &ext) {
                Ok(t) => t,
                Err(e) => {
                    self.error = Some(format!("读取新版本失败：{e}"));
                    return;
                }
            },
            None => String::new(),
        };

        self.error = None;
        self.ops = prosediff::align(&left_text, &right_text);
        self.cache = Some((doc, left_text, right_text));
        self.redline = Redline::default();
        self.scroll_to = Some(0);
    }

    fn save_snapshot(&mut self) {
        let (Some(store), Some(doc)) = (self.store.as_ref(), self.selected.clone()) else {
            return;
        };
        if !store.worktree_exists(&doc) {
            self.error = Some("工作区里没有这个文件".into());
            return;
        }
        let msg = if self.message.trim().is_empty() {
            format!("{} 快照", doc)
        } else {
            self.message.clone()
        };
        match store.save(&doc, &msg) {
            Ok(_) => {
                self.status = format!("已存快照：{msg}");
                self.message.clear();
                self.reload_history();
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn restore(&mut self, idx: usize) {
        let (Some(store), Some(doc)) = (self.store.as_ref(), self.selected.clone()) else {
            return;
        };
        let Some(snap) = self.history.get(idx) else { return };
        match store.restore(snap.id, &doc) {
            Ok(p) => {
                self.status = format!("已恢复到 {}（原文件未改动）", p.display());
                self.reload_history();
            }
            Err(e) => self.error = Some(e),
        }
    }

    // ------------------------------------------------------------- drawing

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;

            let label = self
                .selected
                .clone()
                .unwrap_or_else(|| "未打开文档".to_string());
            ui.label(RichText::new(label).font(self.theme.ui(15.0)).color(self.theme.ink));

            ui.add_space(4.0);
            let stats = self.redline.stats();
            if stats.total() > 0 {
                let mut bits = Vec::new();
                if stats.modified > 0 {
                    bits.push(format!("{} 段修改", stats.modified));
                }
                if stats.added > 0 {
                    bits.push(format!("{} 段新增", stats.added));
                }
                if stats.removed > 0 {
                    bits.push(format!("{} 段删除", stats.removed));
                }
                ui.label(
                    RichText::new(bits.join(" · "))
                        .font(self.theme.ui(12.5))
                        .color(self.theme.dim),
                );
            } else if !self.ops.is_empty() {
                ui.label(RichText::new("没有差异").font(self.theme.ui(12.5)).color(self.theme.faint));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(RichText::new(if self.theme.mode == Mode::Light { "暗" } else { "亮" }).size(12.5))
                    .clicked()
                {
                    self.theme = Theme::new(match self.theme.mode {
                        Mode::Light => Mode::Dark,
                        Mode::Dark => Mode::Light,
                    });
                    self.theme.apply(ui.ctx());
                    self.retheme = true;
                }
                // The current view is the pressed one: a mode that looks the
                // same whether or not it is active makes the reader check.
                if ui.selectable_label(self.side == Side::Left, "并排").clicked()
                    && self.side != Side::Left
                {
                    self.side = Side::Left;
                    self.redline = Redline::default();
                }
                if ui.selectable_label(self.side == Side::Unified, "统一视图").clicked()
                    && self.side != Side::Unified
                {
                    self.side = Side::Unified;
                    self.redline = Redline::default();
                }

                ui.separator();

                if ui.button("上一处").clicked() {
                    let from = self.scroll_to.unwrap_or(usize::MAX / 2);
                    self.scroll_to = self.redline.prev_change(from);
                }
                if ui.button("下一处").clicked() {
                    let from = self.scroll_to.unwrap_or(usize::MAX / 2);
                    self.scroll_to = self.redline.next_change(from);
                }
            });
        });
    }

    fn rail(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("仓库").font(self.theme.ui(11.5)).color(self.theme.faint));
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 62.0).max(60.0);
            let mut s = self.path_input.clone();
            let resp = ui.add_sized(
                [w, 24.0],
                egui::TextEdit::singleline(&mut s)
                    .font(self.theme.num(11.5))
                    .hint_text("文档仓库路径"),
            );
            if resp.changed() {
                self.path_input = s.clone();
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.open(&s);
            }
            if ui.button("打开").clicked() {
                let s = self.path_input.clone();
                self.open(&s);
            }
        });
        if self.store.is_none() {
            if ui.button("在此新建仓库").clicked() {
                self.init_here();
            }
        }

        ui.add_space(10.0);
        ui.label(RichText::new("文档").font(self.theme.ui(11.5)).color(self.theme.faint));
        ui.add_space(2.0);

        let docs = self.documents.clone();
        egui::ScrollArea::vertical()
            .id_salt("docs")
            .max_height(160.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for doc in docs {
                    let selected = self.selected.as_deref() == Some(doc.as_str());
                    let name = doc.rsplit('/').next().unwrap_or(&doc).to_string();
                    let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), 22.0));
                    // Paint the highlight before the text: a filled rect drawn
                    // after a label covers it, which reads as an empty row.
                    if selected {
                        ui.painter().rect_filled(rect, 3.0, self.theme.hover);
                    }
                    let galley = ui.painter().layout_no_wrap(
                        name,
                        self.theme.ui(13.0),
                        if selected { self.theme.ink } else { self.theme.dim },
                    );
                    let clipped = rect.shrink2(Vec2::new(4.0, 0.0));
                    ui.painter().with_clip_rect(clipped).galley(
                        egui::pos2(clipped.left(), rect.center().y - galley.size().y * 0.5),
                        galley,
                        self.theme.ink,
                    );
                    let resp = ui.interact(rect, ui.id().with(("doc", doc.as_str())), Sense::click());
                    if resp.clicked() {
                        self.select(doc.clone());
                    }
                }
            });

        ui.add_space(12.0);
        ui.label(RichText::new("版本").font(self.theme.ui(11.5)).color(self.theme.faint));
        ui.add_space(2.0);

        // Two selection columns: which version is on the left, which on the
        // right. The working copy is a row like any other, because "compare
        // against what I have now" is the common case and should not be hidden
        // behind a checkbox.
        let mut pick_from: Option<Option<usize>> = None;
        let mut pick_to: Option<Option<usize>> = None;
        let history = self.history.clone();
        let (from, to) = (self.from, self.to);

        // The history list must not eat the footer: the save row, the context
        // slider and the font notice are drawn after it, and an unbounded
        // scroll area pushes them off the bottom of the window.
        let footer = 118.0;
        let list_h = (ui.available_height() - footer).max(90.0);
        egui::ScrollArea::vertical()
            .id_salt("history")
            .auto_shrink([false, false])
            .max_height(list_h)
            .show(ui, |ui| {
                if history.is_empty() {
                    ui.label(
                        RichText::new("还没有快照")
                            .font(self.theme.ui(12.0))
                            .color(self.theme.faint),
                    );
                }
                for (i, snap) in history.iter().enumerate() {
                    let is_from = from == Some(i);
                    let is_to = to == Some(i);
                    let row_h = 40.0;
                    let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), row_h));
                    let hovered = ui.rect_contains_pointer(rect);

                    if hovered {
                        ui.painter().rect_filled(rect, 3.0, self.theme.hover);
                    }
                    if is_from || is_to {
                        ui.painter().rect_filled(
                            Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
                            0.0,
                            if is_to { self.theme.ins_ink } else { self.theme.del_ink },
                        );
                    }

                    let x = rect.left() + 10.0;
                    // A snapshot message is free text and can be longer than the
                    // rail. One row with an ellipsis beats a word cut in half.
                    let mut job = egui::text::LayoutJob::single_section(
                        snap.subject.clone(),
                        egui::TextFormat {
                            font_id: self.theme.ui(12.5),
                            color: self.theme.ink,
                            ..Default::default()
                        },
                    );
                    job.wrap = egui::text::TextWrapping {
                        max_width: rect.width() - 34.0,
                        max_rows: 1,
                        break_anywhere: false,
                        overflow_character: Some('…'),
                    };
                    let g = ui.ctx().fonts_mut(|f| f.layout_job(job));
                    ui.painter().galley(egui::pos2(x, rect.top() + 8.0), g, self.theme.ink);
                    ui.painter().text(
                        egui::pos2(x, rect.top() + 23.0),
                        egui::Align2::LEFT_TOP,
                        format!("{} · {}", snap.short, snap.when_text()),
                        self.theme.num(10.5),
                        self.theme.faint,
                    );

                    // Two hit zones per row: left half selects the old side,
                    // right half the new side.
                    let left_zone = Rect::from_min_size(rect.min, Vec2::new(rect.width() * 0.5, rect.height()));
                    let right_zone = Rect::from_min_size(
                        egui::pos2(rect.center().x, rect.top()),
                        Vec2::new(rect.width() * 0.5, rect.height()),
                    );
                    for (zone, which) in [(left_zone, 0u8), (right_zone, 1u8)] {
                        let r = ui.interact(zone, ui.id().with(("hist", i, which)), Sense::click())
                            .on_hover_text(format!(
                                "{} · {} · {}\n左半选为旧版，右半选为新版，右键恢复到工作区",
                                snap.subject, snap.author, snap.short
                            ));
                        if r.clicked() {
                            if which == 0 {
                                pick_from = Some(Some(i));
                            } else {
                                pick_to = Some(Some(i));
                            }
                        }
                        if r.secondary_clicked() {
                            self.restore(i);
                        }
                        if r.hovered() {
                            ui.painter().rect_stroke(
                                zone,
                                3.0,
                                Stroke::new(1.0, self.theme.rule),
                                egui::StrokeKind::Inside,
                            );
                        }
                    }

                    if is_from {
                        ui.painter().text(
                            egui::pos2(rect.right() - 8.0, rect.top() + 8.0),
                            egui::Align2::RIGHT_TOP,
                            "旧",
                            self.theme.ui(10.5),
                            self.theme.del_ink,
                        );
                    }
                    if is_to {
                        ui.painter().text(
                            egui::pos2(rect.right() - 8.0, rect.top() + 8.0),
                            egui::Align2::RIGHT_TOP,
                            "新",
                            self.theme.ui(10.5),
                            self.theme.ins_ink,
                        );
                    }
                }

                // The working copy is a version too.
                let row_h = 40.0;
                let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), row_h));
                let selected = to.is_none();
                if selected {
                    ui.painter().rect_filled(rect, 3.0, self.theme.hover);
                    ui.painter().rect_filled(
                        Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
                        0.0,
                        self.theme.ins_ink,
                    );
                }
                ui.painter().text(
                    egui::pos2(rect.left() + 10.0, rect.top() + 8.0),
                    egui::Align2::LEFT_TOP,
                    "工作区（未存）",
                    self.theme.ui(12.5),
                    self.theme.ink,
                );
                let r = ui.interact(rect, ui.id().with("worktree"), Sense::click());
                if r.clicked() {
                    pick_to = Some(None);
                }
            });

        if let Some(v) = pick_from {
            self.from = v;
            self.recompute();
        }
        if let Some(v) = pick_to {
            self.to = v;
            self.recompute();
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            let w = (ui.available_width() - 58.0).max(60.0);
            ui.add_sized(
                [w, 24.0],
                egui::TextEdit::singleline(&mut self.message)
                    .font(self.theme.ui(12.0))
                    .hint_text("快照说明"),
            );
            if ui.button("存快照").clicked() {
                self.save_snapshot();
            }
        });

        // Context is a count, so it is a stepper rather than a slider: a slider
        // makes the reader aim at a number the document already knows, and its
        // track is invisible against a paper-coloured panel anyway.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                RichText::new("上下文")
                    .font(self.theme.ui(11.0))
                    .color(self.theme.faint),
            );
            if ui
                .add_enabled(self.context > 0, egui::Button::new(RichText::new("−").size(12.0)))
                .clicked()
            {
                self.context -= 1;
                self.redline = Redline::default();
            }
            ui.label(
                RichText::new(format!("{} 段", self.context))
                    .font(self.theme.num(11.0))
                    .color(self.theme.dim),
            );
            if ui
                .add_enabled(self.context < 20, egui::Button::new(RichText::new("+").size(12.0)))
                .clicked()
            {
                self.context += 1;
                self.redline = Redline::default();
            }
        });

        if !self.serif_ok || !self.sans_ok {
            ui.add_space(4.0);
            ui.label(
                RichText::new(if !self.serif_ok {
                    "未找到 Noto Serif CJK，正文回退为黑体"
                } else {
                    "未找到 Noto Sans CJK，界面文字可能缺字"
                })
                .font(self.theme.ui(10.5))
                .color(self.theme.faint),
            );
        }
    }
}

fn shellexpand(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    p.to_string()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;

        // The new style is in force from this frame on, so this is the first
        // moment a galley can be shaped against the atlas it will be painted
        // with.
        if std::mem::take(&mut self.retheme) {
            self.redline = Redline::default();
        }
        // Drag a document or a folder onto the window: the fastest way in.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if let Some(path) = dropped.into_iter().next() {
            let s = path.display().to_string();
            if path.is_file() {
                // Dropping a file opens its store, and selects that file.
                self.open(&s);
                if let Some(store) = &self.store {
                    let rel = path
                        .strip_prefix(store.root())
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    if !rel.is_empty() {
                        self.select(rel);
                    }
                }
            } else {
                self.open(&s);
            }
        }

        egui::Panel::top("top")
            .exact_size(44.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(14, 6))
                    .fill(self.theme.paper),
            )
            .show(ui, |ui| {
                self.toolbar(ui);
            });

        // A 1px rule under the toolbar, drawn rather than framed: a border on
        // every panel compounds into a grid, and a redline should have one line.
        egui::Panel::top("rule")
            .exact_size(1.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(self.theme.rule))
            .show(ui, |_| {});

        if !self.status.is_empty() {
            let msg = self.status.clone();
            egui::Panel::bottom("status")
                .exact_size(26.0)
                .show_separator_line(false)
                .frame(
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(14, 4))
                        .fill(self.theme.rail),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(msg).font(self.theme.ui(11.5)).color(self.theme.dim));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").clicked() {
                                self.status.clear();
                            }
                        });
                    });
                });
        }

        egui::Panel::left("rail")
            .exact_size(272.0)
            .show_separator_line(false)
            .resizable(true)
            .frame(
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(12, 4))
                    .fill(self.theme.rail),
            )
            .show(ui, |ui| {
                self.rail(ui);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(18, 10))
                    .fill(self.theme.paper),
            )
            .show(ui, |ui| {
                if let Some(err) = self.error.clone() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(err).font(self.theme.ui(12.5)).color(self.theme.del_ink));
                    ui.add_space(6.0);
                }

                if self.selected.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("选择一个文档，或把 .docx 拖进窗口")
                                .font(self.theme.body())
                                .color(self.theme.faint),
                        );
                    });
                    return;
                }

                if self.ops.is_empty() && self.selected.is_some() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("还没有快照可以对照")
                                .font(self.theme.body())
                                .color(self.theme.faint),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("先存一个快照，之后每次改动都会和它逐词对照")
                                .font(self.theme.ui(11.5))
                                .color(self.theme.faint),
                        );
                    });
                    return;
                }

                if !prosediff::has_changes(&self.ops) && !self.ops.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("两个版本正文一致")
                                .font(self.theme.body())
                                .color(self.theme.faint),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("改动可能只在格式、批注或页眉页脚里")
                                .font(self.theme.ui(11.5))
                                .color(self.theme.faint),
                        );
                    });
                    return;
                }

                let width = ui.available_width();
                let ops = self.ops.clone();
                if self.redline.ensure(ctx, &ops, &self.theme, self.side, width, self.context) {
                    // Rebuilt: the cached row offsets are gone, so honour any
                    // pending jump on this frame.
                }
                let jump = self.scroll_to.take();
                self.redline.show(ui, &self.theme, jump);
            });
    }
}
