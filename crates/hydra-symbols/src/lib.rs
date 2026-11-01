//! ISO 1219 symbols as plain drawing primitives.
//!
//! [`render_symbol`] turns a symbol definition into polylines, circles and text in millimetres
//! (x right, y down), with an anchor point per port. [`render_circuit`] places symbols and draws
//! the nets. The canvas, SVG and PDF export all draw the same primitives.

use hydra_core::{
    Actuator, Catalogue, Circuit, Component, ComponentKind, DirectionalValve, FlowActuator, Issue,
    MAX_LISTED, Net, PORT_PIN, Placement, Rotation, Side, SymbolDef, ValveStatus,
};

mod svg;
pub use svg::{to_pdf, to_svg};

pub type Point = [f32; 2];

#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Polyline {
        points: Vec<Point>,
        closed: bool,
        dashed: bool,
        filled: bool,
    },
    Circle {
        center: Point,
        radius: f32,
        filled: bool,
    },
    Text {
        at: Point,
        text: String,
        size: f32,
        align: Align,
    },
}

/// Horizontal text alignment around `at`; text is vertically centred on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Middle,
    End,
}

/// A rendered symbol or circuit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    pub primitives: Vec<Primitive>,
    /// Where each port's line ends, by port name.
    pub ports: Vec<(String, Point)>,
}

/// Side of one spool envelope.
const ENVELOPE: f32 = 10.0;
/// Length of the port line outside a symbol.
const STUB: f32 = 3.0;
/// Width of one actuator box.
const ACTUATOR: f32 = 5.0;
const LABEL: f32 = 2.5;

fn line(a: Point, b: Point) -> Primitive {
    Primitive::Polyline {
        points: vec![a, b],
        closed: false,
        dashed: false,
        filled: false,
    }
}

fn open(points: Vec<Point>) -> Primitive {
    Primitive::Polyline {
        points,
        closed: false,
        dashed: false,
        filled: false,
    }
}

