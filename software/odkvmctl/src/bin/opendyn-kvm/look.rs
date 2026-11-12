//! The window's look, after the Creative App: a dark (or light) sidebar with coloured page icons and quick controls at
//! its foot, and the page's content in rounded cards. The accent colour is the active PC's.
//!
//! Text is set in Adwaita Sans at regular weight (`fonts/make_ui_font.py`): egui's own Ubuntu Light is too thin to
//! read as dark text on a light background. Chinese is set in Noto Sans CJK SC (`fonts/make_cjk_font.py`), which
//! follows it. Coloured text goes through [`ink`], which keeps it readable on a card.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Stroke, Theme, ThemePreference, Ui, Visuals};
use kvm_proto::ctrl::Rgb;
use odkvmctl::local;

/// Accent while no KVM is connected.
pub const NEUTRAL: Color32 = Color32::from_rgb(70, 130, 220);
pub const GOOD: Color32 = Color32::from_rgb(46, 174, 96);
pub const WARN: Color32 = Color32::from_rgb(220, 150, 20);
pub const BAD: Color32 = Color32::from_rgb(220, 64, 64);

pub fn color32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

pub fn preference(t: local::Theme) -> ThemePreference {
    match t {
        local::Theme::System => ThemePreference::System,
        local::Theme::Dark => ThemePreference::Dark,
        local::Theme::Light => ThemePreference::Light,
    }
}

/// Light theme: body and button text, and the grey of notes; about the contrast of an editor's light theme.
const TEXT_LIGHT: Color32 = Color32::from_rgb(36, 40, 46);
const WEAK_LIGHT: Color32 = Color32::from_rgb(84, 91, 102);

/// The text font (`fonts/AdwaitaSans-UI.ttf`, SIL OFL 1.1, `fonts/OFL.txt`): Latin text at regular weight.
const UI_FONT: &[u8] = include_bytes!("../../../fonts/AdwaitaSans-UI.ttf");
const UI_FONT_NAME: &str = "AdwaitaSans-UI";
/// The Chinese text font (`fonts/NotoSansSC-UI.otf`, SIL OFL 1.1, `fonts/OFL-NotoSansCJK.txt`): the characters the
/// Chinese translation uses, at regular weight. egui's own fonts have no Chinese.
const CJK_FONT: &[u8] = include_bytes!("../../../fonts/NotoSansSC-UI.otf");
const CJK_FONT_NAME: &str = "NotoSansSC-UI";

/// egui's fonts with the text fonts ahead of them, Latin first, then Chinese: egui's still draw the symbols, emoji and
/// scripts these haven't. Monospace text (the diagnostics' copy) finds Chinese after egui's monospace font.
fn fonts() -> egui::FontDefinitions {
    let mut f = egui::FontDefinitions::default();
    f.font_data.insert(UI_FONT_NAME.to_owned(), Arc::new(egui::FontData::from_static(UI_FONT)));
    f.font_data.insert(CJK_FONT_NAME.to_owned(), Arc::new(egui::FontData::from_static(CJK_FONT)));
    let proportional = f.families.entry(egui::FontFamily::Proportional).or_default();
    proportional.insert(0, UI_FONT_NAME.to_owned());
    proportional.insert(1, CJK_FONT_NAME.to_owned());
    f.families.entry(egui::FontFamily::Monospace).or_default().push(CJK_FONT_NAME.to_owned());
    f
}

/// Colours that egui's visuals don't carry.
pub struct Palette {
    pub sidebar: Color32,
    pub card: Color32,
    pub card_stroke: Color32,
    pub weak: Color32,
}

pub fn palette(ui: &Ui) -> Palette {
    if ui.visuals().dark_mode {
        Palette {
            sidebar: Color32::from_rgb(16, 18, 21),
            card: Color32::from_rgb(33, 36, 41),
            card_stroke: Color32::from_rgb(46, 50, 56),
            weak: Color32::from_rgb(150, 156, 166),
        }
    } else {
        Palette {
            sidebar: Color32::from_rgb(232, 234, 238),
            card: Color32::WHITE,
            card_stroke: Color32::from_rgb(220, 224, 230),
            weak: WEAK_LIGHT,
        }
    }
}

