//! ISO 1219 symbols as plain drawing primitives.
//!
//! [`render_symbol`] turns a symbol definition into polylines, circles and text in millimetres
//! (x right, y down), with an anchor point per port. [`render_circuit`] places symbols and draws
//! the nets. The canvas, SVG and PDF export all draw the same primitives.

use hydra_core::{
    Actuator, Catalogue, Circuit, Component, ComponentKind, DirectionalValve, FlowActuator, Issue,
    PORT_PIN, Placement, Rotation, Side, SymbolDef, ValveStatus,
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

const PORT_RADIUS: f32 = 1.2;

/// External connection, e.g. manifold port P.
fn port_marker(label: &str) -> Drawing {
    Drawing {
        primitives: vec![
            Primitive::Circle {
                center: [0.0, 0.0],
                radius: PORT_RADIUS,
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
    draw_component(c, place, catalogue, &[])
}

/// [`render_component`], with the designator clear of `wires` where it can be.
fn draw_component(
    c: &Component,
    place: Placement,
    catalogue: &Catalogue,
    wires: &[Primitive],
) -> Option<Drawing> {
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
    for (name, p) in symbol.ports {
        d.ports.push((name, at(p)));
    }
    if !matches!(c.kind, ComponentKind::Port { .. })
        && let Some((min, max)) = bounds(&d.primitives)
    {
        let status = status_words(c, catalogue);
        let pad = 1.5;
        // Outside the status frame, when there is one.
        let gap = if status.is_some() { pad + 1.0 } else { 1.0 };
        let mid = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
        // Designator beside the middle of a side no port leaves from, so no wire of this part
        // starts there: left, right, then top, skipping a side any of `wires` crosses.
        // ponytail: when ports or wires take every one of those sides it goes on the first free
        // one, or on the left, even over a wire; a crowded layout needs a part moved.
        let side_of = |[x, y]: Point| {
            let edges = [x - min[0], max[0] - x, y - min[1], max[1] - y];
            (0..4).min_by(|&a, &b| edges[a].total_cmp(&edges[b]))
        };
        let taken: Vec<_> = d.ports.iter().map(|(_, p)| side_of(*p)).collect();
        let label = |side| match side {
            1 => text([max[0] + gap, mid[1]], &c.id, Align::Start),
            2 => text([mid[0], min[1] - gap - LABEL / 2.0], &c.id, Align::Middle),
            _ => text([min[0] - gap, mid[1]], &c.id, Align::End),
        };
        let free: Vec<_> = (0..3)
            .filter(|s| !taken.contains(&Some(*s)))
            .map(label)
            .collect();
        let clear = |l: &&Primitive| {
            let (lo, hi) = bounds(std::slice::from_ref(*l)).unwrap_or_default();
            !wires.iter().any(|w| crosses(w, lo, hi))
        };
        let designator = free.iter().find(clear).or(free.first()).cloned();
        d.primitives.push(designator.unwrap_or_else(|| label(0)));
        let mut below = max[1] + LABEL;
        if let ComponentKind::Item(item) = &c.kind
            && let Some(label) = item.label.as_deref().filter(|l| !l.is_empty())
        {
            d.primitives
                .push(text([(min[0] + max[0]) / 2.0, below], label, Align::Middle));
            below += LABEL * 1.4;
        }
        if let Some(words) = status {
            // A dashed frame and the status in words, so a black-and-white print shows it too.
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
    let (parts, wires, outline) = parts_and_wires(circuit, catalogue);
    let mut d = Drawing {
        primitives: parts.into_iter().flat_map(|(_, p)| p.primitives).collect(),
        ports: wires.ports,
    };
    d.primitives.extend(wires.primitives);
    if let Some(outline) = outline {
        let corners = outline.corners();
        let ports: Vec<Point> = circuit
            .components
            .iter()
            .filter(|c| matches!(c.kind, ComponentKind::Port { .. }))
            .map(|c| placement(circuit, &c.id))
            .map(|p| [p.x, p.y])
            .filter(|&p| outward(&corners, p).is_some())
            .collect();
        d.primitives.extend(dash_dot(&corners, &ports));
    }
    d
}

/// Each drawn part by id, exactly as [`render_circuit`] draws it: its designator keeps clear of
/// the circuit's wires, so anything measuring or hit-testing parts should use this.
pub fn render_parts(circuit: &Circuit, catalogue: &Catalogue) -> Vec<(String, Drawing)> {
    parts_and_wires(circuit, catalogue).0
}

/// The parts by id, the wires with every port anchor named `component.port`, and the block's
/// outline.
fn parts_and_wires(
    circuit: &Circuit,
    catalogue: &Catalogue,
) -> (Vec<(String, Drawing)>, Drawing, Option<Outline>) {
    let place = |c: &Component| placement(circuit, &c.id);
    let mut d = Drawing::default();
    for c in &circuit.components {
        let Some(part) = render_component(c, place(c), catalogue) else {
            continue;
        };
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
    // The parts again, now that their designators can keep clear of the wires.
    let mut parts: Vec<(String, Drawing)> = circuit
        .components
        .iter()
        .filter_map(|c| {
            Some((
                c.id.clone(),
                draw_component(c, place(c), catalogue, &wires)?,
            ))
        })
        .collect();
    d.primitives = wires;
    // A port on the block's edge has its label outside the block.
    let outline = outline(circuit, &parts);
    let corners = outline.as_ref().map(Outline::corners).unwrap_or_default();
    for (id, part) in &mut parts {
        let Some(c) = circuit.component(id) else {
            continue;
        };
        let at = [place(c).x, place(c).y];
        let (ComponentKind::Port { .. }, Some([nx, ny])) = (&c.kind, outward(&corners, at)) else {
            continue;
        };
        for prim in &mut part.primitives {
            if let Primitive::Text { at: t, align, .. } = prim {
                (*t, *align) = if nx == 0.0 {
                    ([at[0], at[1] + 2.5 * ny.signum()], Align::Middle)
                } else if nx < 0.0 {
                    ([at[0] - 2.0, at[1]], Align::End)
                } else {
                    ([at[0] + 2.0, at[1]], Align::Start)
                };
            }
        }
    }
    (parts, d, outline)
}

/// Whether a line of `p` passes through the box `lo`-`hi` (Liang-Barsky clipping per segment).
fn crosses(p: &Primitive, lo: Point, hi: Point) -> bool {
    let Primitive::Polyline { points, .. } = p else {
        return false;
    };
    points.windows(2).any(|s| {
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for k in 0..2 {
            let d = s[1][k] - s[0][k];
            for (p, q) in [(-d, s[0][k] - lo[k]), (d, hi[k] - s[0][k])] {
                if p == 0.0 {
                    if q < 0.0 {
                        return false;
                    }
                } else if p < 0.0 {
                    t0 = t0.max(q / p);
                } else {
                    t1 = t1.min(q / p);
                }
            }
        }
        t0 <= t1
    })
}

/// Space between the block's outline and the parts inside it.
const OUTLINE_PAD: f32 = 4.0;

/// Tolerance for a point lying on an outline edge, in mm.
const ON_EDGE: f32 = 1e-3;

type Rect = (Point, Point);

/// The block's outline: the corners of a closed rectilinear polygon, clockwise from the top
/// left. `None` when no drawn component is in the block.
///
/// It is the rectangle round every part in the block and through the block's ports, with a
/// corner cut out round any part outside the block that stands in a corner holding no block
/// part, so an L-shaped block keeps the part in its notch outside.
pub fn block_outline(circuit: &Circuit, catalogue: &Catalogue) -> Option<Vec<Point>> {
    Some(parts_and_wires(circuit, catalogue).2?.corners())
}

/// Whether `c`, placed at `place`, would be drawn over the block's outline as it is now.
pub fn drawn_inside_outline(
    circuit: &Circuit,
    c: &Component,
    place: Placement,
    catalogue: &Catalogue,
) -> bool {
    let Some(outline) = parts_and_wires(circuit, catalogue).2 else {
        return false;
    };
    // Not drawn yet, so measured on its own: no wire reaches it to move its designator.
    let e = match c.kind {
        ComponentKind::Port { .. } => Some(([place.x, place.y], [place.x, place.y])),
        _ => render_component(c, place, catalogue).and_then(|d| bounds(&d.primitives)),
    };
    e.is_some_and(|e| outline.covers(e))
}

/// The block's bounding rectangle and the corners cut out of it, top left, top right, bottom
/// right, bottom left.
struct Outline {
    rect: Rect,
    notches: [Option<Rect>; 4],
}

impl Outline {
    /// Whether a drawing's extent reaches over the outline's interior.
    fn covers(&self, e: Rect) -> bool {
        let (lo, hi) = self.rect;
        let clipped = (
            [e.0[0].max(lo[0]), e.0[1].max(lo[1])],
            [e.1[0].min(hi[0]), e.1[1].min(hi[1])],
        );
        overlaps(e, self.rect)
            && !self.notches.iter().flatten().any(|n| {
                n.0[0] <= clipped.0[0]
                    && n.0[1] <= clipped.0[1]
                    && clipped.1[0] <= n.1[0]
                    && clipped.1[1] <= n.1[1]
            })
    }

    fn corners(&self) -> Vec<Point> {
        let mut corners = Vec::new();
        for (k, notch) in self.notches.iter().enumerate() {
            let c = flip(k, self.rect).0;
            // With the corner flipped to the top left, a notch to `i` goes down, across, up.
            let i = notch.map_or(c, |n| flip(k, n).1);
            let mut three = [[c[0], i[1]], i, [i[0], c[1]]].map(|p| flip(k, (p, p)).0);
            // One flip turns the polygon anticlockwise; walk it back.
            if k % 2 == 1 {
                three.reverse();
            }
            corners.extend(three);
        }
        corners.dedup();
        corners
    }
}

/// `r` mirrored so corner `k` of it (top left, top right, bottom right, bottom left) becomes its
/// minimum; its own inverse.
fn flip(k: usize, (lo, hi): Rect) -> Rect {
    let axis = |lo: f32, hi: f32, flipped: bool| if flipped { (-hi, -lo) } else { (lo, hi) };
    let (x0, x1) = axis(lo[0], hi[0], k == 1 || k == 2);
    let (y0, y1) = axis(lo[1], hi[1], k >= 2);
    ([x0, y0], [x1, y1])
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.0[0] < b.1[0] && b.0[0] < a.1[0] && a.0[1] < b.1[1] && b.0[1] < a.1[1]
}

/// The outline round the block's parts, measured in `parts` from [`render_parts`].
fn outline(circuit: &Circuit, parts: &[(String, Drawing)]) -> Option<Outline> {
    let (mut members, mut others) = (Vec::new(), Vec::new());
    for c in &circuit.components {
        let Some((lo, hi)) = extent(circuit, c, parts) else {
            continue;
        };
        match (circuit.in_block.contains(&c.id), &c.kind) {
            (false, _) => others.push((lo, hi)),
            // Ports set the edge exactly, as block ports sit on it; parts get room around them.
            (true, ComponentKind::Port { .. }) => members.push((lo, hi)),
            (true, _) => members.push((
                [lo[0] - OUTLINE_PAD, lo[1] - OUTLINE_PAD],
                [hi[0] + OUTLINE_PAD, hi[1] + OUTLINE_PAD],
            )),
        }
    }
    let rect = members.iter().copied().reduce(|(a, b), (c, d)| {
        (
            [a[0].min(c[0]), a[1].min(c[1])],
            [b[0].max(d[0]), b[1].max(d[1])],
        )
    })?;
    // An outside part with block parts on all four sides is more likely left out of the block
    // by mistake than standing in a notch, so it is flagged instead.
    let centre = |r: Rect| [(r.0[0] + r.1[0]) / 2.0, (r.0[1] + r.1[1]) / 2.0];
    let surrounded = |o: Rect| {
        let o = centre(o);
        let side = |axis: usize, sign: f32| {
            members
                .iter()
                .any(|&m| (centre(m)[axis] - o[axis]) * sign > 0.0)
        };
        side(0, 1.0) && side(0, -1.0) && side(1, 1.0) && side(1, -1.0)
    };
    let mut notches = [None; 4];
    for k in 0..4 {
        // Flipped so the corner is the top left, a notch runs from `c` to its inner corner.
        let (c, far) = flip(k, rect);
        let fits = |n: Point| {
            // It leaves the far edges, no block part reaches into it, nor a port onto its outer
            // edges...
            n[0] < far[0]
                && n[1] < far[1]
                && members.iter().all(|&m| {
                    let lo = flip(k, m).0;
                    !(lo[0] < n[0] && lo[1] < n[1])
                })
                && notches.iter().flatten().all(|&o: &Rect| {
                    // ...and it keeps clear of the other notches, so the outline stays one loop.
                    let lo = flip(k, o).0;
                    !(lo[0] <= n[0] && lo[1] <= n[1])
                })
        };
        // ponytail: parts are taken greedily in drawing order, so two outside parts that each
        // fit a notch but not together keep the first; a staircase outline would fit both.
        let mut inner = c;
        for &o in others
            .iter()
            .filter(|&&o| overlaps(o, rect) && !surrounded(o))
        {
            let hi = flip(k, o).1;
            let want = [
                inner[0].max(hi[0].min(far[0])),
                inner[1].max(hi[1].min(far[1])),
            ];
            if fits(want) {
                inner = want;
            }
        }
        if inner != c {
            // Room round the outside part too, where the block's parts allow it.
            let p = OUTLINE_PAD;
            let inner = [[p, p], [p, 0.0], [0.0, p], [0.0, 0.0]]
                .map(|[dx, dy]| [inner[0] + dx, inner[1] + dy])
                .into_iter()
                .find(|&n| fits(n))
                .unwrap_or(inner);
            notches[k] = Some(flip(k, (c, inner)));
        }
    }
    Some(Outline { rect, notches })
}

/// Where a component is placed; the origin when it has no placement.
fn placement(circuit: &Circuit, id: &str) -> Placement {
    circuit
        .layout
        .components
        .get(id)
        .copied()
        .unwrap_or_default()
}

/// Where a component is on the sheet: a port's point, or the box round anything else's drawing.
/// `parts` comes from [`render_parts`], so a designator counts where it is drawn.
fn extent(circuit: &Circuit, c: &Component, parts: &[(String, Drawing)]) -> Option<Rect> {
    match c.kind {
        ComponentKind::Port { .. } => {
            let place = placement(circuit, &c.id);
            Some(([place.x, place.y], [place.x, place.y]))
        }
        _ => bounds(&parts.iter().find(|(id, _)| *id == c.id)?.1.primitives),
    }
}

/// Components outside the block that are drawn over its outline, so the schematic would show
/// them in the block.
pub fn outline_issues(circuit: &Circuit, catalogue: &Catalogue) -> Vec<Issue> {
    outside_parts(circuit, catalogue, false)
        .into_iter()
        .map(Issue::DrawnInsideBlock)
        .collect()
}

/// Components outside the block that the outline has a corner cut out round.
pub fn notched_parts(circuit: &Circuit, catalogue: &Catalogue) -> Vec<String> {
    outside_parts(circuit, catalogue, true)
}

/// Ids of the components outside the block that reach into its bounding rectangle: those in a
/// notch, or those drawn over the outline.
fn outside_parts(circuit: &Circuit, catalogue: &Catalogue, notched: bool) -> Vec<String> {
    let (parts, _, Some(outline)) = parts_and_wires(circuit, catalogue) else {
        return Vec::new();
    };
    circuit
        .components
        .iter()
        .filter(|c| !circuit.in_block.contains(&c.id))
        .filter(|c| {
            extent(circuit, c, &parts)
                .is_some_and(|e| overlaps(e, outline.rect) && outline.covers(e) != notched)
        })
        .map(|c| c.id.clone())
        .collect()
}

/// Outward from the outline edge `p` lies on, e.g. `[0, w]` on a bottom edge `w` long.
fn outward(corners: &[Point], p: Point) -> Option<Point> {
    (0..corners.len()).find_map(|k| {
        let (a, b) = (corners[k], corners[(k + 1) % corners.len()]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let between = |v: f32, a: f32, b: f32| a.min(b) - ON_EDGE <= v && v <= a.max(b) + ON_EDGE;
        // Edges are level or upright, so within the edge's box is on it.
        let on = between(p[0], a[0], b[0]) && between(p[1], a[1], b[1]);
        // Clockwise on a y-down sheet, the outside is to the left of travel.
        (on && a != b).then_some([dy, -dx])
    })
}

/// Longest outline drawn dash-dot; a longer one (a part placed far off by mistake) is drawn
/// dashed, so a typo can't make millions of dashes.
const MAX_DASH_DOT: f32 = 20_000.0;

/// A closed dash-dot line through `corners`, the ISO 1219 boundary of a block or assembly,
/// broken round the port circles at `ports`.
fn dash_dot(corners: &[Point], ports: &[Point]) -> Vec<Primitive> {
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
        // Where along this edge each port circle crosses it, as (centre, half width): also a
        // port on the next edge close to the corner.
        let holes: Vec<(f32, f32)> = ports
            .iter()
            .filter_map(|p| {
                let t = ((p[0] - a[0]) * (b[0] - a[0]) + (p[1] - a[1]) * (b[1] - a[1])) / len;
                let q = at(t);
                let off = (q[0] - p[0]).hypot(q[1] - p[1]);
                let half = (PORT_RADIUS.powi(2) - off.powi(2)).sqrt();
                (off < PORT_RADIUS && (-half..=len + half).contains(&t)).then_some((t, half))
            })
            .collect();
        let mut pos = 0.0;
        while pos < len {
            let (dash, on) = PATTERN[i];
            let step = (dash - used).min(len - pos);
            if on {
                let mut pieces = vec![(pos, pos + step)];
                for &(t, half) in &holes {
                    pieces = pieces
                        .into_iter()
                        .flat_map(|(s, e)| [(s, e.min(t - half)), (s.max(t + half), e)])
                        // Not a sliver left by rounding at a gap's edge.
                        .filter(|(s, e)| e - s > 0.01)
                        .collect();
                }
                out.extend(pieces.into_iter().map(|(s, e)| line(at(s), at(e))));
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
