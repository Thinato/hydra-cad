//! Drawing helpers: paint hydra-symbols primitives with the egui painter.

use eframe::egui::{self, Align2, FontFamily, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use hydra_core::SymbolDef;
use hydra_symbols::{Align, Font, Pen, Point, Primitive, Sheet, bounds, render_symbol};

use crate::theme::{self, LIGHT};

/// Grid spacing on the sheet, and every how many lines one is drawn stronger.
const GRID: f32 = 5.0;
const MAJOR: usize = 5;
/// Grid lines closer than this on screen are left out.
const GRID_MIN_PX: f32 = 4.0;

/// Draw primitives scaled to fit a fixed-size box, e.g. a symbol preview.
pub fn draw_fitted(ui: &mut egui::Ui, size: Vec2, primitives: &[Primitive]) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let (pan, zoom) = fit(bounds(primitives), size);
    paint_sheet(&ui.painter_at(rect), rect, pan, zoom, None, primitives);
}

/// A symbol preview: `symbol` fitted in `size` on the sheet colour, over a caption strip with
/// `caption` at its left and what `right` adds at its right, such as the symbol's reference.
pub fn symbol_preview(
    ui: &mut egui::Ui,
    size: Vec2,
    symbol: Option<&SymbolDef>,
    caption: &str,
    right: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(LIGHT.sheet)
        .stroke(Stroke::new(1.0, LIGHT.border))
        .show(ui, |ui| {
            ui.set_width(size.x);
            ui.spacing_mut().item_spacing.y = 0.0;
            match symbol {
                Some(def) => draw_fitted(ui, size, &render_symbol(def).primitives),
                None => {
                    ui.allocate_exact_size(size, Sense::hover());
                }
            }
            let strip = egui::Frame::new()
                .fill(LIGHT.panel)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(caption)
                                .small()
                                .family(FontFamily::Name(theme::SEMIBOLD.into())),
                        );
                        let layout = egui::Layout::right_to_left(egui::Align::Center);
                        ui.with_layout(layout, right);
                    });
                });
            let rect = strip.response.rect;
            ui.painter()
                .hline(rect.x_range(), rect.top(), Stroke::new(1.0, LIGHT.border));
        });
}

/// Pan and zoom that fit the box `(min, max)` into a view of `size` with a small margin.
pub fn fit(bounds: Option<(Point, Point)>, size: Vec2) -> (Vec2, f32) {
    let Some(([x0, y0], [x1, y1])) = bounds else {
        return (Vec2::ZERO, 1.0);
    };
    let extent = Vec2::new(x1 - x0, y1 - y0).max(Vec2::splat(1.0));
    let zoom = (size.x / extent.x).min(size.y / extent.y) * 0.9;
    let centre = Vec2::new(x0 + x1, y0 + y1) / 2.0;
    (size / 2.0 - centre * zoom, zoom)
}

/// Primitives in ink; `pan` is the drawing origin's offset from `rect.min`. With a `sheet`,
/// they sit on that sheet of paper, gridded every 5 mm, on the grey desk, with the sheet's own
/// primitives such as its title block; without one, the whole of `rect` is paper.
pub fn paint_sheet(
    painter: &egui::Painter,
    rect: Rect,
    pan: Vec2,
    zoom: f32,
    sheet: Option<(&Sheet, &[Primitive])>,
    primitives: &[Primitive],
) {
    let origin = rect.min + pan;
    let to_screen = |[x, y]: Point| origin + Vec2::new(x, y) * zoom;
    let printed = match sheet {
        None => {
            painter.rect_filled(rect, 0.0, LIGHT.sheet);
            &[][..]
        }
        Some((sheet, printed)) => {
            painter.rect_filled(rect, 0.0, LIGHT.desk);
            let paper = Rect::from_min_max(to_screen(sheet.min), to_screen(sheet.max));
            painter.rect_filled(paper, 0.0, LIGHT.sheet);
            paint_grid(painter, paper.intersect(rect), origin, zoom);
            painter.rect_stroke(
                paper,
                0.0,
                Stroke::new(1.0, LIGHT.border_strong),
                egui::StrokeKind::Outside,
            );
            printed
        }
    };
    for prim in primitives.iter().chain(printed) {
        paint(painter, prim, &to_screen, zoom);
    }
}

/// The grid within `visible`, drawn from `origin`, the drawing's origin on screen. The sheet's
/// corner is on the 5 mm grid, so its lines are the drawing's multiples of 5 mm.
fn paint_grid(painter: &egui::Painter, visible: Rect, origin: Pos2, zoom: f32) {
    if !visible.is_positive() {
        return;
    }
    let colour = |major: bool| if major { LIGHT.grid_major } else { LIGHT.grid };
    let mm = |px: f32, at: f32| (px - at) / zoom;
    let along = |lo: f32, hi: f32, at: f32| grid_lines(mm(lo, at), mm(hi, at), zoom);
    for (x, major) in along(visible.min.x, visible.max.x, origin.x) {
        let x = origin.x + x * zoom;
        painter.vline(x, visible.y_range(), Stroke::new(1.0, colour(major)));
    }
    for (y, major) in along(visible.min.y, visible.max.y, origin.y) {
        let y = origin.y + y * zoom;
        painter.hline(visible.x_range(), y, Stroke::new(1.0, colour(major)));
    }
}