fn visuals(theme: Theme, accent: Color32) -> Visuals {
    let mut v = match theme {
        Theme::Dark => Visuals::dark(),
        Theme::Light => Visuals::light(),
    };
    let dark = theme == Theme::Dark;
    if !dark {
        // egui's light text is mid grey (80 for labels, 60 on buttons).
        v.widgets.noninteractive.fg_stroke.color = TEXT_LIGHT;
        v.widgets.inactive.fg_stroke.color = TEXT_LIGHT;
        v.weak_text_color = Some(WEAK_LIGHT);
    }
    v.panel_fill = if dark { Color32::from_rgb(23, 25, 29) } else { Color32::from_rgb(244, 245, 247) };
    v.window_fill = v.panel_fill;
    v.extreme_bg_color = if dark { Color32::from_rgb(14, 15, 18) } else { Color32::from_rgb(250, 251, 252) };
    // Text fields stand out from the cards (white in light mode).
    v.text_edit_bg_color = Some(if dark { v.extreme_bg_color } else { Color32::from_rgb(236, 239, 243) });
    v.faint_bg_color = if dark { Color32::from_rgb(30, 33, 38) } else { Color32::from_rgb(238, 240, 243) };
    v.selection.bg_fill = accent.gamma_multiply(if dark { 0.55 } else { 0.35 });
    // The text on a selected button: the accent on its own tint is hard to read.
    v.selection.stroke = Stroke::new(1.0, if dark { Color32::WHITE } else { Color32::from_rgb(20, 24, 30) });
    v.hyperlink_color = accent;
    let radius = CornerRadius::same(8);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    v.widgets.inactive.weak_bg_fill =
        if dark { Color32::from_rgb(42, 46, 52) } else { Color32::from_rgb(228, 231, 236) };
    v.widgets.inactive.bg_fill = v.widgets.inactive.weak_bg_fill;
    if !dark {
        // Check and radio boxes, slider rails: the buttons' grey all but vanishes on a white card.
        v.widgets.inactive.bg_fill = Color32::from_rgb(204, 209, 217);
        v.widgets.hovered.bg_fill = Color32::from_rgb(190, 196, 206);
    }
    v.widgets.hovered.weak_bg_fill = accent.gamma_multiply(0.35);
    v.widgets.active.weak_bg_fill = accent.gamma_multiply(0.55);
    v.window_corner_radius = CornerRadius::same(12);
    v.slider_trailing_fill = true;
    v
}

/// Sets the text font, and both themes' visuals and spacing for `accent` (the active PC's colour).
pub fn apply(ctx: &egui::Context, accent: Color32) {
    // A no-op once the context has them (egui compares the definitions).
    ctx.set_fonts(fonts());
    for theme in [Theme::Dark, Theme::Light] {
        ctx.set_visuals_of(theme, visuals(theme, accent));
        ctx.style_mut_of(theme, |s| {
            s.spacing.item_spacing = egui::vec2(8.0, 8.0);
            s.spacing.button_padding = egui::vec2(12.0, 6.0);
            s.spacing.interact_size.y = 26.0;
            s.spacing.slider_width = 160.0;
        });
    }
}

/// A rounded card with an optional title.
pub fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_stroke))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if !title.is_empty() {
                ui.label(RichText::new(title).size(15.0).strong());
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

/// Grey explanatory text.
pub fn note(ui: &mut Ui, text: impl Into<String>) {
    let weak = palette(ui).weak;
    ui.label(RichText::new(text.into()).color(weak));
}

/// `c` as the colour of text on a card: status and PC colours are bright enough to fill a shape but too faint as text
/// in the light theme (and red is in the dark one). See [`ink_on`].
pub fn ink(ui: &Ui, c: Color32) -> Color32 {
    ink_on(c, palette(ui).card)
}

/// Text contrast to reach: WCAG AA for body text.
const CONTRAST: f32 = 4.5;

/// `c` darkened (on a light `background`) or lightened (on a dark one) in small steps, just until text in it has a
/// contrast of 4.5:1 there; a colour that already has it comes back as it is.
pub fn ink_on(c: Color32, background: Color32) -> Color32 {
    let toward = if luminance(background) > 0.18 { Color32::BLACK } else { Color32::WHITE };
    (0..=20)
        .map(|step| lerp(c, toward, step as f32 / 20.0))
        .find(|&t| contrast(t, background) >= CONTRAST)
        .unwrap_or(toward)
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// WCAG relative luminance.
fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio, 1 to 21.
fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A small coloured status chip.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) {
    // On its own tint over the card.
    let ink = ink_on(color, lerp(palette(ui).card, color, 0.18));
    Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(ink).size(12.0).strong());
        });
}

