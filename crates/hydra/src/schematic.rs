//! Read-only schematic view: draws a circuit's primitives with the egui painter.

use std::path::Path;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use hydra_core::{Catalogue, Circuit, Severity};
use hydra_symbols::{Align, Drawing, Primitive, bounds, render_circuit};

pub struct SchematicView {
    name: String,
    drawing: Drawing,
    /// Catalogue problems and circuit validation, in plain words.
    issues: Vec<(Severity, String)>,
    /// Screen offset of the sheet origin from the view's top-left corner.
    pan: Vec2,
    /// Screen points per millimetre.
    zoom: f32,
    /// View size the circuit was last fitted to; refit on resize until the user pans or zooms.
    fitted_to: Option<Vec2>,
}

impl SchematicView {
    /// Load a circuit document and the catalogue it is drawn against.
    pub fn load(circuit: &Path, catalogue: &Path) -> anyhow::Result<Self> {
        let read = |p: &Path| {
            std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))
        };
        let catalogue: Catalogue = serde_json::from_str(&read(catalogue)?)?;
        let circuit = Circuit::from_json(&read(circuit)?)?;
        Ok(Self {
            name: circuit.name.clone(),
            drawing: render_circuit(&circuit, &catalogue),
            issues: catalogue
                .problems()
                .into_iter()
                .map(|p| (Severity::Error, format!("catalogue: {p}")))
                .chain(
                    circuit
                        .validate(&catalogue)
                        .iter()
                        .map(|i| (i.severity(), i.to_string())),
                )
                .collect(),
            pan: Vec2::ZERO,
            zoom: 1.0,
            fitted_to: Some(Vec2::ZERO),
        })
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let errors = self
            .issues
            .iter()
            .filter(|(s, _)| *s == Severity::Error)
            .count();
        let warnings = self.issues.len() - errors;
        ui.horizontal(|ui| {
            // Truncate so a long name never widens the panel past its visible size.
            ui.label(format!("{errors} errors, {warnings} warnings"));
            let hint = "drag to pan, scroll to zoom, double-click to fit";
            ui.add(egui::Label::new(egui::RichText::new(&self.name).strong()).truncate())
                .on_hover_text(hint);
        });
        if !self.issues.is_empty() {
            egui::CollapsingHeader::new("Validation")
                .default_open(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (severity, message) in &self.issues {
                                let color = match severity {
                                    Severity::Error => ui.visuals().error_fg_color,
                                    Severity::Warning => ui.visuals().warn_fg_color,
                                };
                                ui.colored_label(color, message);
                            }
                        });
                });
        }

        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if response.double_clicked() || self.fitted_to.is_some_and(|s| s != rect.size()) {
            self.fit(rect);
            self.fitted_to = Some(rect.size());
        }
        if response.dragged() {
            self.pan += response.drag_delta();
            self.fitted_to = None;
        }
        if let Some(pointer) = response.hover_pos() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            // Clamped so text sizes stay sane at both ends.
            let zoom = (self.zoom * (scroll * 0.002).exp() * pinch).clamp(0.05, 200.0);
            let factor = zoom / self.zoom;
            if factor != 1.0 {
                // Keep the sheet point under the pointer still.
                let anchor = pointer - rect.min;
                self.pan = anchor - (anchor - self.pan) * factor;
                self.zoom *= factor;
                self.fitted_to = None;
            }
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::WHITE);
        let origin = rect.min + self.pan;
        let to_screen = |[x, y]: [f32; 2]| origin + Vec2::new(x, y) * self.zoom;
        let stroke = Stroke::new((0.25 * self.zoom).max(1.0), Color32::BLACK);
        for prim in &self.drawing.primitives {
            paint(&painter, prim, &to_screen, stroke, self.zoom);
        }
    }

    /// Zoom and pan so the whole circuit fills `rect` with a small margin.
    fn fit(&mut self, rect: Rect) {
        let Some(([x0, y0], [x1, y1])) = bounds(&self.drawing.primitives) else {
            self.zoom = 1.0;
            return;
        };
        let size = Vec2::new(x1 - x0, y1 - y0).max(Vec2::splat(1.0));
        self.zoom = (rect.width() / size.x).min(rect.height() / size.y) * 0.9;
        let centre = Vec2::new(x0 + x1, y0 + y1) / 2.0;
        self.pan = rect.size() / 2.0 - centre * self.zoom;
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