fn dashed(points: Vec<Point>) -> Primitive {
    Primitive::Polyline {
        points,
        closed: false,
        dashed: true,
        filled: false,
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Primitive {
    Primitive::Polyline {
        points: vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        closed: true,
        dashed: false,
        filled: false,
    }
}

fn text(at: Point, s: &str, align: Align) -> Primitive {
    Primitive::Text {
        at,
        text: s.to_string(),
        size: LABEL,
        align,
    }
}

/// Filled arrow head with its tip at `tip`, pointing away from `from`.
fn arrow_head(from: Point, tip: Point) -> Primitive {
    let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    let (ux, uy) = (dx / len, dy / len);
    let (l, w) = (1.6, 0.6);
    let base = [tip[0] - ux * l, tip[1] - uy * l];
    Primitive::Polyline {
        points: vec![
            tip,
            [base[0] - uy * w, base[1] + ux * w],
            [base[0] + uy * w, base[1] - ux * w],
        ],
        closed: true,
        dashed: false,
        filled: true,
    }
}

/// Zigzag spring from `a` to `b` with `turns` peaks.
fn spring(a: Point, b: Point, turns: usize, amplitude: f32) -> Primitive {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    let (nx, ny) = (-dy / len * amplitude, dx / len * amplitude);
    let steps = turns * 2;
    let mut points = vec![a];
    for i in 0..steps {
        let t = (i as f32 + 0.5) / steps as f32;
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        points.push([a[0] + dx * t + nx * side, a[1] + dy * t + ny * side]);
    }
    points.push(b);
    open(points)
}

/// Polyline approximating a circular arc, angles in degrees, y down.
fn arc(center: Point, radius: f32, from_deg: f32, to_deg: f32) -> Primitive {
    let n = 8;
    open(
        (0..=n)
            .map(|i| {
                let a = (from_deg + (to_deg - from_deg) * i as f32 / n as f32).to_radians();
                [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
            })
            .collect(),
    )
}

/// Diagonal arrow through a symbol, the ISO mark for "adjustable".
fn adjustable(center: Point, half: f32) -> [Primitive; 2] {
    let from = [center[0] - half, center[1] + half];
    let tip = [center[0] + half, center[1] - half];
    [line(from, tip), arrow_head(from, tip)]
}

/// Render one symbol around its origin: the centre of the rest-position envelope.
pub fn render_symbol(def: &SymbolDef) -> Drawing {
    match def {
        SymbolDef::DirectionalValve(v) => directional_valve(v),
        SymbolDef::Check { spring: has_spring } => check(*has_spring),
        SymbolDef::Relief { adjustable: adj } => relief(*adj),
        SymbolDef::Orifice { adjustable: adj } => orifice(*adj),
        SymbolDef::FlowControl {
            actuator,
            compensated,
            bypass,
        } => flow_control(*actuator, *compensated, *bypass),
        SymbolDef::PilotCheck { spring: has_spring } => pilot_check(*has_spring),
        SymbolDef::Counterbalance => counterbalance(),
        SymbolDef::TestPoint => test_point(),
        SymbolDef::HydraulicMotor {
            bidirectional,
            variable,
            drain,
        } => hydraulic_motor(*bidirectional, *variable, *drain),
        SymbolDef::Pump { variable } => pump(*variable),
        SymbolDef::Box { label, ports } => labelled_box(label, ports),
    }
}

/// Horizontal port on the left (`left`) or right side at height `y`, its line starting at `x`.
fn side_port(d: &mut Drawing, name: &str, x: f32, y: f32, left: bool) {
    let edge = if left { x - STUB } else { x + STUB };
    d.primitives.push(line([x, y], [edge, y]));
    // Name above the line, running away from the symbol.
    let (at, align) = if left {
        ([x - 0.5, y - 1.5], Align::End)
    } else {
        ([x + 0.5, y - 1.5], Align::Start)
    };
    d.primitives.push(text(at, name, align));
    d.ports.push((name.to_string(), [edge, y]));
}

/// Dashed pilot line from a port on the left side into `tip`.
fn pilot_from_left(d: &mut Drawing, name: &str, x: f32, tip: Point) {
    d.primitives.push(dashed(vec![[x, tip[1]], tip]));
    d.primitives.push(arrow_head([x, tip[1]], tip));
    side_port(d, name, x, tip[1], true);
}

fn triangle(tip: Point, base_mid: Point, half: f32) -> Primitive {
    let (dx, dy) = (tip[0] - base_mid[0], tip[1] - base_mid[1]);
    let len = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    let (nx, ny) = (-dy / len * half, dx / len * half);
    Primitive::Polyline {
        points: vec![
            tip,
            [base_mid[0] + nx, base_mid[1] + ny],
            [base_mid[0] - nx, base_mid[1] - ny],
        ],
        closed: true,
        dashed: false,
        filled: true,
    }
}

/// Flow control from In (bottom) to Out (top), with Bypass on the right when `bypass`.
fn flow_control(actuator: FlowActuator, compensated: bool, bypass: bool) -> Drawing {
    let mut d = Drawing::default();
    let h = ENVELOPE / 2.0;
    d.primitives.push(rect(-h, -h, ENVELOPE, ENVELOPE));
    d.primitives.push(line([0.0, -h], [0.0, h]));
    d.primitives.push(arc([-2.6, 0.0], 2.2, -50.0, 50.0));
    d.primitives.push(arc([2.6, 0.0], 2.2, 130.0, 230.0));
    d.primitives.extend(adjustable([0.0, 0.0], 3.5));
    if compensated {
        // Compensator: a small arrow beside the throttle.
        let (from, tip) = ([3.3, 4.3], [3.3, 0.8]);
        d.primitives.push(line(from, tip));
        d.primitives.push(arrow_head(from, tip));
    }
    if actuator == FlowActuator::Proportional {
        // Proportional solenoid on the left: a box with two slanted strokes.
        let (x, y, w, hh) = (-h - ACTUATOR, -2.0, ACTUATOR, 4.0);
        d.primitives.push(rect(x, y, w, hh));
        d.primitives
            .push(line([x + w * 0.2, y + hh], [x + w * 0.6, y]));
        d.primitives
            .push(line([x + w * 0.45, y + hh], [x + w * 0.85, y]));
    }
    if bypass {
        d.primitives.push(line([0.0, h - 2.0], [h, h - 2.0]));
        side_port(&mut d, "Bypass", h, h - 2.0, false);
    }
    vertical_ports(&mut d, "Out", "In", h);
    d
}

/// Check valve A (bottom) to B (top), opened backwards by pilot pressure on X (left).
fn pilot_check(has_spring: bool) -> Drawing {
    let mut d = check(has_spring);
    pilot_from_left(&mut d, "X", -5.0, [-1.8, 1.5]);
    d
}

/// Counterbalance valve: V (bottom) to C (top) through the check, C held until X opens it.
fn counterbalance() -> Drawing {
    let mut d = Drawing::default();
    let h = ENVELOPE / 2.0;
    d.primitives.push(rect(-h, -h, ENVELOPE, ENVELOPE));
    // Relief path C -> V, offset left of the port line.
    let (from, tip) = ([-2.0, -h + 1.0], [-2.0, h - 1.0]);
    d.primitives.push(line(from, tip));
    d.primitives.push(arrow_head(from, tip));
    // Check V -> C on the right.
    d.primitives.push(Primitive::Circle {
        center: [2.5, -0.5],
        radius: 1.2,
        filled: false,
    });
    d.primitives
        .push(open(vec![[0.9, 0.2], [2.5, 2.5], [4.1, 0.2]]));
    d.primitives.push(line([2.5, 2.5], [2.5, h]));
    d.primitives.push(line([2.5, -1.7], [2.5, -h]));
    d.primitives.push(line([0.0, -h], [0.0, -h + 1.0]));
    d.primitives.push(line([0.0, h], [0.0, h - 1.0]));
    d.primitives
        .push(spring([h, -2.5], [h + 5.0, -2.5], 3, 1.5));
    pilot_from_left(&mut d, "X", -h - 2.0, [-h, 2.5]);
    vertical_ports(&mut d, "C", "V", h);
    d
}

/// Gauge connection M.
fn test_point() -> Drawing {
    let mut d = Drawing::default();
    let r = 1.8;
    d.primitives.push(Primitive::Circle {
        center: [0.0, 0.0],
        radius: r,
        filled: false,
    });
    let k = r * 0.7;
    d.primitives.push(line([-k, -k], [k, k]));
    d.primitives.push(line([-k, k], [k, -k]));
    d.primitives.push(line([0.0, r], [0.0, r + STUB]));
    d.primitives
        .push(text([1.0, r + STUB - 1.0], "M", Align::Start));
    d.ports.push(("M".to_string(), [0.0, r + STUB]));
    d
}

/// Hydraulic motor A (bottom) to B (top), drain L on the right when `drain`.
fn hydraulic_motor(bidirectional: bool, variable: bool, drain: bool) -> Drawing {
    let mut d = Drawing::default();
    let r = 5.0;
    d.primitives.push(Primitive::Circle {
        center: [0.0, 0.0],
        radius: r,
        filled: false,
    });
    // Motors: triangles pointing in, from where the oil arrives.
    d.primitives.push(triangle([0.0, r - 2.2], [0.0, r], 1.3));
    if bidirectional {
        d.primitives.push(triangle([0.0, -r + 2.2], [0.0, -r], 1.3));
    }
    if variable {
        d.primitives.extend(adjustable([0.0, 0.0], r + 1.0));
    }
    if drain {
        d.primitives
            .push(dashed(vec![[r * 0.7, r * 0.7], [r + 1.0, r * 0.7]]));
        side_port(&mut d, "L", r + 1.0, r * 0.7, false);
    }
    vertical_ports(&mut d, "B", "A", r);
    d
}

/// Pump from S (bottom) to P (top).
fn pump(variable: bool) -> Drawing {
    let mut d = Drawing::default();
    let r = 5.0;
    d.primitives.push(Primitive::Circle {
        center: [0.0, 0.0],
        radius: r,
        filled: false,
    });
    // Pumps: the triangle points out, towards P.
    d.primitives.push(triangle([0.0, -r], [0.0, -r + 2.2], 1.3));
    if variable {
        d.primitives.extend(adjustable([0.0, 0.0], r + 1.0));
    }
    vertical_ports(&mut d, "P", "S", r);
    d
}

/// Box with a label, the first half of the ports on top and the rest at the bottom.
fn labelled_box(label: &str, ports: &[String]) -> Drawing {
    let mut d = Drawing::default();
    let top = ports.len().div_ceil(2);
    let per_side = top.max(1) as f32;
    let w = (per_side * 6.0)
        .max(ENVELOPE)
        .max(label.chars().count() as f32 * LABEL * 0.6 + 2.0);
    let h = ENVELOPE / 2.0;
    d.primitives.push(rect(-w / 2.0, -h, w, ENVELOPE));
    if !label.is_empty() {
        d.primitives.push(text([0.0, 0.0], label, Align::Middle));
    }
    for (side, names) in [(-1.0, &ports[..top]), (1.0, &ports[top..])] {
        let n = names.len() as f32;
        for (i, name) in names.iter().enumerate() {
            let x = -w / 2.0 + w * (i as f32 + 0.5) / n;
            let (inner, edge) = (side * h, side * (h + STUB));
            d.primitives.push(line([x, inner], [x, edge]));
            d.primitives
                .push(text([x + 1.0, edge - side * 1.0], name, Align::Start));
            d.ports.push((name.clone(), [x, edge]));
        }
    }
    d
}

/// Vertical two-port symbol: port lines from `inner` to the anchors above and below.
fn vertical_ports(d: &mut Drawing, top: &str, bottom: &str, inner: f32) {
    let edge = inner + STUB;
    d.primitives.push(line([0.0, -inner], [0.0, -edge]));
    d.primitives.push(line([0.0, inner], [0.0, edge]));
    d.primitives
        .push(text([1.0, -edge + 1.0], top, Align::Start));
    d.primitives
        .push(text([1.0, edge - 1.0], bottom, Align::Start));
    d.ports.push((top.to_string(), [0.0, -edge]));
    d.ports.push((bottom.to_string(), [0.0, edge]));
}

fn directional_valve(v: &DirectionalValve) -> Drawing {
    let mut d = Drawing::default();
    let s = ENVELOPE;
    let n = v.positions.len();
    // Shift so the rest envelope is centred on the origin.
    let ox = -(v.normal_position as f32 * s + s / 2.0);
    let oy = -s / 2.0;

    // Port x offset inside an envelope, by side and order along that side.
    let offset = |name: &str| -> Option<(f32, Side)> {
        let port = v.ports.iter().find(|p| p.name == name)?;
        let same: Vec<_> = v.ports.iter().filter(|p| p.side == port.side).collect();
        let j = same.iter().position(|p| p.name == name)?;
        Some(((j + 1) as f32 * s / (same.len() + 1) as f32, port.side))
    };
    let y_of = |side: Side| if side == Side::Top { oy } else { oy + s };

    for (i, pos) in v.positions.iter().enumerate() {
        let x0 = ox + i as f32 * s;
        d.primitives.push(rect(x0, oy, s, s));
        let at = |name: &str| offset(name).map(|(x, side)| ([x0 + x, y_of(side)], side));

        for (pair, arrow) in pos
            .flows
            .iter()
            .map(|p| (p, true))
            .chain(pos.links.iter().map(|p| (p, false)))
        {
            let (Some((a, sa)), Some((b, sb))) = (at(&pair[0]), at(&pair[1])) else {
                continue;
            };
            if sa == sb {
                // Both ports on one side: a U dipping into the envelope.
                let dip = if sa == Side::Top { s * 0.4 } else { -s * 0.4 };
                let (a2, b2) = ([a[0], a[1] + dip], [b[0], b[1] + dip]);
                d.primitives.push(open(vec![a, a2, b2, b]));
                if arrow {
                    d.primitives.push(arrow_head(b2, b));
                }
            } else {
                d.primitives.push(line(a, b));
                if arrow {
                    d.primitives.push(arrow_head(a, b));
                }
            }
        }
        for name in &pos.blocked {
            let Some((p, side)) = at(name) else { continue };
            let into = if side == Side::Top { 2.5 } else { -2.5 };
            let end = [p[0], p[1] + into];
            d.primitives.push(line(p, end));
            d.primitives
                .push(line([end[0] - 1.5, end[1]], [end[0] + 1.5, end[1]]));
        }
    }

    // Port lines and anchors on the rest envelope.
    let x0 = ox + v.normal_position as f32 * s;
    for port in &v.ports {
        let Some((x, side)) = offset(&port.name) else {
            continue;
        };
        let (inner, outer) = match side {
            Side::Top => (oy, oy - STUB),
            Side::Bottom => (oy + s, oy + s + STUB),
        };
        let px = x0 + x;
        d.primitives.push(line([px, inner], [px, outer]));
        let label_y = if side == Side::Top {
            outer + 1.0
        } else {
            outer - 1.0
        };
        d.primitives
            .push(text([px + 0.6, label_y], &port.name, Align::Start));
        d.ports.push((port.name.clone(), [px, outer]));
    }

    let left_edge = ox;
    let right_edge = ox + n as f32 * s;
    for (j, a) in v.left.iter().enumerate() {
        actuator(&mut d, *a, left_edge - (j + 1) as f32 * ACTUATOR, oy, true);
    }
    for (j, a) in v.right.iter().enumerate() {
        actuator(&mut d, *a, right_edge + j as f32 * ACTUATOR, oy, false);
    }
    d
}

/// One actuator box starting at `x`, beside an envelope whose top is `oy`.
fn actuator(d: &mut Drawing, kind: Actuator, x: f32, oy: f32, left: bool) {
    let w = ACTUATOR;
    let h = ENVELOPE * 0.6;
    let y = oy + (ENVELOPE - h) / 2.0;
    let mid = y + h / 2.0;
    // Edge facing the envelope, and the far edge.
    let (near, far) = if left { (x + w, x) } else { (x, x + w) };
    match kind {
        Actuator::Solenoid => {
            d.primitives.push(rect(x, y, w, h));
            d.primitives
                .push(line([x + w * 0.25, y + h], [x + w * 0.75, y]));
        }
        Actuator::Spring => d
            .primitives
            .push(spring([near, mid], [far, mid], 3, h * 0.35)),
        Actuator::Lever => {
            d.primitives.push(line([near, mid], [far, y - 1.0]));
            d.primitives.push(Primitive::Circle {
                center: [far, y - 1.0],
                radius: 0.6,
                filled: true,
            });
        }
        Actuator::Pilot => {
            let tip = [near, mid];
            d.primitives.push(dashed(vec![[far, mid], tip]));
            d.primitives.push(arrow_head([far, mid], tip));
        }
        Actuator::Detent => {
            d.primitives.push(rect(x, y + h * 0.3, w, h * 0.4));
            d.primitives.push(open(vec![
                [x + w * 0.3, y + h * 0.3],
                [x + w * 0.5, mid],
                [x + w * 0.7, y + h * 0.3],
            ]));
        }
    }
}

/// Check valve, free flow upward from A (bottom) to B (top).
fn check(has_spring: bool) -> Drawing {
    let mut d = Drawing::default();
    let r = 1.8;
    let ball = [0.0, -0.5];
    d.primitives.push(Primitive::Circle {
        center: ball,
        radius: r,
        filled: false,
    });
    // Seat: a V under the ball on the A side.
    d.primitives
        .push(open(vec![[-2.8, -1.0], [0.0, 3.0], [2.8, -1.0]]));
    d.primitives.push(line([0.0, 3.0], [0.0, 5.0]));
    if has_spring {
        d.primitives
            .push(spring([0.0, ball[1] - r], [0.0, -5.0], 2, 1.2));
    } else {
        d.primitives.push(line([0.0, ball[1] - r], [0.0, -5.0]));
    }
    vertical_ports(&mut d, "B", "A", 5.0);
    d
}

/// Pressure relief valve, P at the bottom, T at the top, normally closed.
fn relief(adj: bool) -> Drawing {
    let mut d = Drawing::default();
    let h = ENVELOPE / 2.0;
    d.primitives.push(rect(-h, -h, ENVELOPE, ENVELOPE));
    // Flow arrow offset from the port line: closed until pilot pressure opens it.
    let (from, tip) = ([-1.5, h - 1.0], [-1.5, -h + 1.0]);
    d.primitives.push(line(from, tip));
    d.primitives.push(arrow_head(from, tip));
    d.primitives.push(line([0.0, h], [0.0, h - 1.0]));
    d.primitives.push(line([0.0, -h], [0.0, -h + 1.0]));
    // Spring on the right pushing the valve shut.
    d.primitives.push(spring([h, 0.0], [h + 5.0, 0.0], 3, 1.5));
    if adj {
        d.primitives.extend(adjustable([h + 2.5, 0.0], 2.5));
    }
    // Pilot line from P to the left end.
    d.primitives.push(dashed(vec![
        [0.0, h + 1.5],
        [-h - 2.0, h + 1.5],
        [-h - 2.0, 0.0],
        [-h, 0.0],
    ]));
    d.primitives.push(arrow_head([-h - 2.0, 0.0], [-h, 0.0]));
    vertical_ports(&mut d, "T", "P", h);
    d
}

/// Fixed or adjustable orifice between A (bottom) and B (top).
fn orifice(adj: bool) -> Drawing {
    let mut d = Drawing::default();
    d.primitives.push(line([0.0, -4.0], [0.0, 4.0]));
    // Two arcs pinching the line.
    d.primitives.push(arc([-3.2, 0.0], 2.6, -50.0, 50.0));
    d.primitives.push(arc([3.2, 0.0], 2.6, 130.0, 230.0));
    if adj {
        d.primitives.extend(adjustable([0.0, 0.0], 3.5));
    }
    vertical_ports(&mut d, "B", "A", 4.0);
    d
}

/// External connection, e.g. manifold port P.
fn port_marker(label: &str) -> Drawing {
    Drawing {
        primitives: vec![
            Primitive::Circle {
                center: [0.0, 0.0],
                radius: 1.2,
                filled: false,
            },
            text([0.0, -2.5], label, Align::Middle),
        ],
        ports: vec![(PORT_PIN.to_string(), [0.0, 0.0])],
    }
}

fn rotate(p: Point, r: Rotation) -> Point {
    let [x, y] = p;
    // Clockwise on a y-down sheet.
    match r {
        Rotation::R0 => [x, y],
        Rotation::R90 => [-y, x],
        Rotation::R180 => [-x, -y],
        Rotation::R270 => [y, -x],
    }
}

/// Bounding box `(min, max)` of the primitives; text counts as its estimated extent.
pub fn bounds(primitives: &[Primitive]) -> Option<(Point, Point)> {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut add = |p: Point| {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    };
    for prim in primitives {
        match prim {
            Primitive::Polyline { points, .. } => points.iter().for_each(|p| add(*p)),
            Primitive::Circle {
                center: [x, y],
                radius: r,
                ..
            } => {
                add([x - r, y - r]);
                add([x + r, y + r]);
            }
            Primitive::Text {
                at: [x, y],
                text,
                size,
                align,
            } => {
                let w = text.chars().count() as f32 * size * 0.6;
                let x0 = match align {
                    Align::Start => *x,
                    Align::Middle => x - w / 2.0,
                    Align::End => x - w,
                };
                add([x0, y - size / 2.0]);
                add([x0 + w, y + size / 2.0]);
            }
        }
    }
    (min[0] <= max[0]).then_some((min, max))
}

/// Draw one component at its placement, with port anchors named by port.
///
/// `None` when a valve's catalogue rows are missing; [`Circuit::validate`] reports those.
pub fn render_component(c: &Component, place: Placement, catalogue: &Catalogue) -> Option<Drawing> {
    let symbol = match &c.kind {
        ComponentKind::Valve(v) => render_symbol(v.symbol(catalogue)?),
        ComponentKind::Port { label } => port_marker(label),
        ComponentKind::Item(item) => render_symbol(&item.symbol),
    };
    let at = |[x, y]: Point| {
        let flipped = if place.mirror { [-x, y] } else { [x, y] };
        let [x, y] = rotate(flipped, place.rotation);
        [x + place.x, y + place.y]
    };
    let mut d = Drawing::default();
    for prim in symbol.primitives {
        d.primitives.push(match prim {
            Primitive::Polyline {
                points,
                closed,
                dashed,
                filled,
            } => Primitive::Polyline {
                points: points.into_iter().map(at).collect(),
                closed,
                dashed,
                filled,
            },
            Primitive::Circle {
                center,
                radius,
                filled,
            } => Primitive::Circle {
                center: at(center),
                radius,
                filled,
            },
            // Text stays upright, only its anchor moves; flip its side when turned over.
            Primitive::Text {
                at: p,
                text,
                size,
                align,
            } => {
                // A mirror and a half turn each swap the side; both together cancel out.
                let align = match (place.mirror != (place.rotation == Rotation::R180), align) {
                    (true, Align::Start) => Align::End,
                    (true, Align::End) => Align::Start,
                    _ => align,
                };
                Primitive::Text {
                    at: at(p),
                    text,
                    size,
                    align,
                }
            }
        });
    }
    if !matches!(c.kind, ComponentKind::Port { .. })
        && let Some((min, max)) = bounds(&d.primitives)
    {
        // Designator beside the symbol's top-left corner, clear of its port lines.
        d.primitives.push(text(
            [min[0] - 1.0, min[1] + LABEL / 2.0],
            &c.id,
            Align::End,
        ));
        let mut below = max[1] + LABEL;
        if let ComponentKind::Item(item) = &c.kind
            && let Some(label) = item.label.as_deref().filter(|l| !l.is_empty())
        {
            d.primitives
                .push(text([(min[0] + max[0]) / 2.0, below], label, Align::Middle));
            below += LABEL * 1.4;
        }
        if let Some(words) = status_words(c, catalogue) {
            // A dashed frame and the status in words, so a black-and-white print shows it too.
            let pad = 1.5;
            d.primitives.push(Primitive::Polyline {
                points: vec![
                    [min[0] - pad, min[1] - pad],
                    [max[0] + pad, min[1] - pad],
                    [max[0] + pad, max[1] + pad],
                    [min[0] - pad, max[1] + pad],
                ],
                closed: true,
                dashed: true,
                filled: false,
            });
            d.primitives.push(text(
                [(min[0] + max[0]) / 2.0, below + pad],
                &words,
                Align::Middle,
            ));
        }
    }
    for (name, p) in symbol.ports {
        d.ports.push((name, at(p)));
    }
    Some(d)
}

/// What a valve that isn't normal shows in words, e.g. `MISSING DEMO-X` or
/// `STAND-IN for DEMO-X`.
pub fn status_words(c: &Component, catalogue: &Catalogue) -> Option<String> {
    let status = c.valve_status(catalogue)?;
    let code = c
        .valve()
        .and_then(|v| v.requested.as_ref())
        .map(|r| r.code.trim())
        .unwrap_or_default();
    match status {
        ValveStatus::Normal => None,
        ValveStatus::StandIn => Some(format!("STAND-IN for {code}")),
        s => Some(format!("{} {code}", s.words())),
    }
}

/// Draw a whole circuit: every placed component, its designator, and its nets.
///
/// Components without a placement sit at the origin. Components whose catalogue rows are
/// missing are skipped; [`Circuit::validate`] reports them.
pub fn render_circuit(circuit: &Circuit, catalogue: &Catalogue) -> Drawing {
    let mut d = Drawing::default();
    for c in &circuit.components {
        let place = circuit
            .layout
            .components
            .get(&c.id)
            .copied()
            .unwrap_or_default();
        let Some(part) = render_component(c, place, catalogue) else {
            continue;
        };
        d.primitives.extend(part.primitives);
        for (name, p) in part.ports {
            d.ports.push((format!("{}.{name}", c.id), p));
        }
    }

    let anchor = |c: &str, p: &str| {
        let key = format!("{c}.{p}");
        d.ports.iter().find(|(k, _)| *k == key).map(|(_, p)| *p)
    };
    let mut wires = Vec::new();
    for net in &circuit.nets {
        if let Some(drawn) = circuit.layout.wires.get(&net.id) {
            wires.extend(drawn.iter().map(|w| open(w.clone())));
            // Junction dots where three or more lines meet: at wire ends, and at ports, which
            // a wire may pass over (a test point on a straight line).
            let ports: Vec<Point> = (net.ports.iter())
                .filter_map(|p| anchor(&p.component, &p.port))
                .collect();
            let mut dots: Vec<Point> = Vec::new();
            for at in drawn
                .iter()
                .flat_map(|w| ends(w))
                .chain(ports.iter().copied())
            {
                if !dots.iter().any(|d| near(*d, at)) && arms(at, drawn, &ports) >= 3 {
                    dots.push(at);
                }
            }
            wires.extend(dots.into_iter().map(|center| Primitive::Circle {
                center,
                radius: 0.6,
                filled: true,
            }));
            continue;
        }
        let points: Vec<Point> = net
            .ports
            .iter()
            .filter_map(|p| anchor(&p.component, &p.port))
            .collect();
        let Some((&first, rest)) = points.split_first() else {
            continue;
        };
        // Star from the first port: across along a trunk, then straight into each port.
        for &p in rest {
            let elbow = [p[0], first[1]];
            wires.push(open(vec![first, elbow, p]));
            // Junction dot where the trunk carries on past this branch.
            let further = |q: &Point| {
                (q[0] - first[0]).signum() == (p[0] - first[0]).signum()
                    && (q[0] - first[0]).abs() > (p[0] - first[0]).abs()
            };
            if p[1] != first[1] && rest.iter().any(further) {
                wires.push(Primitive::Circle {
                    center: elbow,
                    radius: 0.6,
                    filled: true,
                });
            }
        }
        let left = rest.iter().any(|q| q[0] < first[0]);
        let right = rest.iter().any(|q| q[0] > first[0]);
        if left && right {
            wires.push(Primitive::Circle {
                center: first,
                radius: 0.6,
                filled: true,
            });
        }
    }
    d.primitives.extend(wires);
    if let Some((min, max)) = block_outline(circuit, catalogue) {
        let corners = [min, [max[0], min[1]], max, [min[0], max[1]]];
        d.primitives.extend(dash_dot(&corners));
    }
    d
}

/// Space between the block's outline and the parts inside it.
const OUTLINE_PAD: f32 = 4.0;

/// The block's outline `(min, max)`: around every part in the block, through the block's ports.
/// `None` when no drawn component is in the block.
pub fn block_outline(circuit: &Circuit, catalogue: &Catalogue) -> Option<(Point, Point)> {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for c in circuit
        .components
        .iter()
        .filter(|c| circuit.in_block.contains(&c.id))
    {
        // Ports set the edge exactly, as block ports sit on it; parts get room around them.
        let (lo, hi) = match extent(circuit, c, catalogue) {
            Some(e) if matches!(c.kind, ComponentKind::Port { .. }) => e,
            Some((lo, hi)) => (
                [lo[0] - OUTLINE_PAD, lo[1] - OUTLINE_PAD],
                [hi[0] + OUTLINE_PAD, hi[1] + OUTLINE_PAD],
            ),
            None => continue,
        };
        min = [min[0].min(lo[0]), min[1].min(lo[1])];
        max = [max[0].max(hi[0]), max[1].max(hi[1])];
    }
    (min[0] <= max[0]).then_some((min, max))
}

/// Where a component is on the sheet: a port's point, or the box round anything else's drawing.
fn extent(circuit: &Circuit, c: &Component, catalogue: &Catalogue) -> Option<(Point, Point)> {
    let place = circuit
        .layout
        .components
        .get(&c.id)
        .copied()
        .unwrap_or_default();
    match c.kind {
        ComponentKind::Port { .. } => Some(([place.x, place.y], [place.x, place.y])),
        _ => bounds(&render_component(c, place, catalogue)?.primitives),
    }
}

/// Components outside the block that are drawn over its outline, so the schematic would show
/// them in the block.
pub fn outline_issues(circuit: &Circuit, catalogue: &Catalogue) -> Vec<Issue> {
    let Some((min, max)) = block_outline(circuit, catalogue) else {
        return Vec::new();
    };
    circuit
        .components
        .iter()
        .filter(|c| !circuit.in_block.contains(&c.id))
        .filter(|c| {
            extent(circuit, c, catalogue).is_some_and(|(lo, hi)| {
                lo[0] < max[0] && min[0] < hi[0] && lo[1] < max[1] && min[1] < hi[1]
            })
        })
        .map(|c| Issue::DrawnInsideBlock(c.id.clone()))
        .collect()
}

/// How far a wire's end may be from the port or wire it lands on, in millimetres.
const WIRE_TOLERANCE: f32 = 0.5;

fn near(a: Point, b: Point) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= WIRE_TOLERANCE
}

/// Whether `p` lies on the polyline `w`.
fn on_wire(p: Point, w: &[Point]) -> bool {
    w.windows(2).any(|s| {
        let (a, d) = (s[0], [s[1][0] - s[0][0], s[1][1] - s[0][1]]);
        let len2 = d[0] * d[0] + d[1] * d[1];
        let t = if len2 > 0.0 {
            (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        near(p, [a[0] + t * d[0], a[1] + t * d[1]])
    })
}

fn ends(w: &[Point]) -> impl Iterator<Item = Point> + '_ {
    w.first().into_iter().chain(w.last()).copied()
}

/// Lines meeting at `p`: one per wire end or port there, two per wire passing through.
fn arms(p: Point, wires: &[Vec<Point>], ports: &[Point]) -> usize {
    let through = |w: &Vec<Point>| if ends(w).any(|e| near(e, p)) { 1 } else { 2 };
    let wires: usize = wires.iter().filter(|w| on_wire(p, w)).map(through).sum();
    wires + ports.iter().filter(|q| near(**q, p)).count()
}

/// Ports' names (`component.port`) and positions.
type Ports = Vec<(String, Point)>;

/// A hand-drawn net, its wires, and its ports.
struct DrawnNet<'a> {
    net: &'a Net,
    wires: &'a [Vec<Point>],
    ports: Ports,
}

/// The hand-drawn nets, and where every drawn port is. `None` when `import_problems` refuses
/// the wires anyway, which also keeps the work here bounded. A net with a port that can't be
/// drawn is left out, as validation reports that part.
fn drawn_nets<'a>(
    circuit: &'a Circuit,
    catalogue: &Catalogue,
) -> Option<(Vec<DrawnNet<'a>>, Ports)> {
    if !circuit.layout.wires_are_bounded() {
        return None;
    }
    let anchors = render_circuit(circuit, catalogue).ports;
    let nets = (circuit.nets.iter())
        .filter_map(|net| {
            let wires = circuit.layout.wires.get(&net.id)?;
            let ports: Ports = (net.ports.iter())
                .filter_map(|p| {
                    let key = p.to_string();
                    let (_, at) = anchors.iter().find(|(k, _)| *k == key)?;
                    Some((key, *at))
                })
                .collect();
            (ports.len() == net.ports.len()).then_some(DrawnNet { net, wires, ports })
        })
        .collect();
    Some((nets, anchors))
}

/// Hand-drawn wires that don't join their net's ports up, or touch another net, in plain words
/// saying how to fix them.
pub fn wire_problems(circuit: &Circuit, catalogue: &Catalogue) -> Vec<String> {
    let Some((nets, anchors)) = drawn_nets(circuit, catalogue) else {
        return Vec::new();
    };
    let mut problems = Vec::new();
    for (a, d) in nets.iter().enumerate() {
        let (net, wires, ports) = (&d.net.id, d.wires, &d.ports);
        let at = |(k, [x, y]): &(String, Point)| format!("{k} at {x}, {y}");
        for (i, w) in wires.iter().enumerate() {
            for [x, y] in ends(w) {
                let lands = ports.iter().any(|(_, p)| near(*p, [x, y]))
                    || (wires.iter().enumerate()).any(|(j, o)| j != i && on_wire([x, y], o));
                if !lands {
                    problems.push(format!(
                        "wire {} of net {net} ends at {x}, {y}, on no port or other wire of the \
                         net; end it on one (the net's ports: {})",
                        i + 1,
                        ports.iter().map(at).collect::<Vec<_>>().join("; ")
                    ));
                }
            }
        }
        // Wires joined where one ends on another or both reach a port; a net is one piece.
        let n = wires.len();
        let mut group: Vec<usize> = (0..n + ports.len()).collect();
        fn root(g: &mut [usize], mut i: usize) -> usize {
            while g[i] != i {
                i = g[i];
            }
            i
        }
        fn join(g: &mut [usize], a: usize, b: usize) {
            let (a, b) = (root(g, a), root(g, b));
            g[a] = b;
        }
        let mut reached = vec![false; ports.len()];
        for (i, w) in wires.iter().enumerate() {
            for (j, o) in wires.iter().enumerate() {
                if i != j && ends(w).any(|e| on_wire(e, o)) {
                    join(&mut group, i, j);
                }
            }
            for (k, (_, p)) in ports.iter().enumerate() {
                if on_wire(*p, w) {
                    reached[k] = true;
                    join(&mut group, i, n + k);
                }
            }
        }
        for (k, port) in ports.iter().enumerate() {
            if !reached[k] {
                problems.push(format!(
                    "port {} is on net {net}, but none of the net's wires reaches it; draw one \
                     to it",
                    at(port)
                ));
            }
        }
        let mut pieces: std::collections::BTreeMap<usize, Vec<String>> = Default::default();
        for i in 0..n {
            let r = root(&mut group, i);
            pieces.entry(r).or_default().push((i + 1).to_string());
        }
        if pieces.len() > 1 {
            let list: Vec<String> = (pieces.values())
                .map(|p| format!("wires {}", p.join(", ")))
                .collect();
            problems.push(format!(
                "the wires of net {net} are drawn in {} separate pieces ({}); join them by \
                 ending a wire on another, as wires that only cross are not joined",
                pieces.len(),
                list.join("; ")
            ));
        }
        // Other nets: crossing a wire at a point is fine, touching anything else is not.
        for (i, w) in wires.iter().enumerate() {
            let what = format!("wire {} of net {net}", i + 1);
            for (key, p) in &anchors {
                if ports.iter().any(|(k, _)| k == key) || !on_wire(*p, w) {
                    continue;
                }
                let on = circuit
                    .nets
                    .iter()
                    .find(|n| n.ports.iter().any(|q| q.to_string() == *key))
                    .map_or("on no net".to_string(), |n| format!("on net {}", n.id));
                problems.push(format!(
                    "{what} passes over port {key}, which is {on}; route it around"
                ));
            }
            // Each pair of nets once, from the first of the two.
            for o in &nets[a + 1..] {
                for (j, ow) in o.wires.iter().enumerate() {
                    let touch = (w.iter().copied().find(|p| on_wire(*p, ow)))
                        .or_else(|| ow.iter().copied().find(|p| on_wire(*p, w)));
                    if let Some([x, y]) = touch {
                        problems.push(format!(
                            "{what} and wire {} of net {} touch at {x}, {y}; a wire may cross \
                             another net's wire, but never end, turn or run along it, so move \
                             whichever differs from the drawing",
                            j + 1,
                            o.net.id
                        ));
                    }
                }
            }
        }
    }
    if problems.len() > MAX_LISTED {
        let more = problems.len() - MAX_LISTED;
        problems.truncate(MAX_LISTED);
        problems.push(format!("and {more} more wire problems like these"));
    }
    problems
}

/// Hand-drawn wires that run through a part's symbol, other than into its own ports. Usually a
/// misread route.
pub fn wire_warnings(circuit: &Circuit, catalogue: &Catalogue) -> Vec<String> {
    let Some((nets, _)) = drawn_nets(circuit, catalogue) else {
        return Vec::new();
    };
    // Each part's symbol box: no designator, status words or status frame.
    let parts: Vec<(&Component, Point, Point)> = (circuit.components.iter())
        .filter_map(|c| {
            let place = circuit.layout.components.get(&c.id).copied();
            let drawing = render_component(c, place.unwrap_or_default(), catalogue)?;
            let symbol: Vec<Primitive> = (drawing.primitives.into_iter())
                .filter(|p| !matches!(p, Primitive::Text { .. }))
                .filter(|p| !matches!(p, Primitive::Polyline { dashed: true, .. }))
                .collect();
            let (lo, hi) = bounds(&symbol)?;
            Some((c, lo, hi))
        })
        .collect();
    let mut warnings = Vec::new();
    for d in &nets {
        for &(c, lo, hi) in &parts {
            // A segment into one of the part's ports on this net enters its symbol rightly.
            // (`ports` lines up with the net's ports, as nets with a port not drawn are skipped.)
            let own: Vec<Point> = (d.net.ports.iter().zip(&d.ports))
                .filter(|(p, _)| p.component == c.id)
                .map(|(_, (_, at))| *at)
                .collect();
            // ponytail: a segment's box against the symbol's box, exact for the horizontal and
            // vertical segments imports are held to.
            let through = (d.wires.iter().flat_map(|w| w.windows(2)))
                .filter(|s| !own.iter().any(|p| near(*p, s[0]) || near(*p, s[1])))
                .any(|s| {
                    (0..2).all(|k| {
                        s[0][k].min(s[1][k]) < hi[k] - WIRE_TOLERANCE
                            && s[0][k].max(s[1][k]) > lo[k] + WIRE_TOLERANCE
                    })
                });
            if through {
                warnings.push(format!(
                    "a wire of net {} runs through {}; check its route against the drawing",
                    d.net.id, c.id
                ));
            }
        }
    }
    warnings
}

/// Longest outline drawn dash-dot; a longer one (a part placed far off by mistake) is drawn
/// dashed, so a typo can't make millions of dashes.
const MAX_DASH_DOT: f32 = 20_000.0;

/// A closed dash-dot line through `corners`, the ISO 1219 boundary of a block or assembly.
fn dash_dot(corners: &[Point]) -> Vec<Primitive> {
    let perimeter: f32 = (0..corners.len())
        .map(|k| {
            let (a, b) = (corners[k], corners[(k + 1) % corners.len()]);
            ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
        })
        .sum();
    if perimeter.is_nan() || perimeter > MAX_DASH_DOT {
        return vec![Primitive::Polyline {
            points: corners.to_vec(),
            closed: true,
            dashed: true,
            filled: false,
        }];
    }
    const PATTERN: [(f32, bool); 4] = [(4.0, true), (1.0, false), (0.3, true), (1.0, false)];
    let (mut i, mut used) = (0, 0.0);
    let mut out = Vec::new();
    for (k, &a) in corners.iter().enumerate() {
        let b = corners[(k + 1) % corners.len()];
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let at = |t: f32| {
            [
                a[0] + (b[0] - a[0]) * t / len,
                a[1] + (b[1] - a[1]) * t / len,
            ]
        };
        let mut pos = 0.0;
        while pos < len {
            let (dash, on) = PATTERN[i];
            let step = (dash - used).min(len - pos);
            if on {
                out.push(line(at(pos), at(pos + step)));
            }
            pos += step;
            used += step;
            if used >= dash {
                (i, used) = ((i + 1) % PATTERN.len(), 0.0);
            }
        }
    }
    out
}
