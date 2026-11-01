//! Hydra's look: the colours, type and spacing of the design system in `docs/design.md`.
//!
//! Only the light theme exists. A new theme is another [`Palette`] constant passed to
//! [`apply`]; the contrast test below checks every palette listed in it.

use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Frame, Label, Layout, Margin, Painter, Rect, Response, RichText, Sense, Shadow, Shape, Stroke,
    StrokeKind, TextStyle, Theme, Ui, Vec2, Widget, WidgetInfo, WidgetText, WidgetType,
    text::{LayoutJob, TextFormat},
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
    /// Sheet grid lines every 5 mm and every 25 mm.
    pub grid: Color32,
    pub grid_major: Color32,
    /// Text and drawing lines.
    pub ink: Color32,
    /// Labels, meta, hints.
    pub secondary: Color32,
    /// Card edges and panel dividers.
    pub border: Color32,
    /// Lines between rows of a list or table.
    pub divider: Color32,
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
    grid: hex(0xE4E8ED),
    grid_major: hex(0xCFD5DC),
    ink: hex(0x14181F),
    secondary: hex(0x4A5361),
    border: hex(0xC3C9D1),
    divider: hex(0xDDE1E6),
    border_strong: hex(0x7F8895),
    accent: hex(0x1F5FAD),
    accent_hover: hex(0x174C8C),
    accent_tint: hex(0xE3ECF7),
    accent_text: hex(0x174C8C),
    on_accent: hex(0xFFFFFF),
    unlisted: tone(0xC07A00, 0xFFF1D6, 0x8A5300),
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

/// A status mark: ▲ ■ ⇄ ✓ ◆ ● ✕. Painted, because Plex and egui's fonts lack some of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// ▲ part not listed, or a warning.
    Warning,
    /// ■ missing, or an error.
    Error,
    /// ⇄ stand-in.
    StandIn,
    /// ✓ approved or checked.
    Check,
    /// ◆ the assistant's work.
    Agent,
    /// ● waiting for a person's decision.
    Waiting,
    /// ✕ rejected.
    Rejected,
}

impl Mark {
    /// The tone a chip or banner with this mark is drawn in. Waiting takes the accent (a
    /// decision still to make) and rejected is grey: neither is a catalogue status.
    pub fn tone(self) -> Tone {
        let p = &LIGHT;
        match self {
            Mark::Warning => p.unlisted,
            Mark::Error => p.missing,
            Mark::StandIn => p.stand_in,
            Mark::Check => p.ok,
            Mark::Agent => p.agent,
            Mark::Waiting => Tone {
                strong: p.accent,
                tint: p.accent_tint,
                text: p.accent_text,
            },
            Mark::Rejected => Tone {
                strong: p.border_strong,
                tint: p.panel,
                text: p.secondary,
            },
        }
    }

    /// Paint the mark in `color`, centred in the largest square that fits in `rect`.
    pub fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let c = rect.center();
        let s = rect.width().min(rect.height()) / 2.0;
        // Points on a square of side 2 round the centre, y down.
        let at = |x: f32, y: f32| c + Vec2::new(x, y) * s;
        let line = Stroke::new((s * 0.35).max(1.5), color);
        let fill = |points| Shape::convex_polygon(points, color, Stroke::NONE);
        match self {
            Mark::Warning => {
                painter.add(fill(vec![at(0.0, -0.9), at(1.0, 0.85), at(-1.0, 0.85)]));
            }
            Mark::Error => {
                painter.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 1.5)), 0.0, color);
            }
            Mark::StandIn => {
                painter.arrow(at(-1.0, -0.45), Vec2::new(2.0 * s, 0.0), line);
                painter.arrow(at(1.0, 0.45), Vec2::new(-2.0 * s, 0.0), line);
            }
            Mark::Check => {
                painter.line(vec![at(-0.85, 0.0), at(-0.3, 0.6), at(0.85, -0.65)], line);
            }
            Mark::Agent => {
                painter.add(fill(vec![
                    at(0.0, -1.0),
                    at(0.8, 0.0),
                    at(0.0, 1.0),
                    at(-0.8, 0.0),
                ]));
            }
            Mark::Waiting => {
                painter.circle_filled(c, s * 0.75, color);
            }
            Mark::Rejected => {
                painter.line_segment([at(-0.7, -0.7), at(0.7, 0.7)], line);
                painter.line_segment([at(0.7, -0.7), at(-0.7, 0.7)], line);
            }
        }
    }
}

