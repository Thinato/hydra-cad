//! Drawing helpers: paint hydra-symbols primitives with the egui painter.

use eframe::egui::{self, Align2, FontFamily, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use hydra_symbols::{Align, Font, Pen, Point, Primitive, Sheet, bounds};

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
            paint_grid(painter, paper.intersect(rect), paper.min, zoom);
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

/// Grid lines every 5 mm from the sheet's corner `corner`, every fifth stronger, within
/// `visible`; the 5 mm ones only while they are at least 4 px apart.
fn paint_grid(painter: &egui::Painter, visible: Rect, corner: Pos2, zoom: f32) {
    if !visible.is_positive() {
        return;
    }
    let step = GRID * zoom;
    let every = if step >= GRID_MIN_PX { 1 } else { MAJOR };
    if step * (every as f32) < GRID_MIN_PX {
        return;
    }
    // Only the lines on screen: their indices from the corner, along each axis.
    let lines = |lo: f32, hi: f32, start: f32| {
        let first = ((lo - start) / step).ceil().max(0.0) as usize;
        let last = ((hi - start) / step).floor().max(0.0) as usize;
        (first..=last)
            .filter(move |i| i % every == 0)
            .map(move |i| (start + i as f32 * step, i % MAJOR == 0))
    };
    let colour = |major: bool| if major { LIGHT.grid_major } else { LIGHT.grid };
    for (x, major) in lines(visible.min.x, visible.max.x, corner.x) {
        painter.vline(x, visible.y_range(), Stroke::new(1.0, colour(major)));
    }
    for (y, major) in lines(visible.min.y, visible.max.y, corner.y) {
        painter.hline(visible.x_range(), y, Stroke::new(1.0, colour(major)));
    }
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
