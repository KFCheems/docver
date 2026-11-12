//! Palette and typography.
//!
//! The diff is meant to read like a redline on paper, so the surface is warm
//! off-white rather than the cool grey of a dashboard, body text is a serif,
//! and changes are marked with colour *and* a rule in the gutter — colour alone
//! fails for the ~8% of men who cannot separate the two most natural choices,
//! red and green.
//!
//! Fonts are Noto CJK *variable* collections with five faces inside one .ttc:
//! JP, KR, SC, TC, HK. The face index is what picks the language — index 2 is
//! Simplified Chinese — and getting it wrong silently renders a Chinese contract
//! with Japanese glyph forms (骨, 直, 令 all differ), which is a real defect in a
//! document that gets signed.
//!
//! Both collections also default to their thinnest weight (wght 100 sans, 200
//! serif), so the weight axis has to be set explicitly: 400 for body text.
//! Note `fc-scan` reports indices like 262146 for the same face — that number
//! encodes face * 131072 + named instance and is *not* what skrifa wants.

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke};

pub const FONT_SANS: &str = "/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc";
pub const FONT_SERIF: &str = "/usr/share/fonts/google-noto-serif-cjk-vf-fonts/NotoSerifCJK-VF.ttc";
pub const FONT_MONO: &str = "/usr/share/fonts/jetbrains-mono-fonts/JetBrainsMono-Regular.otf";
/// Face 2 of each collection is "Noto {Sans,Serif} CJK SC".
const SC_FACE: u32 = 2;
/// The weight axis, pinned to Regular; the collections default to Thin.
const WEIGHT_REGULAR: f32 = 400.0;

pub const SERIF: &str = "serif";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub mode: Mode,
    pub paper: Color32,
    pub rail: Color32,
    pub ink: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub rule: Color32,
    pub del_ink: Color32,
    pub del_bg: Color32,
    pub ins_ink: Color32,
    pub ins_bg: Color32,
    pub accent: Color32,
    pub hover: Color32,
}

impl Theme {
    pub fn new(mode: Mode) -> Self {
        match mode {
            Mode::Light => Self {
                mode,
                paper: Color32::from_rgb(0xfb, 0xfa, 0xf7),
                rail: Color32::from_rgb(0xf3, 0xf1, 0xec),
                ink: Color32::from_rgb(0x1c, 0x1b, 0x18),
                dim: Color32::from_rgb(0x6f, 0x6a, 0x5e),
                faint: Color32::from_rgb(0x9a, 0x94, 0x86),
                rule: Color32::from_rgb(0xe4, 0xe0, 0xd6),
                del_ink: Color32::from_rgb(0x8e, 0x2b, 0x1e),
                del_bg: Color32::from_rgb(0xfa, 0xec, 0xe8),
                ins_ink: Color32::from_rgb(0x1d, 0x5f, 0x2f),
                ins_bg: Color32::from_rgb(0xe9, 0xf3, 0xe7),
                accent: Color32::from_rgb(0x3d, 0x55, 0x6b),
                hover: Color32::from_rgb(0xf0, 0xee, 0xe7),
            },
            Mode::Dark => Self {
                mode,
                paper: Color32::from_rgb(0x17, 0x18, 0x19),
                rail: Color32::from_rgb(0x1e, 0x20, 0x22),
                ink: Color32::from_rgb(0xe6, 0xe3, 0xdb),
                dim: Color32::from_rgb(0x9c, 0x97, 0x8b),
                faint: Color32::from_rgb(0x6d, 0x69, 0x60),
                rule: Color32::from_rgb(0x2f, 0x32, 0x35),
                del_ink: Color32::from_rgb(0xe8, 0xa1, 0x92),
                del_bg: Color32::from_rgb(0x35, 0x20, 0x1d),
                ins_ink: Color32::from_rgb(0xa3, 0xd4, 0xac),
                ins_bg: Color32::from_rgb(0x1d, 0x2d, 0x21),
                accent: Color32::from_rgb(0x8f, 0xaa, 0xc0),
                hover: Color32::from_rgb(0x25, 0x27, 0x2a),
            },
        }
    }

    pub fn body(&self) -> FontId {
        FontId::new(15.5, FontFamily::Name(SERIF.into()))
    }

    pub fn ui(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }

    pub fn num(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Monospace)
    }

    pub fn del_strike(&self) -> Stroke {
        Stroke::new(1.0, self.del_ink.gamma_multiply(0.55))
    }

    /// Push the palette into egui's style so stock widgets match the paper.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = match self.mode {
            Mode::Light => egui::Visuals::light(),
            Mode::Dark => egui::Visuals::dark(),
        };
        v.panel_fill = self.paper;
        v.window_fill = self.paper;
        v.extreme_bg_color = self.rail;
        v.faint_bg_color = self.hover;
        v.override_text_color = Some(self.ink);
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.accent.gamma_multiply(0.30);
        v.selection.stroke = Stroke::new(1.0, self.accent);

        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = egui::CornerRadius::same(4);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.rule);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.dim);
        v.widgets.inactive.bg_fill = self.rail;
        v.widgets.inactive.weak_bg_fill = self.rail;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, self.ink);
        v.widgets.hovered.bg_fill = self.hover;
        v.widgets.hovered.weak_bg_fill = self.hover;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, self.rule);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, self.ink);
        v.widgets.active.bg_fill = self.rule;
        v.widgets.active.weak_bg_fill = self.rule;
        v.widgets.active.fg_stroke = Stroke::new(1.0, self.ink);
        v.window_stroke = Stroke::new(1.0, self.rule);
        v.window_corner_radius = egui::CornerRadius::same(6);
        ctx.set_visuals(v.clone());

        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.interact_size.y = 26.0;
            style.visuals = v.clone();
        });
    }
}

/// Load the CJK faces. Returns whether the serif came from the real file —
/// if it did not, the UI still works but falls back to the sans face, and
/// saying so is better than a silently different document.
pub fn install_fonts(ctx: &egui::Context) -> (bool, bool) {
    let mut defs = FontDefinitions::default();

    let mut load = |path: &str, index: u32, weight: Option<f32>, key: &str| -> bool {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut data = FontData::from_owned(bytes);
                data.index = index;
                if let Some(w) = weight {
                    data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", w)]);
                }
                defs.font_data.insert(key.to_owned(), std::sync::Arc::new(data));
                true
            }
            Err(_) => false,
        }
    };

    let sans = load(FONT_SANS, SC_FACE, Some(WEIGHT_REGULAR), "cjk-sans");
    let serif = load(FONT_SERIF, SC_FACE, Some(WEIGHT_REGULAR), "cjk-serif");
    let mono = load(FONT_MONO, 0, None, "mono");

    if sans {
        // Appended, not prepended: egui's own font has better Latin metrics for
        // UI chrome, and CJK only needs to be reachable as a fallback.
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .push("cjk-sans".to_owned());
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .push("cjk-sans".to_owned());
    }
    if mono {
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "mono".to_owned());
    }
    if serif {
        // The serif family is only ever asked for by name, so it can be the
        // document face alone — that is what makes a contract look printed.
        defs.families.insert(
            FontFamily::Name(SERIF.into()),
            vec!["cjk-serif".to_owned(), "cjk-sans".to_owned()],
        );
    } else if sans {
        defs.families
            .insert(FontFamily::Name(SERIF.into()), vec!["cjk-sans".to_owned()]);
    }

    ctx.set_fonts(defs);
    (sans, serif)
}