/// A sidebar entry: coloured icon and label; returns true when clicked.
pub fn nav_item(ui: &mut Ui, icon: &str, icon_color: Color32, label: &str, selected: bool, accent: Color32) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 34.0), egui::Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(8), accent.gamma_multiply(0.22));
        let bar = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 7.0), egui::vec2(3.0, rect.height() - 14.0));
        painter.rect_filled(bar, CornerRadius::same(2), accent);
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(8), ui.visuals().widgets.hovered.weak_bg_fill.gamma_multiply(0.5));
    }
    let text = ui.visuals().text_color();
    painter.text(
        rect.left_center() + egui::vec2(14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(17.0),
        icon_color,
    );
    painter.text(
        rect.left_center() + egui::vec2(42.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.5),
        if selected { text } else { text.gamma_multiply(0.85) },
    );
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_reaches_text_contrast_and_leaves_readable_colours_alone() {
        let white = Color32::WHITE;
        let dark_card = Color32::from_rgb(33, 36, 41);
        // Bright fills, too faint as text on white: darkened just enough (the step before falls short).
        for c in [GOOD, WARN, BAD, NEUTRAL, Color32::from_rgb(255, 106, 0), Color32::GRAY] {
            let t = ink_on(c, white);
            assert!(contrast(t, white) >= CONTRAST, "{c:?} -> {t:?}: {}", contrast(t, white));
            assert!(luminance(t) < luminance(c), "{c:?} darkened");
            let less = (0..=20).map(|s| lerp(c, Color32::BLACK, s as f32 / 20.0)).find(|&x| x == t).is_some();
            assert!(less, "{t:?} is a step toward black");
        }
        // Already readable: unchanged, in both themes.
        assert_eq!(ink_on(TEXT_LIGHT, white), TEXT_LIGHT);
        assert_eq!(ink_on(WEAK_LIGHT, white), WEAK_LIGHT);
        assert_eq!(ink_on(GOOD, dark_card), GOOD);
        // On the dark card, a colour too dark to read is lightened instead.
        let t = ink_on(Color32::from_rgb(0, 70, 255), dark_card);
        assert!(contrast(t, dark_card) >= CONTRAST && luminance(t) > luminance(Color32::from_rgb(0, 70, 255)));
        // The light theme's text and notes on a card and on the window's background.
        for bg in [white, Color32::from_rgb(244, 245, 247)] {
            assert!(contrast(TEXT_LIGHT, bg) >= 12.0, "{}", contrast(TEXT_LIGHT, bg));
            assert!(contrast(WEAK_LIGHT, bg) >= 6.0, "{}", contrast(WEAK_LIGHT, bg));
        }
    }

    #[test]
    fn the_text_fonts_come_first_and_have_every_letter_of_every_language() {
        let defs = fonts();
        assert_eq!(defs.families[&egui::FontFamily::Proportional][..2], [UI_FONT_NAME, CJK_FONT_NAME]);
        // Only the text fonts, so a letter they lack isn't found in egui's fallbacks.
        let mut only = egui::FontDefinitions::empty();
        for name in [UI_FONT_NAME, CJK_FONT_NAME] {
            only.font_data.insert(name.to_owned(), defs.font_data[name].clone());
        }
        only.families.insert(egui::FontFamily::Proportional, vec![UI_FONT_NAME.to_owned(), CJK_FONT_NAME.to_owned()]);
        let mut f = egui::epaint::text::Fonts::new(Default::default(), only);
        let id = egui::FontId::proportional(14.0);
        for (lang, _) in odkvmctl::i18n::LANGUAGES {
            let path = format!("{}/i18n/{lang}/odkvmctl.ftl", env!("CARGO_MANIFEST_DIR"));
            let text = std::fs::read_to_string(&path).unwrap();
            let mut missing: Vec<char> = text.chars().filter(|c| !c.is_control() && !f.has_glyph(&id, *c)).collect();
            missing.dedup();
            assert!(missing.is_empty(), "{lang}: the text fonts lack {missing:?} (fonts/make_*_font.py)");
        }
        // Even digits, so a live value doesn't move the text after it.
        let mut view = f.with_pixels_per_point(1.0);
        let mut width = |s: &str| view.layout_no_wrap(s.into(), id.clone(), Color32::WHITE).size().x;
        assert_eq!(width("1111"), width("8888"));
    }
}