/// Grid lines from `lo` to `hi` mm along one axis at `zoom` px per mm: each multiple of 5 mm,
/// and whether it is a multiple of 25 mm. The 5 mm lines are left out while closer than 4 px,
/// and every line while the 25 mm ones would be.
fn grid_lines(lo: f32, hi: f32, zoom: f32) -> Vec<(f32, bool)> {
    let every = if GRID * zoom >= GRID_MIN_PX {
        1
    } else {
        MAJOR as i64
    };
    if GRID * zoom * (every as f32) < GRID_MIN_PX {
        return Vec::new();
    }
    let (first, last) = ((lo / GRID).ceil() as i64, (hi / GRID).floor() as i64);
    (first..=last)
        .filter(|k| k.rem_euclid(every) == 0)
        .map(|k| (k as f32 * GRID, k.rem_euclid(MAJOR as i64) == 0))
        .collect()
}

fn paint(painter: &egui::Painter, prim: &Primitive, to_screen: &impl Fn(Point) -> Pos2, zoom: f32) {
    let ink = LIGHT.ink;
    // At least a pixel, so a line never vanishes when zoomed out.
    let stroke = |pen: Pen| Stroke::new((pen.width() * zoom).max(1.0), ink);
    match prim {
        Primitive::Polyline {
            points,
            closed,
            pen,
            filled,
        } => {
            let mut pts: Vec<Pos2> = points.iter().map(|p| to_screen(*p)).collect();
            if *filled {
                painter.add(Shape::convex_polygon(pts, ink, Stroke::NONE));
            } else if *pen == Pen::Border {
                // The title block is paper over the grid, as on the boards.
                painter.add(Shape::convex_polygon(pts, LIGHT.sheet, stroke(*pen)));
            } else if let Some([on, off]) = pen.dash() {
                if *closed {
                    pts.push(pts[0]);
                }
                painter.extend(Shape::dashed_line(
                    &pts,
                    stroke(*pen),
                    on * zoom,
                    off * zoom,
                ));
            } else if *closed {
                painter.add(Shape::closed_line(pts, stroke(*pen)));
            } else {
                painter.add(Shape::line(pts, stroke(*pen)));
            }
        }
        Primitive::Circle {
            center,
            radius,
            filled,
        } => {
            let (c, r) = (to_screen(*center), radius * zoom);
            if *filled {
                painter.circle_filled(c, r, ink);
            } else {
                painter.circle_stroke(c, r, stroke(Pen::Body));
            }
        }
        Primitive::Text {
            at,
            text,
            size,
            align,
            font,
        } => {
            let anchor = match align {
                Align::Start => Align2::LEFT_CENTER,
                Align::Middle => Align2::CENTER_CENTER,
                Align::End => Align2::RIGHT_CENTER,
            };
            let named = |name: &str| FontFamily::Name(name.into());
            let (family, colour) = match font {
                Font::Sans => (FontFamily::Proportional, ink),
                Font::Bold => (named(theme::SEMIBOLD), ink),
                Font::Caption => (named(theme::CONDENSED), LIGHT.secondary),
                Font::Warning => (named(theme::SEMIBOLD), LIGHT.unlisted.text),
            };
            let font = FontId::new(size * zoom, family);
            painter.text(to_screen(*at), anchor, text, font, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::grid_lines;

    #[test]
    fn grid_lines_fall_on_multiples_of_5_mm_with_25_mm_majors_and_thin_out() {
        // 4 px apart: every 5 mm line, the 25 mm multiples major, wherever the view starts.
        assert_eq!(
            grid_lines(-27.0, 7.0, 0.8),
            [
                (-25.0, true),
                (-20.0, false),
                (-15.0, false),
                (-10.0, false),
                (-5.0, false),
                (0.0, true),
                (5.0, false)
            ]
        );
        // Majors are the drawing's multiples of 25 mm, not every fifth line from the view's edge.
        assert_eq!(
            grid_lines(-17.0, 12.0, 0.8),
            [
                (-15.0, false),
                (-10.0, false),
                (-5.0, false),
                (0.0, true),
                (5.0, false),
                (10.0, false)
            ]
        );
        // Closer than 4 px: only the 25 mm lines, until they too would be.
        assert_eq!(
            grid_lines(-27.0, 60.0, 0.79),
            [(-25.0, true), (0.0, true), (25.0, true), (50.0, true)]
        );
        assert_eq!(grid_lines(-27.0, 60.0, 0.17).len(), 4);
        assert!(grid_lines(-27.0, 60.0, 0.15).is_empty());
    }
}
