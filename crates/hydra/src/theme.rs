//! Hydra's look: the colours, type and spacing of the design system in `docs/design.md`.
//!
//! Only the light theme exists. A new theme is another [`Palette`] constant passed to
//! [`apply`]; the contrast test below checks every palette listed in it.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, TextStyle, Theme, Vec2,
};
use std::sync::Arc;

/// A status colour in its three uses: a line or edge, a background, and text on that background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub strong: Color32,
    pub tint: Color32,
    pub text: Color32,
}

/// Every colour the app uses, by role. Names match the tokens in `docs/design.md`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// App frame, tab strip, toolbars.
    pub chrome: Color32,
    /// Side panels, inspector, issue list.
    pub panel: Color32,
    /// Inputs, cards, tables, dialogs.
    pub surface: Color32,
    /// Behind a hovered control.
    pub hover: Color32,
    /// The drawing paper.
    pub sheet: Color32,
    /// Around the sheet.
    pub desk: Color32,
    /// Text and drawing lines.
    pub ink: Color32,
    /// Labels, meta, hints.
    pub secondary: Color32,
    /// Card edges and dividers.
    pub border: Color32,
    /// Edges of inputs and buttons.
    pub border_strong: Color32,
    /// Primary buttons, selection, focus, links. Never a status.
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_tint: Color32,
    /// Accent-coloured text on `accent_tint`.
    pub accent_text: Color32,
    /// Text on `accent`.
    pub on_accent: Color32,
    /// The part number isn't listed; also warnings and unchecked imports.
    pub unlisted: Tone,
    /// Not in the catalogue; also errors that block a save or an approval.
    pub missing: Tone,
    /// Drawn with another valve in place of the one asked for.
    pub stand_in: Tone,
    /// Finished human decisions only: approved, checked.
    pub ok: Tone,
    /// Work done by the AI assistant.
    pub agent: Tone,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

const fn tone(strong: u32, tint: u32, text: u32) -> Tone {
    Tone {
        strong: hex(strong),
        tint: hex(tint),
        text: hex(text),
    }
}

pub const LIGHT: Palette = Palette {
    chrome: hex(0xE9ECEF),
    panel: hex(0xF4F5F7),
    surface: hex(0xFFFFFF),
    hover: hex(0xEEF1F4),
    sheet: hex(0xFFFFFF),
    desk: hex(0xCDD3DA),
    ink: hex(0x14181F),
    secondary: hex(0x4A5361),
    border: hex(0xC3C9D1),
    border_strong: hex(0x8A93A0),
    accent: hex(0x1F5FAD),
    accent_hover: hex(0x174C8C),
    accent_tint: hex(0xE3ECF7),
    accent_text: hex(0x174C8C),
    on_accent: hex(0xFFFFFF),
    unlisted: tone(0xD98A00, 0xFFF1D6, 0x8A5300),
    missing: tone(0xC4291C, 0xFDE7E5, 0xA3201A),
    stand_in: tone(0x6A3FC9, 0xEEE8FB, 0x5B33B8),
    ok: tone(0x2A8A52, 0xE2F2E8, 0x1E6B3E),
    agent: tone(0x0E7C86, 0xE0F2F3, 0x0B5F67),
};

/// Corner radius of controls, cards and chips. Panels and the sheet are square.
pub const RADIUS: u8 = 3;

/// Font families beyond egui's Proportional (Plex Sans) and Monospace (Plex Mono Medium).
pub const MEDIUM: &str = "medium";
pub const SEMIBOLD: &str = "semibold";
pub const CONDENSED: &str = "condensed";
pub const MONO_SEMIBOLD: &str = "mono-semibold";

/// Text styles beyond egui's: 16 px section headings, 13 px field labels, 12 px overlines.
/// Overlines are written in capitals by the caller.
pub fn heading() -> TextStyle {
    TextStyle::Name("heading".into())
}
pub fn label() -> TextStyle {
    TextStyle::Name("label".into())
}
pub fn overline() -> TextStyle {
    TextStyle::Name("overline".into())
}