/// The mark, then `text`, both in the mark's tone and the mark as tall as the text: a problem
/// line, a "No symbol" note. Always left to right, whatever the layout around it.
pub fn marked(ui: &mut Ui, mark: Mark, text: impl Into<RichText>) -> Response {
    let color = mark.tone().text;
    let text = text.into().color(color);
    let side = ui.fonts_mut(|f| text.font_height(f, ui.style()));
    let gap = ui.spacing().item_spacing.x;
    // Laid out first, so the pair takes only its own width and right-aligns when asked to.
    let room = (ui.available_size_before_wrap().x - side - gap).max(0.0);
    let text = WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        room,
        TextStyle::Body,
    );
    let row = Vec2::new(side + gap + text.size().x, side);
    ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
        mark.paint(ui.painter(), rect.shrink(side * 0.2), color);
        ui.label(text)
    })
    .inner
}

/// A status chip: the mark and the word in the mark's tone, on its tint with a strong edge,
/// 24 px high. Screen readers and kittests read the word as a label.
pub fn chip(ui: &mut Ui, mark: Mark, word: &str) -> Response {
    let tone = mark.tone();
    let size = TextStyle::Small.resolve(ui.style()).size;
    let font = FontId::new(size, FontFamily::Name(SEMIBOLD.into()));
    let galley = ui.painter().layout_no_wrap(word.into(), font, tone.text);
    let (pad, side, gap) = (8.0, 10.0, 6.0);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(pad + side + gap + galley.size().x + pad, 24.0),
        Sense::hover(),
    );
    let enabled = ui.is_enabled();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, enabled, word));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let edge = Stroke::new(1.0, tone.strong);
        painter.rect(rect, RADIUS, tone.tint, edge, StrokeKind::Inside);
        let mark_at = Rect::from_center_size(
            rect.left_center() + Vec2::new(pad + side / 2.0, 0.0),
            Vec2::splat(side),
        );
        mark.paint(painter, mark_at, tone.text);
        let text_at = rect.left_center() + Vec2::new(pad + side + gap, -galley.size().y / 2.0);
        painter.galley(text_at, galley, tone.text);
    }
    response
}

/// The primary button: filled accent, for the one action in a view that commits. 32 px high.
pub fn primary(text: impl Into<String>) -> impl Widget {
    let p = &LIGHT;
    button(
        text.into(),
        [p.accent, p.accent_hover],
        [p.accent_hover; 2],
        p.on_accent,
    )
}

/// The danger button: outlined in the missing tone, never filled, for reject, delete and
/// discard. 32 px high.
pub fn danger(text: impl Into<String>) -> impl Widget {
    let t = LIGHT.missing;
    button(
        text.into(),
        [LIGHT.surface, t.tint],
        [t.text, t.strong],
        t.text,
    )
}

/// A 32 px button with a semibold label in `text_color`, its fill and edge at rest and under
/// the pointer (or pressed), and a 2 px accent ring while it has keyboard focus.
fn button(
    text: String,
    fill: [Color32; 2],
    edge: [Color32; 2],
    text_color: Color32,
) -> impl Widget {
    move |ui: &mut Ui| {
        ui.scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            for (state, i) in [
                (&mut w.inactive, 0),
                (&mut w.hovered, 1),
                (&mut w.active, 1),
            ] {
                state.weak_bg_fill = fill[i];
                state.bg_fill = fill[i];
                state.bg_stroke = Stroke::new(1.0, edge[i]);
                state.fg_stroke = Stroke::new(1.0, text_color);
            }
            let label = RichText::new(text).family(FontFamily::Name(SEMIBOLD.into()));
            let response = ui.add(Button::new(label).min_size(Vec2::new(0.0, 32.0)));
            if response.has_focus() {
                let ring = Stroke::new(2.0, LIGHT.accent);
                let rect = response.rect.expand(1.0);
                ui.painter()
                    .rect_stroke(rect, RADIUS + 1, ring, StrokeKind::Outside);
            }
            response
        })
        .inner
    }
}

