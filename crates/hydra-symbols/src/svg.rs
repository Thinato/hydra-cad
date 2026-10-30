//! Primitives to a standalone SVG document.

use std::fmt::Write;

use crate::{Align, Primitive, bounds};

const STROKE: f32 = 0.25;
const MARGIN: f32 = 2.0;

/// Render primitives as an SVG whose user units are millimetres.
pub fn to_svg(primitives: &[Primitive]) -> String {
    let ([x0, y0], [x1, y1]) = bounds(primitives).unwrap_or(([0.0; 2], [0.0; 2]));
    let (x, y) = (x0 - MARGIN, y0 - MARGIN);
    let (w, h) = (x1 - x0 + 2.0 * MARGIN, y1 - y0 + 2.0 * MARGIN);
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}mm" height="{h:.2}mm" viewBox="{x:.2} {y:.2} {w:.2} {h:.2}">"#
    );
    let _ = writeln!(
        s,
        r#"<g fill="none" stroke="black" stroke-width="{STROKE}" stroke-linecap="round" stroke-linejoin="round">"#
    );
    for p in primitives {
        match p {
            Primitive::Polyline {
                points,
                closed,
                dashed,
                filled,
            } => {
                let tag = if *closed || *filled {
                    "polygon"
                } else {
                    "polyline"
                };
                let pts: Vec<String> = points
                    .iter()
                    .map(|[x, y]| format!("{x:.2},{y:.2}"))
                    .collect();
                let fill = if *filled { r#" fill="black""# } else { "" };
                let dash = if *dashed {
                    r#" stroke-dasharray="1,0.8""#
                } else {
                    ""
                };
                let _ = writeln!(s, r#"<{tag} points="{}"{fill}{dash}/>"#, pts.join(" "));
            }
            Primitive::Circle {
                center: [cx, cy],
                radius,
                filled,
            } => {
                let fill = if *filled { r#" fill="black""# } else { "" };
                let _ = writeln!(
                    s,
                    r#"<circle cx="{cx:.2}" cy="{cy:.2}" r="{radius:.2}"{fill}/>"#
                );
            }
            Primitive::Text {
                at: [tx, ty],
                text,
                size,
                align,
            } => {
                let anchor = match align {
                    Align::Start => "start",
                    Align::Middle => "middle",
                    Align::End => "end",
                };
                let _ = writeln!(
                    s,
                    r#"<text x="{tx:.2}" y="{ty:.2}" font-size="{size}" font-family="sans-serif" text-anchor="{anchor}" dominant-baseline="central" fill="black" stroke="none">{}</text>"#,
                    escape(text)
                );
            }
        }
    }
    s.push_str("</g>\n</svg>\n");
    s
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render primitives as a one-page PDF, the page the size of the drawing in millimetres.
///
/// Text uses egui's bundled font so the output does not depend on the machine.
pub fn to_pdf(primitives: &[Primitive]) -> Result<Vec<u8>, String> {
    use svg2pdf::usvg::{Options, Tree};

    let mut options = Options::default();
    let fontdb = options.fontdb_mut();
    fontdb.load_font_data(epaint_default_fonts::UBUNTU_LIGHT.to_vec());
    let family = fontdb
        .faces()
        .next()
        .and_then(|face| face.families.first())
        .map(|(name, _)| name.clone())
        .ok_or("could not build the PDF: the bundled font did not load")?;
    fontdb.set_sans_serif_family(family);
    let tree = Tree::from_str(&to_svg(primitives), &options)
        .map_err(|e| format!("could not build the PDF: {e}"))?;
    // usvg reads millimetres at 96 dpi
    let page = svg2pdf::PageOptions { dpi: 96.0 };
    svg2pdf::to_pdf(&tree, svg2pdf::ConversionOptions::default(), page)
        .map_err(|e| format!("could not build the PDF: {e}"))
}
