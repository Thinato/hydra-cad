//! Drawing helpers: paint hydra-symbols primitives with the egui painter.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use hydra_symbols::{Align, Primitive, bounds};

/// Draw primitives scaled to fit a fixed-size box, e.g. a symbol preview.
pub fn draw_fitted(ui: &mut egui::Ui, size: Vec2, primitives: &[Primitive]) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let (pan, zoom) = fit(primitives, size);
    paint_sheet(&ui.painter_at(rect), rect, pan, zoom, primitives);
}

/// Pan and zoom that fit the primitives into a view of `size` with a small margin.
pub fn fit(primitives: &[Primitive], size: Vec2) -> (Vec2, f32) {
    let Some(([x0, y0], [x1, y1])) = bounds(primitives) else {
        return (Vec2::ZERO, 1.0);
    };
    let extent = Vec2::new(x1 - x0, y1 - y0).max(Vec2::splat(1.0));
    let zoom = (size.x / extent.x).min(size.y / extent.y) * 0.9;
    let centre = Vec2::new(x0 + x1, y0 + y1) / 2.0;
    (size / 2.0 - centre * zoom, zoom)
}

/// White sheet with black primitives; `pan` is the sheet origin's offset from `rect.min`.
pub fn paint_sheet(
    painter: &egui::Painter,
    rect: Rect,
    pan: Vec2,
    zoom: f32,
    primitives: &[Primitive],
) {
    painter.rect_filled(rect, 0.0, Color32::WHITE);
    let origin = rect.min + pan;
    let to_screen = |[x, y]: [f32; 2]| origin + Vec2::new(x, y) * zoom;
    let stroke = Stroke::new((0.25 * zoom).max(1.0), Color32::BLACK);
    for prim in primitives {
        paint(painter, prim, &to_screen, stroke, zoom);
    }
}

fn paint(
    painter: &egui::Painter,
    prim: &Primitive,
    to_screen: &impl Fn([f32; 2]) -> Pos2,
    stroke: Stroke,
    zoom: f32,
) {
    match prim {
        Primitive::Polyline {
            points,
            closed,
            dashed,
            filled,
        } => {
            let mut pts: Vec<Pos2> = points.iter().map(|p| to_screen(*p)).collect();
            if *filled {
                painter.add(Shape::convex_polygon(pts, stroke.color, Stroke::NONE));
            } else if *dashed {
                if *closed {
                    pts.push(pts[0]);
                }
                painter.extend(Shape::dashed_line(&pts, stroke, zoom, 0.8 * zoom));
            } else if *closed {
                painter.add(Shape::closed_line(pts, stroke));
            } else {
                painter.add(Shape::line(pts, stroke));
            }
        }
        Primitive::Circle {
            center,
            radius,
            filled,
        } => {
            let (c, r) = (to_screen(*center), radius * zoom);
            if *filled {
                painter.circle_filled(c, r, stroke.color);
            } else {
                painter.circle_stroke(c, r, stroke);
            }
        }
        Primitive::Text {
            at,
            text,
            size,
            align,
        } => {
            let anchor = match align {
                Align::Start => Align2::LEFT_CENTER,
                Align::Middle => Align2::CENTER_CENTER,
                Align::End => Align2::RIGHT_CENTER,
            };
            let font = FontId::proportional(size * zoom);
            painter.text(to_screen(*at), anchor, text, font, stroke.color);
        }
    }
}