/// A full-width banner in the mark's tone: the mark, the bold `first` sentence, then `rest`, and
/// at the right whatever `action` adds: the board allows one button, or nothing.
pub fn banner<R>(
    ui: &mut Ui,
    mark: Mark,
    first: &str,
    rest: &str,
    action: impl FnOnce(&mut Ui) -> R,
) -> R {
    let tone = mark.tone();
    Frame::new()
        .fill(tone.tint)
        .stroke(Stroke::new(1.0, tone.strong))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let body = TextStyle::Body.resolve(ui.style());
            let bold = FontId::new(body.size, FontFamily::Name(SEMIBOLD.into()));
            let mut job = LayoutJob::default();
            job.append(first, 0.0, TextFormat::simple(bold, LIGHT.ink));
            job.append(
                &format!(" {rest}"),
                0.0,
                TextFormat::simple(body, LIGHT.ink),
            );
            // The action first, from the right, so the text wraps in the room left of it.
            let row = Vec2::new(ui.available_width(), 0.0);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Min), |ui| {
                let inner = action(ui);
                ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                    let side = ui.text_style_height(&TextStyle::Body);
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
                    mark.paint(ui.painter(), rect.shrink(side * 0.15), tone.text);
                    ui.add(Label::new(job).wrap());
                });
                inner
            })
            .inner
        })
        .inner
}

/// Controls joined edge to edge in one `border_strong` frame, square inside and split by 1 px
/// `border` lines: a toolbar group or a segmented control. Give a segment
/// `Button::selected` to show which one is on.
pub fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        // Shows through the 1 px gaps between the controls as their dividers.
        .fill(LIGHT.border)
        .stroke(Stroke::new(1.0, LIGHT.border_strong))
        .corner_radius(RADIUS)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            let w = &mut ui.visuals_mut().widgets;
            for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                state.corner_radius = CornerRadius::ZERO;
                state.bg_stroke = Stroke::NONE;
            }
            ui.horizontal(add).inner
        })
        .inner
}

/// A strip of a side panel, as wide as the panel: `add` in 12 × 10 px padding over a `border`
/// line. A panel's header, or a section of the inspector.
pub fn section<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let strip = Frame::new()
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    let rect = strip.response.rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, LIGHT.border),
    );
    strip.inner
}

/// A clickable list row: `add` in 12 × 8 px padding over a `divider` line. The selected row is
/// `accent_tint` with a 1 px `accent` edge all round, and a focused row gets the edge. Screen
/// readers and kittests read it as a selectable `name`.
pub fn list_row(ui: &mut Ui, selected: bool, name: &str, add: impl FnOnce(&mut Ui)) -> Response {
    let row = ui
        .scope_builder(egui::UiBuilder::new().sense(Sense::click()), |ui| {
            // Clicks on the row's text select the row.
            ui.style_mut().interaction.selectable_labels = false;
            let last = ui.response();
            let fill = if selected {
                LIGHT.accent_tint
            } else if last.hovered() {
                LIGHT.hover
            } else {
                Color32::TRANSPARENT
            };
            let edge = if selected || last.has_focus() {
                LIGHT.accent
            } else {
                Color32::TRANSPARENT
            };
            let row = Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0, edge))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui);
                });
            let rect = row.response.rect;
            ui.painter().hline(
                rect.x_range(),
                rect.bottom(),
                Stroke::new(1.0, LIGHT.divider),
            );
        })
        .response;
    let enabled = ui.is_enabled();
    row.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, enabled, selected, name));
    row
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
    // Also egui's strong text colour: bold labels stay ink, not accent.
    w.active.fg_stroke = edge(p.ink);
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

