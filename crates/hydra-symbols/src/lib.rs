//! ISO 1219 symbols as plain drawing primitives.
//!
//! [`render_symbol`] turns a symbol definition into polylines, circles and text in millimetres
//! (x right, y down), with an anchor point per port. [`render_circuit`] places symbols and draws
//! the nets. The canvas, SVG and PDF export all draw the same primitives.

use hydra_core::{
    Actuator, Catalogue, Circuit, Component, ComponentKind, DirectionalValve, FlowActuator, Issue,
    LineKind, MAX_LISTED, Net, PORT_PIN, Placement, Rotation, Side, SymbolDef, ValveStatus,
};

mod route;
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
        SymbolDef::Relief {
            adjustable: adj,
            pilot_port,
        } => relief(*adj, *pilot_port),
        SymbolDef::Orifice { adjustable: adj } => orifice(*adj),
        SymbolDef::FlowControl {
            actuator,
            compensated,
            bypass,
            pilot_port,
        } => flow_control(*actuator, *compensated, *bypass, *pilot_port),
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

/// Flow control from In (bottom) to Out (top), with Bypass on the right when `bypass`, and a
/// second pilot X on the right above it when `pilot_port`.
fn flow_control(
    actuator: FlowActuator,
    compensated: bool,
    bypass: bool,
    pilot_port: bool,
) -> Drawing {
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
    if pilot_port {
        // Dashed pilot from the right into the box, on the compensator's side.
        let (x, tip) = (h + 4.0, [h, -2.0]);
        d.primitives.push(dashed(vec![[x, tip[1]], tip]));
        d.primitives.push(arrow_head([x, tip[1]], tip));
        side_port(&mut d, "X", x, tip[1], false);
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

/// Pressure relief valve, P at the bottom, T at the top, normally closed. Piloted from P, or
/// with `pilot_port` from X on the left.
fn relief(adj: bool, pilot_port: bool) -> Drawing {
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
    if pilot_port {
        pilot_from_left(&mut d, "X", -h - 4.0, [-h, 0.0]);
    } else {
        // Pilot line from P to the left end.
        d.primitives.push(dashed(vec![
            [0.0, h + 1.5],
            [-h - 2.0, h + 1.5],
            [-h - 2.0, 0.0],
            [-h, 0.0],
        ]));
        d.primitives.push(arrow_head([-h - 2.0, 0.0], [-h, 0.0]));
    }
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

/// External connection, e.g. manifold port P. A plugged one has no circle: [`draw_component`]
/// closes its line with a bar.
fn port_marker(label: &str, plugged: bool) -> Drawing {
    let circle = Primitive::Circle {
        center: [0.0, 0.0],
        radius: PORT_RADIUS,
        filled: false,
    };
    let label = text([0.0, -2.5], label, Align::Middle);
    Drawing {
        primitives: if plugged {
            vec![label]
        } else {
            vec![circle, label]
        },
        ports: vec![(PORT_PIN.to_string(), [0.0, 0.0])],
    }
}

/// The first of `points` off `at`, when they start at `at`.
fn leaving(mut points: impl Iterator<Item = Point>, at: Point) -> Option<Point> {
    near(points.next()?, at).then_some(())?;
    points.find(|q| !near(*q, at))
}

/// ISO 1219 closed connection: a short thick bar across the line that ends at `at`, square to
/// it. Among `wires`, the line is the one with an end there; without one, an upright line.
fn plug(at: Point, wires: &[Primitive]) -> Primitive {
    let from = wires.iter().find_map(|w| {
        let Primitive::Polyline { points, .. } = w else {
            return None;
        };
        leaving(points.iter().copied(), at).or_else(|| leaving(points.iter().rev().copied(), at))
    });
    let [dx, dy] = from.map_or([0.0, 1.0], |q| [at[0] - q[0], at[1] - q[1]]);
    let len = dx.hypot(dy);
    let (along, across) = (
        [dx / len * 0.35, dy / len * 0.35],
        [-dy / len * 1.5, dx / len * 1.5],
    );
    let corner = |a: f32, b: f32| {
        [
            at[0] + a * along[0] + b * across[0],
            at[1] + a * along[1] + b * across[1],
        ]
    };
    Primitive::Polyline {
        points: vec![
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ],
        closed: true,
        dashed: false,
        filled: true,
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
        ComponentKind::Port { label, plugged } => port_marker(label, *plugged),
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
    if let ComponentKind::Port { plugged: true, .. } = c.kind {
        d.primitives.push(plug([place.x, place.y], wires));
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
    // Finding dots compares points with wires, so only for wires an import could save.
    let dotted = circuit.layout.wires_are_bounded();
    let routed: std::collections::BTreeMap<String, Vec<hydra_core::Wire>> =
        (automatic_wires(circuit, catalogue).into_iter())
            .map(|(id, r)| {
                (
                    id,
                    r.wires.into_iter().map(hydra_core::Wire::from).collect(),
                )
            })
            .collect();
    for net in &circuit.nets {
        if let Some(drawn) = (circuit.layout.wires.get(&net.id)).or(routed.get(net.id.as_str())) {
            // Pilot and drain wires are dashed.
            wires.extend(drawn.iter().map(|w| match w.kind {
                LineKind::Working => open(w.points.clone()),
                LineKind::Pilot | LineKind::Drain => dashed(w.points.clone()),
            }));
            let drawn: Vec<&[Point]> = drawn.iter().map(|w| &w.points[..]).collect();
            if !dotted {
                continue;
            }
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
                if !dots.iter().any(|d| near(*d, at)) && arms(at, &drawn, &ports) >= 3 {
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

/// The automatic wires of each net the layout leaves untraced, by net id: routed round the
/// parts, their designators and the traced wires, from the circuit alone. The renderer draws
/// these, and a check can compare against them.
fn automatic_wires(
    circuit: &Circuit,
    catalogue: &Catalogue,
) -> std::collections::BTreeMap<String, route::Routed> {
    let place = |c: &Component| placement(circuit, &c.id);
    let mut anchors: Vec<(String, Point)> = Vec::new();
    // Each part's symbol box, without its text or status frame, and per port its part's box
    // and whether its line may leave to any side, for the automatic wires. They keep clear of
    // each designator where it goes without wires, on a side no port leaves from, so it can
    // stay there, and off the rest of a part's text where they can. They may cross a status
    // frame, as they may another net's wire, but not run along it.
    let mut boxes = Vec::new();
    let mut sides = Vec::new();
    let mut texts = Vec::new();
    let mut frames: Vec<Vec<Point>> = Vec::new();
    for c in &circuit.components {
        let Some(part) = render_component(c, place(c), catalogue) else {
            continue;
        };
        if !matches!(c.kind, ComponentKind::Port { .. }) {
            for p in &part.primitives {
                if let (Primitive::Text { text, .. }, Some(b)) =
                    (p, bounds(std::slice::from_ref(p)))
                {
                    if *text == c.id {
                        &mut boxes
                    } else {
                        &mut texts
                    }
                    .push(b);
                }
                if let Primitive::Polyline {
                    points,
                    closed: true,
                    dashed: true,
                    ..
                } = p
                {
                    frames.push(points.iter().chain(points.first()).copied().collect());
                }
            }
        }
        // The symbol with its pilot lines, without its text or status frame.
        let symbol: Vec<Primitive> = (part.primitives.into_iter())
            .filter(|p| !matches!(p, Primitive::Text { .. }))
            .filter(|p| {
                !matches!(
                    p,
                    Primitive::Polyline {
                        closed: true,
                        dashed: true,
                        ..
                    }
                )
            })
            .collect();
        let at = [place(c).x, place(c).y];
        let symbol = bounds(&symbol).unwrap_or((at, at));
        boxes.push(symbol);
        for (name, p) in part.ports {
            anchors.push((format!("{}.{name}", c.id), p));
            sides.push((symbol, matches!(c.kind, ComponentKind::Port { .. })));
        }
    }

    let bounded = circuit.layout.wires_are_bounded();
    let traced: Vec<&[Point]> = (circuit.layout.wires.values().flatten())
        .filter(|_| bounded)
        .map(|w| &w.points[..])
        .chain(frames.iter().map(|f| &f[..]))
        .collect();
    let untraced: Vec<&Net> = (circuit.nets.iter())
        .filter(|n| !circuit.layout.wires.contains_key(&n.id))
        .collect();
    let to_route: Vec<Vec<route::End>> = (untraced.iter())
        .map(|n| {
            (n.ports.iter())
                .filter_map(|p| {
                    let key = format!("{}.{}", p.component, p.port);
                    let i = anchors.iter().position(|(k, _)| *k == key)?;
                    let (part, any_side) = sides[i];
                    Some(route::End {
                        at: anchors[i].1,
                        part,
                        any_side,
                    })
                })
                .collect()
        })
        .collect();
    (untraced.iter())
        .map(|n| n.id.clone())
        .zip(route::route(&boxes, &texts, &traced, &to_route))
        .collect()
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
    // by mistake than standing in a notch, so it is flagged instead. A side counts when a block
    // part's centre is beyond the outside part's edge, so a part a little off the grid still
    // fits a notch.
    let centre = |r: Rect| [(r.0[0] + r.1[0]) / 2.0, (r.0[1] + r.1[1]) / 2.0];
    let surrounded = |o: Rect| {
        let side = |axis: usize, sign: f32| {
            let edge = if sign > 0.0 { o.1[axis] } else { o.0[axis] };
            members
                .iter()
                .any(|&m| (centre(m)[axis] - edge) * sign > 0.0)
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
fn arms(p: Point, wires: &[&[Point]], ports: &[Point]) -> usize {
    let through = |w: &&[Point]| if ends(w).any(|e| near(e, p)) { 1 } else { 2 };
    let wires: usize = wires.iter().filter(|w| on_wire(p, w)).map(through).sum();
    wires + ports.iter().filter(|q| near(**q, p)).count()
}

/// Ports' names (`component.port`) and positions.
type Ports = Vec<(String, Point)>;

/// A hand-drawn net, its wires, and its ports.
struct DrawnNet<'a> {
    net: &'a Net,
    wires: Vec<&'a [Point]>,
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
            let wires = (circuit.layout.wires.get(&net.id)?.iter())
                .map(|w| &w.points[..])
                .collect();
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
        let (net, wires, ports) = (&d.net.id, &d.wires, &d.ports);
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

/// Hand-drawn wires that run through a part's symbol, other than into its own ports, usually a
/// misread route; and nets drawn automatically that couldn't be routed clear.
pub fn wire_warnings(circuit: &Circuit, catalogue: &Catalogue) -> Vec<String> {
    let mut warnings: Vec<String> = (automatic_wires(circuit, catalogue).into_iter())
        .filter(|(_, r)| !r.clear)
        .map(|(net, _)| {
            format!(
                "net {net} couldn't be routed clear of the other lines and parts, so it is drawn \
                 with plain elbows that may cross them; trace its wires"
            )
        })
        .collect();
    let Some((nets, _)) = drawn_nets(circuit, catalogue) else {
        return warnings;
    };
    // Each part's symbol box: no designator, status words or status frame. Without its text,
    // a part measures the same here as in `render_parts`, wherever its designator moved to.
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