/// Install the fonts and the palette's style, and keep the app in light mode whatever the
/// system setting is.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    ctx.set_fonts(fonts());
    ctx.set_theme(Theme::Light);
    ctx.set_style_of(Theme::Light, style(palette));
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let files: [(&str, &'static [u8]); 6] = [
        (
            "plex-sans",
            include_bytes!("../fonts/IBMPlexSans-Regular.ttf"),
        ),
        (MEDIUM, include_bytes!("../fonts/IBMPlexSans-Medium.ttf")),
        (
            SEMIBOLD,
            include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf"),
        ),
        (
            CONDENSED,
            include_bytes!("../fonts/IBMPlexSansCondensed-SemiBold.ttf"),
        ),
        (
            "plex-mono",
            include_bytes!("../fonts/IBMPlexMono-Medium.ttf"),
        ),
        (
            MONO_SEMIBOLD,
            include_bytes!("../fonts/IBMPlexMono-SemiBold.ttf"),
        ),
    ];
    for (name, bytes) in files {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // egui's own fonts stay behind Plex for the symbols Plex lacks.
    let sans = fonts.families[&FontFamily::Proportional].clone();
    let mono = fonts.families[&FontFamily::Monospace].clone();
    let with = |first: &str, rest: &[String]| {
        std::iter::once(first.to_string())
            .chain(rest.iter().cloned())
            .collect::<Vec<_>>()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with("plex-sans", &sans));
    fonts
        .families
        .insert(FontFamily::Monospace, with("plex-mono", &mono));
    for name in [MEDIUM, SEMIBOLD, CONDENSED] {
        fonts
            .families
            .insert(FontFamily::Name(name.into()), with(name, &sans));
    }
    fonts.families.insert(
        FontFamily::Name(MONO_SEMIBOLD.into()),
        with(MONO_SEMIBOLD, &mono),
    );
    fonts
}

fn style(p: &Palette) -> egui::Style {
    let mut style = egui::Style::default();
    let family = |name: &str| FontFamily::Name(name.into());
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::new(13.0, family(MEDIUM))),
        (TextStyle::Monospace, FontId::monospace(13.0)),
        (TextStyle::Heading, FontId::new(20.0, family(SEMIBOLD))),
        (heading(), FontId::new(16.0, family(SEMIBOLD))),
        (label(), FontId::new(13.0, family(MEDIUM))),
        (overline(), FontId::new(12.0, family(CONDENSED))),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 6.0);
    s.button_padding = Vec2::new(10.0, 4.0);
    s.interact_size = Vec2::new(28.0, 28.0);
    s.window_margin = Margin::same(16);
    s.menu_margin = Margin::same(6);

    let v = &mut style.visuals;
    *v = egui::Visuals::light();
    let radius = CornerRadius::same(RADIUS);
    let edge = |c| Stroke::new(1.0, c);
    v.panel_fill = p.panel;
    v.window_fill = p.surface;
    v.window_stroke = edge(p.border);
    v.window_corner_radius = radius;
    v.menu_corner_radius = radius;
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(40),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(36),
    };
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.panel;
    v.code_bg_color = p.chrome;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.unlisted.text;
    v.error_fg_color = p.missing.text;
    v.weak_text_color = Some(p.secondary);
    v.selection.bg_fill = p.accent_tint;
    v.selection.stroke = edge(p.accent_text);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = edge(p.border);
    w.noninteractive.fg_stroke = edge(p.ink);
    w.inactive.bg_fill = p.surface;
    w.inactive.weak_bg_fill = p.surface;
    w.inactive.bg_stroke = edge(p.border_strong);
    w.inactive.fg_stroke = edge(p.ink);
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = edge(p.secondary);
    w.hovered.fg_stroke = edge(p.ink);
    w.active.bg_fill = p.accent_tint;
    w.active.weak_bg_fill = p.accent_tint;
    w.active.bg_stroke = edge(p.accent);
    w.active.fg_stroke = edge(p.accent_text);
    w.open.bg_fill = p.chrome;
    w.open.weak_bg_fill = p.chrome;
    w.open.bg_stroke = edge(p.border_strong);
    w.open.fg_stroke = edge(p.ink);
    for state in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        state.corner_radius = radius;
        // Sturdy, not springy: controls don't grow under the pointer.
        state.expansion = 0.0;
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every palette, so a new theme is checked as soon as it is added.
    const PALETTES: [(&str, Palette); 1] = [("light", LIGHT)];

    /// WCAG relative luminance.
    fn luminance(c: Color32) -> f32 {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn contrast_is_measured_like_wcag() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(hex(0x777777), Color32::WHITE) - 4.48).abs() < 0.01);
    }

    #[test]
    fn every_text_colour_reads_on_its_background() {
        for (name, p) in PALETTES {
            let mut pairs = vec![("on accent", p.on_accent, p.accent)];
            for bg in [p.chrome, p.panel, p.surface, p.hover] {
                pairs.push(("ink", p.ink, bg));
                pairs.push(("secondary", p.secondary, bg));
                pairs.push(("accent", p.accent, bg));
            }
            pairs.push(("accent text", p.accent_text, p.accent_tint));
            for (tone_name, t) in [
                ("unlisted", p.unlisted),
                ("missing", p.missing),
                ("stand-in", p.stand_in),
                ("ok", p.ok),
                ("agent", p.agent),
            ] {
                pairs.push((tone_name, t.text, t.tint));
                // Issue lists and the warn/error colours put tone text straight on a panel.
                pairs.push((tone_name, t.text, p.panel));
                pairs.push((tone_name, t.text, p.surface));
            }
            for (what, fg, bg) in pairs {
                let c = contrast(fg, bg);
                assert!(c >= 4.5, "{name}: {what} {fg:?} on {bg:?} is {c:.2}:1");
            }
            // Amber and red are told apart by lightness too, not hue alone.
            let gap = contrast(p.unlisted.strong, p.missing.strong);
            assert!(
                gap >= 1.4,
                "{name}: amber and red differ by only {gap:.2}:1"
            );
        }
    }

    #[test]
    fn apply_installs_plex_and_the_light_style() {
        let ctx = egui::Context::default();
        apply(&ctx, &LIGHT);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let style = ui.style();
            assert!(!style.visuals.dark_mode);
            assert_eq!(style.visuals.panel_fill, LIGHT.panel);
            assert_eq!(style.visuals.error_fg_color, LIGHT.missing.text);
            assert_eq!(style.text_styles[&TextStyle::Body].size, 14.0);
            for s in [heading(), label(), overline()] {
                assert!(
                    style.text_styles.contains_key(&s),
                    "{s:?} is not registered"
                );
            }
        });
        out.textures_delta.clear();
        let fonts = fonts();
        assert_eq!(fonts.families[&FontFamily::Proportional][0], "plex-sans");
        assert_eq!(fonts.families[&FontFamily::Monospace][0], "plex-mono");
        // Running a frame loaded every bundled file (epaint panics on a bad one); the symbols
        // the design types as text render in every family, from Plex or the fallbacks.
        let families = [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name(MEDIUM.into()),
            FontFamily::Name(SEMIBOLD.into()),
            FontFamily::Name(CONDENSED.into()),
            FontFamily::Name(MONO_SEMIBOLD.into()),
        ];
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().fonts_mut(|f| {
                for family in &families {
                    let id = FontId::new(14.0, family.clone());
                    // Status marks (▲ ■ ◆ ● ✕) are painted: Plex and egui's fonts lack some of them.
                    assert!(f.has_glyphs(&id, "Hydra 210 bar → ✓ ⇄"), "{family:?}");
                }
            });
        });
        out.textures_delta.clear();
    }
}