/// A kittest harness in the app's real fonts, type sizes and colours, so tests see the layout
/// engineers see and the theme's named text styles exist.
#[cfg(test)]
pub fn themed<S>(mut h: egui_kittest::Harness<'_, S>) -> egui_kittest::Harness<'_, S> {
    apply(&h.ctx, &LIGHT);
    // Keep what kittest turns off for repeatable runs, which `apply`'s style turned back on.
    h.ctx.all_styles_mut(|s| {
        s.animation_time = 0.0;
        s.scroll_animation = egui::style::ScrollAnimation::none();
        s.visuals.text_cursor.blink = false;
    });
    // The fonts are installed at the start of the next frame.
    h.step();
    h
}

/// A themed kittest harness of `size` around `app`. kittest draws a first frame before the
/// theme can be installed, so `app` only runs once it is: the theme's text styles exist.
#[cfg(test)]
pub fn harness<'a, S>(
    size: Vec2,
    mut app: impl FnMut(&mut Ui, &mut S) + 'a,
    state: S,
) -> egui_kittest::Harness<'a, S> {
    themed(
        egui_kittest::Harness::builder()
            .with_size(size)
            .build_ui_state(
                move |ui, state| {
                    if ui.style().text_styles.contains_key(&label()) {
                        app(ui, state);
                    }
                },
                state,
            ),
    )
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
            // A selected row and a changed table row carry ordinary text.
            for bg in [p.accent_tint, p.unlisted.tint] {
                pairs.push(("ink", p.ink, bg));
                pairs.push(("secondary", p.secondary, bg));
            }
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
            // Edges and marks that must be seen, not read: 3:1 (WCAG non-text contrast).
            let mut edges = vec![];
            for bg in [p.chrome, p.panel, p.surface] {
                edges.push(("border strong", p.border_strong, bg));
            }
            for t in [p.unlisted, p.missing, p.stand_in, p.ok, p.agent] {
                edges.push(("status edge", t.strong, p.sheet));
                edges.push(("status edge", t.strong, p.panel));
            }
            edges.push(("accent", p.accent, p.sheet));
            for (what, fg, bg) in edges {
                let c = contrast(fg, bg);
                assert!(c >= 3.0, "{name}: {what} {fg:?} on {bg:?} is {c:.2}:1");
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
        // As on a Mac in dark mode: the app still comes up light.
        let input = || egui::RawInput {
            system_theme: Some(Theme::Dark),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input(), |ui| {
            let style = ui.style();
            assert!(!style.visuals.dark_mode);
            assert_eq!(style.visuals.panel_fill, LIGHT.panel);
            assert_eq!(style.visuals.error_fg_color, LIGHT.missing.text);
            assert_eq!(style.visuals.strong_text_color(), LIGHT.ink);
            assert_eq!(style.text_styles[&TextStyle::Body].size, 14.0);
            for s in [heading(), label(), overline()] {
                assert!(
                    style.text_styles.contains_key(&s),
                    "{s:?} is not registered"
                );
            }
        });
        out.textures_delta.clear();
        // Running a frame loaded every bundled file: epaint panics on a bad one.
        let mut out = ctx.run_ui(input(), |ui| {
            let installed = ui.ctx().fonts(|f| f.definitions().families.clone());
            let named = |n: &str| FontFamily::Name(n.into());
            for (family, first) in [
                (FontFamily::Proportional, "plex-sans"),
                (FontFamily::Monospace, "plex-mono"),
                (named(MEDIUM), MEDIUM),
                (named(SEMIBOLD), SEMIBOLD),
                (named(CONDENSED), CONDENSED),
                (named(MONO_SEMIBOLD), MONO_SEMIBOLD),
            ] {
                let fonts = &installed[&family];
                assert_eq!(fonts[0], first, "{family:?} doesn't start with Plex");
                // egui's fonts follow, for what Plex lacks.
                assert!(fonts.len() > 1, "{family:?} has no fallback");
            }
            ui.ctx().fonts_mut(|f| {
                for family in installed.keys() {
                    let id = FontId::new(14.0, family.clone());
                    // Status marks (▲ ■ ◆ ● ✕) are painted: the fonts lack some of them.
                    assert!(f.has_glyphs(&id, "Hydra 210 bar → ✓ ⇄"), "{family:?}");
                }
                // ■ isn't in Plex: it comes from a fallback behind it.
                assert!(f.has_glyphs(&FontId::proportional(14.0), "■"));
            });
        });
        out.textures_delta.clear();
    }

    const MARKS: [Mark; 7] = [
        Mark::Warning,
        Mark::Error,
        Mark::StandIn,
        Mark::Check,
        Mark::Agent,
        Mark::Waiting,
        Mark::Rejected,
    ];

    /// The shapes `paint` draws in a frame of the themed app.
    fn shapes(mut paint: impl FnMut(&mut Ui)) -> Vec<Shape> {
        let ctx = egui::Context::default();
        apply(&ctx, &LIGHT);
        // The fonts are installed at the start of the next frame.
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        let mut out = ctx.run_ui(Default::default(), |ui| paint(ui));
        out.textures_delta.clear();
        out.shapes.into_iter().map(|s| s.shape).collect()
    }

    fn rects(shapes: &[Shape]) -> Vec<&egui::epaint::RectShape> {
        shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Rect(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_mark_paints_inside_its_rect() {
        let rect = Rect::from_min_size(egui::pos2(100.0, 100.0), Vec2::splat(16.0));
        for mark in MARKS {
            let painted = shapes(|ui| mark.paint(ui.painter(), rect, LIGHT.ink));
            assert!(!painted.is_empty(), "{mark:?} paints nothing");
            for shape in &painted {
                let bounds = shape.visual_bounding_rect();
                assert!(bounds.area() > 0.0, "{mark:?} paints an empty shape");
                // Lines may reach past by half their width.
                assert!(
                    rect.expand(3.0).contains_rect(bounds),
                    "{mark:?}: {bounds:?}"
                );
            }
        }
        // Marks are told apart by shape, not colour: no two paint the same.
        let all: Vec<_> = MARKS
            .iter()
            .map(|m| format!("{:?}", shapes(|ui| m.paint(ui.painter(), rect, LIGHT.ink))))
            .collect();
        for (i, a) in all.iter().enumerate() {
            assert!(
                !all[i + 1..].contains(a),
                "{:?} looks like another mark",
                MARKS[i]
            );
        }
    }

    #[test]
    fn chips_and_banners_take_their_marks_tone() {
        assert_eq!(Mark::Warning.tone(), LIGHT.unlisted);
        assert_eq!(Mark::Error.tone(), LIGHT.missing);
        assert_eq!(Mark::Check.tone(), LIGHT.ok);
        assert_eq!(Mark::Agent.tone(), LIGHT.agent);
        assert_eq!(Mark::Waiting.tone().strong, LIGHT.accent);
        assert_eq!(Mark::Rejected.tone().text, LIGHT.secondary);
        for mark in MARKS {
            let tone = mark.tone();
            let chip = shapes(|ui| {
                chip(ui, mark, "Word");
            });
            let frame = rects(&chip)
                .into_iter()
                .find(|r| r.fill == tone.tint)
                .unwrap_or_else(|| panic!("{mark:?}: no chip on its tint"));
            assert_eq!(frame.stroke.color, tone.strong, "{mark:?}");
            assert_eq!(frame.rect.height(), 24.0, "{mark:?}");
            let banner = shapes(|ui| {
                banner(ui, mark, "First.", "Rest.", |_| ());
            });
            assert!(
                rects(&banner)
                    .iter()
                    .any(|r| r.fill == tone.tint && r.stroke.color == tone.strong),
                "{mark:?}: no banner on its tint"
            );
        }
    }

    #[test]
    fn a_banner_holds_its_action_beside_the_text() {
        let mut button = Rect::NOTHING;
        let mut text_right = 0.0;
        shapes(|ui| {
            banner(ui, Mark::Warning, "Imported.", "Check it.", |ui| {
                button = ui.button("Mark checked").rect;
            });
        });
        let painted = shapes(|ui| {
            banner(ui, Mark::Warning, "Imported.", "Check it.", |_| ());
        });
        for shape in &painted {
            if let Shape::Text(t) = shape {
                text_right = t.visual_bounding_rect().right();
            }
        }
        assert!(button.is_positive(), "the action is laid out");
        assert!(text_right < button.left(), "{text_right} vs {button:?}");
    }

    #[test]
    fn groups_join_their_controls_in_one_strong_frame() {
        let mut buttons = vec![];
        let painted = shapes(|ui| {
            group(ui, |ui| {
                buttons.push(ui.button("New").rect);
                buttons.push(ui.button("Open").rect);
            });
        });
        let frame = rects(&painted)
            .into_iter()
            .find(|r| r.stroke == Stroke::new(1.0, LIGHT.border_strong))
            .expect("a frame with a strong edge");
        assert_eq!(frame.fill, LIGHT.border, "the dividers show through");
        // Joined: a 1 px divider between them, and square inside.
        assert_eq!(buttons[1].left() - buttons[0].right(), 1.0);
        let inner = rects(&painted)
            .into_iter()
            .filter(|r| r.fill == LIGHT.surface)
            .collect::<Vec<_>>();
        assert_eq!(inner.len(), 2);
        assert!(inner.iter().all(|r| r.corner_radius == CornerRadius::ZERO));
        assert!(inner.iter().all(|r| r.stroke.is_empty()));
    }

    #[test]
    fn sections_and_list_rows_draw_their_lines_and_the_selected_row_its_tint() {
        let lines = |shapes: &[Shape]| {
            shapes
                .iter()
                .filter_map(|s| match s {
                    Shape::LineSegment { stroke, .. } => Some(stroke.color),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let painted = shapes(|ui| {
            section(ui, |ui| ui.label("PARTS"));
        });
        assert_eq!(lines(&painted), [LIGHT.border]);
        for selected in [false, true] {
            let painted = shapes(|ui| {
                list_row(ui, selected, "V1", |ui| {
                    ui.label("V1");
                });
            });
            assert_eq!(lines(&painted), [LIGHT.divider]);
            let tinted = rects(&painted)
                .into_iter()
                .any(|r| r.fill == LIGHT.accent_tint && r.stroke == Stroke::new(1.0, LIGHT.accent));
            assert_eq!(tinted, selected, "selected: {selected}");
        }
    }

    /// The shapes of a button after a few frames, with the pointer over it or keyboard focus.
    fn button_shapes<W: Widget>(make: impl Fn() -> W, hover: bool, focus: bool) -> Vec<Shape> {
        let ctx = egui::Context::default();
        apply(&ctx, &LIGHT);
        let mut rect = Rect::NOTHING;
        let mut shapes = vec![];
        for frame in 0..4 {
            let mut input = egui::RawInput::default();
            if hover && frame > 0 {
                input.events.push(egui::Event::PointerMoved(rect.center()));
            }
            let mut out = ctx.run_ui(input, |ui| {
                let response = ui.add(make());
                if focus && frame == 1 {
                    response.request_focus();
                }
                rect = response.rect;
            });
            out.textures_delta.clear();
            shapes = out.shapes.into_iter().map(|s| s.shape).collect();
        }
        shapes
    }

    #[test]
    fn primary_is_filled_and_danger_outlined() {
        let fills = |shapes: &[Shape]| rects(shapes).iter().map(|r| r.fill).collect::<Vec<_>>();
        let primary_ = || primary("Approve");
        let rest = button_shapes(primary_, false, false);
        assert!(fills(&rest).contains(&LIGHT.accent));
        assert!(rects(&rest).iter().all(|r| r.rect.height() >= 32.0));
        // Under the pointer it darkens, so it answers to hover.
        assert!(fills(&button_shapes(primary_, true, false)).contains(&LIGHT.accent_hover));

        let danger_ = || danger("Reject");
        let rest = button_shapes(danger_, false, false);
        let edge = rects(&rest)
            .into_iter()
            .find(|r| r.stroke.color == LIGHT.missing.text)
            .expect("an outline in the missing tone");
        assert_eq!(edge.fill, LIGHT.surface, "danger is never filled");
        assert!(fills(&button_shapes(danger_, true, false)).contains(&LIGHT.missing.tint));

        // Keyboard focus shows a 2 px accent ring on both.
        for focused in [
            button_shapes(primary_, false, true),
            button_shapes(danger_, false, true),
        ] {
            assert!(
                rects(&focused)
                    .iter()
                    .any(|r| r.stroke == Stroke::new(2.0, LIGHT.accent)),
                "no focus ring"
            );
        }
        assert!(
            !rects(&rest)
                .iter()
                .any(|r| r.stroke == Stroke::new(2.0, LIGHT.accent))
        );
    }
}
