//! Primitives to a standalone SVG document.

use std::fmt::Write;

use crate::{Align, Font, Pen, Point, Primitive, Sheet, bounds};

const MARGIN: f32 = 2.0;

// The fonts live with the app's in `crates/hydra/fonts/` (one copy, under its OFL). A path out
// of this crate is fine: the crate is only built inside this workspace, and the bytes are
// compiled in. The app installs these same statics, so the binary holds them once.
/// IBM Plex Sans Regular: everything on the drawing.
pub static PLEX_SANS: &[u8] = include_bytes!("../../hydra/fonts/IBMPlexSans-Regular.ttf");
/// IBM Plex Sans SemiBold: the title block's name and warnings.
pub static PLEX_SANS_SEMIBOLD: &[u8] = include_bytes!("../../hydra/fonts/IBMPlexSans-SemiBold.ttf");
/// IBM Plex Sans Condensed SemiBold: the title block's labels.
pub static PLEX_SANS_CONDENSED_SEMIBOLD: &[u8] =
    include_bytes!("../../hydra/fonts/IBMPlexSansCondensed-SemiBold.ttf");

/// Render primitives as an SVG whose user units are millimetres, cut close round them. Lines
/// are black on white, as printed, rather than the canvas's ink.
pub fn to_svg(primitives: &[Primitive]) -> String {
    let ([x0, y0], [x1, y1]) = bounds(primitives).unwrap_or(([0.0; 2], [0.0; 2]));
    svg(
        [x0 - MARGIN, y0 - MARGIN],
        [x1 + MARGIN, y1 + MARGIN],
        primitives,
    )
}

/// Render primitives on their sheet: an SVG the size of the sheet, in millimetres.
pub fn sheet_svg(sheet: &Sheet, primitives: &[Primitive]) -> String {
    svg(sheet.min, sheet.max, primitives)
}

fn svg([x, y]: Point, [x1, y1]: Point, primitives: &[Primitive]) -> String {
    let (w, h) = (x1 - x, y1 - y);
    let body = Pen::Body.width();
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}mm" height="{h:.2}mm" viewBox="{x:.2} {y:.2} {w:.2} {h:.2}">"#
    );
    let _ = writeln!(
        s,
        r#"<g fill="none" stroke="black" stroke-width="{body}" stroke-linecap="round" stroke-linejoin="round">"#
    );
    for p in primitives {
        match p {
            Primitive::Polyline {
                points,
                closed,
                pen,
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
                let width = match pen.width() {
                    w if w == body => String::new(),
                    w => format!(r#" stroke-width="{w}""#),
                };
                let dash = match pen.dash() {
                    Some([on, off]) => format!(r#" stroke-dasharray="{on},{off}""#),
                    None => String::new(),
                };
                let _ = writeln!(
                    s,
                    r#"<{tag} points="{}"{fill}{width}{dash}/>"#,
                    pts.join(" ")
                );
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
                font,
            } => {
                let anchor = match align {
                    Align::Start => "start",
                    Align::Middle => "middle",
                    Align::End => "end",
                };
                let (family, weight) = match font {
                    Font::Sans => ("IBM Plex Sans", ""),
                    Font::Bold | Font::Warning => ("IBM Plex Sans", r#" font-weight="600""#),
                    Font::Caption => ("IBM Plex Sans Condensed", r#" font-weight="600""#),
                };
                let _ = writeln!(
                    s,
                    r#"<text x="{tx:.2}" y="{ty:.2}" font-size="{size}" font-family="'{family}', sans-serif"{weight} text-anchor="{anchor}" dominant-baseline="central" fill="black" stroke="none">{}</text>"#,
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

/// Render primitives on their sheet as a one-page PDF the size of the sheet.
///
/// Text is IBM Plex, embedded from the bundled fonts, so the output does not depend on the
/// machine.
pub fn sheet_pdf(sheet: &Sheet, primitives: &[Primitive]) -> Result<Vec<u8>, String> {
    use svg2pdf::usvg::{Options, Tree};

    let mut options = Options::default();
    let fontdb = options.fontdb_mut();
    for font in [PLEX_SANS, PLEX_SANS_SEMIBOLD, PLEX_SANS_CONDENSED_SEMIBOLD] {
        fontdb.load_font_data(font.to_vec());
    }
    fontdb.set_sans_serif_family("IBM Plex Sans");
    let tree = Tree::from_str(&sheet_svg(sheet, primitives), &options)
        .map_err(|e| format!("could not build the PDF: {e}"))?;
    // usvg reads millimetres at 96 dpi
    let page = svg2pdf::PageOptions { dpi: 96.0 };
    svg2pdf::to_pdf(&tree, svg2pdf::ConversionOptions::default(), page)
        .map_err(|e| format!("could not build the PDF: {e}"))
}
