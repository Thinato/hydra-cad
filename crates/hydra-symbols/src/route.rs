//! Automatic wires for the nets a layout leaves untraced.
//!
//! Each net is routed on a grid over the sheet, with lines through every port and every part's
//! clearance edge plus free lines evenly spaced between them. A wire leaves each port straight
//! out of its symbol, keeps clear of every symbol and designator, and never runs along (or
//! within a small gap of), turns on or ends on another net's wire or port, or a status frame,
//! though it may cross them. A route costs its length plus a penalty per bend, per crossing and
//! for running over a part's other text, and the cheapest one is found with A*. A net with more
//! ports grows as a tree: each further port takes the cheapest route to any point of the net so
//! far, which gives a trunk with T-junctions. Nets are routed in order after the traced wires,
//! each keeping off those before it. A port with no clean route gets the plain elbow.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::{Point, Rect};

/// Gap kept between an automatic wire and any symbol, in mm.
const CLEAR: f32 = 2.0;
/// Spacing of the free grid lines, in mm.
const PITCH: f32 = 5.0;
/// Most free grid lines across the sheet each way; a bigger sheet spaces them wider.
const MAX_FREE: f32 = 120.0;
/// Cost of a bend, and of crossing another net's wire, in hundredths of a millimetre of wire.
const BEND: u64 = 1000;
const CROSS: u64 = 300;
/// How many times its length a stretch of wire over a part's text costs on top.
const OVER_TEXT: u64 = 3;
/// Most grid points to route on.
// ponytail: past this (hundreds of distinct coordinates each way, as from thousands of traced
// points) every net gets the plain elbows; a sparser grid round each net would lift it.
const MAX_NODES: usize = 250_000;
/// Most states one search takes off its queue, per grid point.
// ponytail: past this a port gets the plain elbows rather than flooding the grid; an unreachable
// port is the usual cause, and a cheaper test for it would lift the cap.
const STEPS_PER_NODE: usize = 4;
/// Margin round a part's text that wires keep off where they can, in mm.
const TEXT_PAD: f32 = 1.0;
/// Hundredths of a millimetre, the unit costs are counted in, per millimetre.
const PER_MM: f32 = 100.0;
/// Nearest a wire runs alongside another net's wire or a status frame, in mm.
const GAP: f32 = 1.5;
/// Grid lines this close are one.
const SAME: f32 = 1e-3;

/// Axis bits: a horizontal and a vertical run.
const H: u8 = 1;
const V: u8 = 2;

/// A port to route: where its line ends, the box round its part's symbol, and whether its line
/// may leave to any side (a port marker) rather than straight out of the symbol.
#[derive(Clone, PartialEq)]
pub(crate) struct End {
    pub at: Point,
    pub part: Rect,
    pub any_side: bool,
}

/// One net's wires as polylines, and whether they are routed clear; `false` when they are the
/// plain elbows, which ignore the other nets and parts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Routed {
    pub wires: Vec<Vec<Point>>,
    pub clear: bool,
}

type Wires = Vec<Routed>;
type Asked = (Vec<Rect>, Vec<Rect>, Vec<Vec<Point>>, Vec<Vec<End>>);

thread_local! {
    /// The last routing asked for, and its wires: the editor draws the same circuit every frame.
    // ponytail: one entry, so two circuits drawn by turns each frame are routed every time; a
    // cache per circuit in the editor would lift it.
    static LAST: RefCell<Option<(Asked, Wires)>> = const { RefCell::new(None) };
}

/// The wires of each net in `nets`, as polylines; `boxes` are the symbols to keep clear of,
/// `texts` the parts' text to keep off where a wire can, and `traced` the hand-drawn wires.
pub(crate) fn route(
    boxes: &[Rect],
    texts: &[Rect],
    traced: &[&[Point]],
    nets: &[Vec<End>],
) -> Wires {
    let asked: Asked = (
        boxes.to_vec(),
        texts.to_vec(),
        traced.iter().map(|w| w.to_vec()).collect(),
        nets.to_vec(),
    );
    let last = LAST.with_borrow(|l| {
        (l.as_ref())
            .filter(|(a, _)| *a == asked)
            .map(|(_, wires)| wires.clone())
    });
    last.unwrap_or_else(|| {
        let wires = plan(boxes, texts, traced, nets);
        LAST.set(Some((asked, wires.clone())));
        wires
    })
}

/// [`route`], worked out afresh.
fn plan(boxes: &[Rect], texts: &[Rect], traced: &[&[Point]], nets: &[Vec<End>]) -> Wires {
    let plain = || -> Wires {
        (nets.iter())
            .map(|ends| Routed {
                wires: elbows(ends),
                clear: ends.len() < 2,
            })
            .collect()
    };
    if nets.iter().all(|ends| ends.len() < 2) {
        return plain();
    }
    let exits: Vec<Vec<Vec<Exit>>> = (nets.iter())
        .map(|ends| ends.iter().map(exits).collect())
        .collect();
    // Each symbol grown by the clearance, except on a side a connected port leaves from: its
    // stub keeps wires off the symbol there, and a wire may run along the stub's end.
    let padded: Vec<Rect> = (boxes.iter())
        .map(|b| {
            let mut open = [false; 4];
            for e in nets
                .iter()
                .flatten()
                .filter(|e| !e.any_side && e.part == *b)
            {
                open[side(e)] = true;
            }
            let c = |s: usize| if open[s] { 0.0 } else { CLEAR };
            (
                [b.0[0] - c(0), b.0[1] - c(2)],
                [b.1[0] + c(1), b.1[1] + c(3)],
            )
        })
        .collect();
    let texts: Vec<Rect> = (texts.iter())
        .map(|(lo, hi)| {
            (
                [lo[0] - TEXT_PAD, lo[1] - TEXT_PAD],
                [hi[0] + TEXT_PAD, hi[1] + TEXT_PAD],
            )
        })
        .collect();
    let keys = (padded.iter().chain(&texts).flat_map(|&(lo, hi)| [lo, hi]))
        .chain(nets.iter().flatten().map(|e| e.at))
        // Each exit, and where its straight run ends (either way: the other is inside the part).
        .chain(
            exits
                .iter()
                .flatten()
                .flatten()
                .flat_map(|&([x, y], a, run)| {
                    let (dx, dy) = if a == H { (run, 0.0) } else { (0.0, run) };
                    [[x, y], [x - dx, y - dy], [x + dx, y + dy]]
                }),
        )
        .chain(traced.iter().flat_map(|w| w.iter().copied()));
    let (xs, ys): (Vec<f32>, Vec<f32>) = keys.map(|[x, y]| (x, y)).unzip();
    let g = Grid {
        xs: lines(xs),
        ys: lines(ys),
    };
    let n = g.xs.len() * g.ys.len();
    if n > MAX_NODES {
        return plain();
    }
    // Per point, the edge to its right (H) and below it (V) when inside a symbol's clearance,
    // and when over a part's text.
    let blocked = inside(&g, &padded);
    let mut r = Router {
        over_text: inside(&g, &texts),
        blocked,
        used: vec![0; n],
        owner: vec![usize::MAX; n],
        dist: vec![0; 2 * n],
        prev: vec![0; 2 * n],
        seen: vec![0; 2 * n],
        epoch: 0,
        g,
    };
    // Where each port's line leaves its symbol belongs to its net, and so does the band within
    // the gap round it; the exits themselves last, so a close neighbour's band never takes one.
    let nx = r.g.xs.len();
    for exact in [false, true] {
        for (net, ends) in exits.iter().enumerate() {
            for &(p, _, _) in ends.iter().flatten() {
                let Some(k) = r.g.node(p) else { continue };
                if exact {
                    r.owner[k] = net;
                    continue;
                }
                for j in beside(&r.g.ys, k / nx) {
                    for i in beside(&r.g.xs, k % nx) {
                        r.owner[j * nx + i] = net;
                    }
                }
            }
        }
    }
    for w in traced {
        mark(&r.g, &mut r.used, w);
    }
    // The points a further port's route may join: the first port's exits (H or V, the axis it
    // leaves along), then the net's wires so far (H | V).
    let mut target = vec![0u8; n];
    let mut out = Vec::new();
    for (net, ends) in nets.iter().enumerate() {
        let Some((first, _)) = ends.split_first() else {
            out.push(Routed {
                wires: Vec::new(),
                clear: true,
            });
            continue;
        };
        // The targets, and the boxes round their runs that A* steers by.
        let mut tree: Vec<usize> = Vec::new();
        let mut goals: Vec<Rect> = Vec::new();
        for &(p, axis, _) in &exits[net][0] {
            if let Some(k) = r.g.node(p) {
                target[k] = axis;
                tree.push(k);
                goals.push((p, p));
            }
        }
        // Nearest first; a stable sort keeps the circuit's order between equals.
        let far = |e: &End| (e.at[0] - first.at[0]).abs() + (e.at[1] - first.at[1]).abs();
        let mut order: Vec<usize> = (1..ends.len()).collect();
        order.sort_by(|&a, &b| far(&ends[a]).total_cmp(&far(&ends[b])));
        let mut wires = Vec::new();
        let mut clear = true;
        for i in order {
            let e = &ends[i];
            let Some(path) = r.search(net, &exits[net][i], &target, &goals) else {
                clear = false;
                break;
            };
            let mut w: Vec<Point> = path.iter().map(|&k| r.g.at(k)).collect();
            let join = path[path.len() - 1];
            if target[join] != H | V {
                // Into the first port: from now on the net is this wire.
                w.push(first.at);
                tree.drain(..).for_each(|k| target[k] = 0);
                goals.clear();
            }
            for &k in &path {
                if target[k] == 0 {
                    target[k] = H | V;
                    tree.push(k);
                }
            }
            let run = simplify(path.iter().map(|&k| r.g.at(k)).collect());
            goals.extend(run.windows(2).map(|s| {
                let lo = [s[0][0].min(s[1][0]), s[0][1].min(s[1][1])];
                (lo, [s[0][0].max(s[1][0]), s[0][1].max(s[1][1])])
            }));
            goals.extend(run.first().map(|&p| (p, p)));
            w.reverse();
            w.push(e.at);
            wires.push(simplify(w));
        }
        tree.into_iter().for_each(|k| target[k] = 0);
        if clear {
            for w in &wires {
                mark(&r.g, &mut r.used, w);
            }
        } else {
            // A port with no clean route: the whole net as the plain elbows, which break the
            // rules anyway, so later nets needn't keep off them.
            wires = elbows(ends);
        }
        out.push(Routed { wires, clear });
    }
    out
}

/// Per grid point, the edges to its right (H) and below it (V) inside any of `boxes`, whose
/// edges are on grid lines.
fn inside(g: &Grid, boxes: &[Rect]) -> Vec<u8> {
    let nx = g.xs.len();
    let mut out = vec![0u8; nx * g.ys.len()];
    for &([x0, y0], [x1, y1]) in boxes {
        let (Some(i0), Some(i1), Some(j0), Some(j1)) =
            (g.find_x(x0), g.find_x(x1), g.find_y(y0), g.find_y(y1))
        else {
            continue;
        };
        for j in j0..=j1 {
            for i in i0..=i1 {
                if j > j0 && j < j1 && i < i1 {
                    out[j * nx + i] |= H;
                }
                if i > i0 && i < i1 && j < j1 {
                    out[j * nx + i] |= V;
                }
            }
        }
    }
    out
}

/// Where a line starts on the grid, the axis it leaves along, and how far it runs straight
/// before it may turn, in mm.
type Exit = (Point, u8, f32);

/// The exits of a line from `e`: a port marker's from its clearance on any side, a symbol's
/// port's from the edge of its symbol's box straight out from the port (its end, unless some
/// of the symbol sticks out past it), running on straight as far as the clearance.
fn exits(e: &End) -> Vec<Exit> {
    let ([x0, y0], [x1, y1]) = e.part;
    let [x, y] = e.at;
    if !e.any_side {
        return vec![match side(e) {
            0 => ([x0, y], H, CLEAR),
            1 => ([x1, y], H, CLEAR),
            2 => ([x, y0], V, CLEAR),
            _ => ([x, y1], V, CLEAR),
        }];
    }
    vec![
        ([x0 - CLEAR, y], H, 0.0),
        ([x1 + CLEAR, y], H, 0.0),
        ([x, y0 - CLEAR], V, 0.0),
        ([x, y1 + CLEAR], V, 0.0),
    ]
}

/// The side of its symbol a port is on: 0 left, 1 right, 2 top, 3 bottom.
fn side(e: &End) -> usize {
    let ([x0, y0], [x1, y1]) = e.part;
    let [x, y] = e.at;
    let gaps = [x - x0, x1 - x, y - y0, y1 - y];
    (0..4)
        .min_by(|&a, &b| gaps[a].total_cmp(&gaps[b]))
        .unwrap_or(0)
}

/// The plain wires: a level trunk through the first port, and straight from it into each
/// other port. Each piece is its own wire, so every T and corner is a wire's end, where the
/// renderer looks for junction dots.
fn elbows(ends: &[End]) -> Vec<Vec<Point>> {
    let Some((first, rest)) = ends.split_first() else {
        return Vec::new();
    };
    let [fx, fy] = first.at;
    let mut out = Vec::new();
    // The trunk each way from the first port, broken where a branch leaves it.
    for way in [-1.0f32, 1.0] {
        let mut stops: Vec<f32> = (rest.iter().map(|e| e.at[0]))
            .filter(|x| (x - fx) * way > 0.0)
            .collect();
        stops.sort_by(|a, b| ((a - fx) * way).total_cmp(&((b - fx) * way)));
        stops.dedup();
        let mut from = fx;
        for x in stops {
            out.push(vec![[from, fy], [x, fy]]);
            from = x;
        }
    }
    for e in rest.iter().filter(|e| e.at[1] != fy) {
        out.push(vec![[e.at[0], fy], e.at]);
    }
    out
}

/// Grid lines through every one of `key`, plus free lines a pitch apart over them and a little
/// beyond, each at least the clearance from a key line.
fn lines(mut key: Vec<f32>) -> Vec<f32> {
    key.retain(|v| v.is_finite());
    key.sort_by(f32::total_cmp);
    key.dedup_by(|a, b| (*a - *b).abs() <= SAME);
    let (Some(&lo), Some(&hi)) = (key.first(), key.last()) else {
        return key;
    };
    let pitch = PITCH.max((hi - lo) / MAX_FREE);
    if !pitch.is_finite() {
        return key;
    }
    let mut all = key.clone();
    for k in (lo / pitch).floor() as i64 - 2..=(hi / pitch).ceil() as i64 + 2 {
        let v = k as f32 * pitch;
        let i = key.partition_point(|&q| q < v);
        let near = [i.wrapping_sub(1), i]
            .iter()
            .filter_map(|&i| key.get(i))
            .any(|q| (q - v).abs() < CLEAR);
        if !near {
            all.push(v);
        }
    }
    all.sort_by(f32::total_cmp);
    all
}

struct Grid {
    xs: Vec<f32>,
    ys: Vec<f32>,
}

impl Grid {
    fn find_x(&self, v: f32) -> Option<usize> {
        find(&self.xs, v)
    }
    fn find_y(&self, v: f32) -> Option<usize> {
        find(&self.ys, v)
    }
    /// The grid point at `p`, when `p` is on one.
    fn node(&self, [x, y]: Point) -> Option<usize> {
        Some(self.find_y(y)? * self.xs.len() + self.find_x(x)?)
    }
    fn at(&self, k: usize) -> Point {
        let nx = self.xs.len();
        [self.xs[k % nx], self.ys[k / nx]]
    }
}

fn find(lines: &[f32], v: f32) -> Option<usize> {
    let i = lines.partition_point(|&l| l < v - SAME);
    ((lines.get(i)? - v).abs() <= SAME).then_some(i)
}

/// The lines nearer than the gap to `lines[at]`, that one included. The band is a hair short
/// of the gap at each end, so a line exactly the gap away is outside it whatever the rounding.
fn beside(lines: &[f32], at: usize) -> std::ops::Range<usize> {
    let v = lines[at];
    let (lo, hi) = (v - GAP + SAME, v + GAP - SAME);
    lines.partition_point(|&l| l < lo)..lines.partition_point(|&l| l <= hi)
}

/// Mark the grid points `w` passes: the axis of each run along it, also on grid lines nearer
/// than the gap beside it, and both axes round its corners and ends, where nothing may touch
/// or pass close by it. Slanted runs are not marked.
fn mark(g: &Grid, used: &mut [u8], w: &[Point]) {
    let nx = g.xs.len();
    for s in w.windows(2) {
        let (Some(a), Some(b)) = (g.node(s[0]), g.node(s[1])) else {
            continue;
        };
        let ((i0, j0), (i1, j1)) = ((a % nx, a / nx), (b % nx, b / nx));
        if j0 == j1 {
            for j in beside(&g.ys, j0) {
                (i0.min(i1)..=i0.max(i1)).for_each(|i| used[j * nx + i] |= H);
            }
        } else if i0 == i1 {
            for i in beside(&g.xs, i0) {
                (j0.min(j1)..=j0.max(j1)).for_each(|j| used[j * nx + i] |= V);
            }
        }
    }
    for k in w.iter().filter_map(|p| g.node(*p)) {
        for j in beside(&g.ys, k / nx) {
            for i in beside(&g.xs, k % nx) {
                used[j * nx + i] = H | V;
            }
        }
    }
}

/// `w` without repeated points or points in the middle of a straight run.
fn simplify(w: Vec<Point>) -> Vec<Point> {
    let same = |a: f32, b: f32| (a - b).abs() <= SAME;
    let mut out: Vec<Point> = Vec::new();
    for p in w {
        if out
            .last()
            .is_some_and(|q| same(q[0], p[0]) && same(q[1], p[1]))
        {
            continue;
        }
        if let [.., a, b] = out[..]
            && ((same(a[0], b[0]) && same(b[0], p[0])) || (same(a[1], b[1]) && same(b[1], p[1])))
        {
            out.pop();
        }
        out.push(p);
    }
    out
}

/// The search state: per grid point the edges blocked by symbols or over text, the axes other nets' wires
/// take through it, and the net whose port leaves through it; per point and arrival axis, the
/// best cost and the step before, valid for the search numbered `epoch`.
struct Router {
    g: Grid,
    blocked: Vec<u8>,
    over_text: Vec<u8>,
    used: Vec<u8>,
    owner: Vec<usize>,
    dist: Vec<u64>,
    prev: Vec<u32>,
    seen: Vec<u32>,
    epoch: u32,
}

impl Router {
    /// The cheapest route for net `net` from one of `from` (exits and their axes) to a point
    /// marked in `target`, all of which lie in `goals`: the grid points from the exit to the
    /// join.
    fn search(
        &mut self,
        net: usize,
        from: &[Exit],
        target: &[u8],
        goals: &[Rect],
    ) -> Option<Vec<usize>> {
        if goals.is_empty() {
            return None;
        }
        self.epoch += 1;
        let (nx, ny) = (self.g.xs.len(), self.g.ys.len());
        // Distance to the nearest goal never overestimates, so A* stays exact.
        let h = |[x, y]: Point| {
            let off = |v: f32, lo: f32, hi: f32| (lo - v).max(v - hi).max(0.0);
            let near = (goals.iter())
                .map(|(lo, hi)| off(x, lo[0], hi[0]) + off(y, lo[1], hi[1]))
                .fold(f32::INFINITY, f32::min);
            (near * PER_MM) as u64
        };
        let mut heap = BinaryHeap::new();
        for &(p, axis, _) in from {
            let Some(k) = self.g.node(p).filter(|&k| self.owner[k] == net) else {
                continue;
            };
            let s = 2 * k + usize::from(axis == V);
            self.reach(s, 0, u32::MAX);
            heap.push(Reverse((h(p), s)));
        }
        // Within an exit's straight run, along its axis.
        let stub = |[x, y]: Point| {
            from.iter().any(|&([px, py], a, run)| {
                let (along, across) = if a == H {
                    (x - px, y - py)
                } else {
                    (y - py, x - px)
                };
                across.abs() <= SAME && along.abs() < run - SAME
            })
        };
        let mut steps_left = STEPS_PER_NODE * nx * ny;
        while let Some(Reverse((f, s))) = heap.pop() {
            steps_left = steps_left.checked_sub(1)?;
            let (k, axis) = (s / 2, if s % 2 == 0 { H } else { V });
            let d = self.dist[s];
            if d + h(self.g.at(k)) < f {
                continue;
            }
            if target[k] != 0 {
                let mut path = vec![k];
                let mut s = s;
                while self.prev[s] != u32::MAX {
                    s = self.prev[s] as usize;
                    path.push(s / 2);
                }
                path.reverse();
                return Some(path);
            }
            // Out of a port straight for the shortest stub at least, and across another net's
            // wire straight over.
            let straight = self.prev[s] == u32::MAX || stub(self.g.at(k)) || self.used[k] != 0;
            // Each step as its axis, the point it reaches, and the point whose edge it is.
            let steps = [
                (H, (k % nx + 1 < nx).then(|| (k + 1, k))),
                (
                    H,
                    (k % nx > 0).then(|| (k.wrapping_sub(1), k.wrapping_sub(1))),
                ),
                (V, (k / nx + 1 < ny).then(|| (k + nx, k))),
                (
                    V,
                    (k >= nx).then(|| (k.wrapping_sub(nx), k.wrapping_sub(nx))),
                ),
            ];
            for (b, step) in steps {
                let Some((m, edge)) = step.filter(|&(_, e)| self.blocked[e] & b == 0) else {
                    continue;
                };
                let (used, owner) = (self.used[m], self.owner[m]);
                if (straight && b != axis)
                    || used & b != 0
                    || (owner != usize::MAX && owner != net)
                    || (target[m] != 0 && used != 0)
                {
                    continue;
                }
                let [ax, ay] = self.g.at(k);
                let [bx, by] = self.g.at(m);
                let len = (((bx - ax).abs() + (by - ay).abs()) * PER_MM).round() as u64;
                let mut c = d + len;
                if self.over_text[edge] & b != 0 {
                    c += OVER_TEXT * len;
                }
                if b != axis {
                    c += BEND;
                }
                if used != 0 {
                    c += CROSS;
                }
                // Into the first port's exit across its axis turns there, or makes a T with it.
                if target[m] != 0 && target[m] & b == 0 {
                    c += BEND;
                }
                let t = 2 * m + usize::from(b == V);
                if self.seen[t] != self.epoch || c < self.dist[t] {
                    self.reach(t, c, s as u32);
                    heap.push(Reverse((c + h([bx, by]), t)));
                }
            }
        }
        None
    }

    fn reach(&mut self, s: usize, d: u64, prev: u32) {
        (self.seen[s], self.dist[s], self.prev[s]) = (self.epoch, d, prev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A port marker at `at`: any side, its circle as the box.
    fn marker(at: Point) -> End {
        let r = 1.2;
        End {
            at,
            part: ([at[0] - r, at[1] - r], [at[0] + r, at[1] + r]),
            any_side: true,
        }
    }

    /// The routed wires of each net, all of them clear.
    fn clear_wires(boxes: &[Rect], traced: &[&[Point]], nets: &[Vec<End>]) -> Vec<Vec<Vec<Point>>> {
        let routed = route(boxes, &[], traced, nets);
        assert!(routed.iter().all(|r| r.clear), "{routed:?}");
        routed.into_iter().map(|r| r.wires).collect()
    }

    fn crosses_box(w: &[Point], (lo, hi): Rect) -> bool {
        w.windows(2)
            .any(|s| (0..2).all(|k| s[0][k].min(s[1][k]) < hi[k] && s[0][k].max(s[1][k]) > lo[k]))
    }

    #[test]
    fn a_wire_goes_round_a_symbol_in_its_way() {
        let part: Rect = ([20.0, -5.0], [30.0, 5.0]);
        let nets = vec![vec![marker([0.0, 0.0]), marker([50.0, 0.0])]];
        let boxes = [part, nets[0][0].part, nets[0][1].part];
        let wires = clear_wires(&boxes, &[], &nets);
        assert_eq!(wires[0].len(), 1);
        let w = &wires[0][0];
        assert!(!crosses_box(w, part), "{w:?}");
        assert_eq!((w[0], w[w.len() - 1]), ([0.0, 0.0], [50.0, 0.0]));
        // Port markers may leave upwards: over the top with the clearance, in two bends.
        assert_eq!(w, &[[0.0, 0.0], [0.0, -7.0], [50.0, -7.0], [50.0, 0.0]]);
    }

    #[test]
    fn a_wire_leaves_a_valve_port_straight_out_of_its_symbol() {
        // A valve's port on the bottom of its symbol, and a port marker up and to the left.
        let valve = End {
            at: [30.0, 10.0],
            part: ([25.0, 0.0], [35.0, 10.0]),
            any_side: false,
        };
        let nets = vec![vec![marker([0.0, -20.0]), valve]];
        let boxes = [nets[0][0].part, nets[0][1].part];
        let w = &clear_wires(&boxes, &[], &nets)[0][0];
        assert!(!crosses_box(w, ([25.0, 0.0], [35.0, 10.0])), "{w:?}");
        let n = w.len();
        assert_eq!(w[n - 1], [30.0, 10.0]);
        assert_eq!(w[n - 2][0], 30.0, "straight down out of the port: {w:?}");
        assert!(w[n - 2][1] > 10.0, "{w:?}");
    }

    #[test]
    fn a_wire_keeps_off_another_nets_wire() {
        // Another net's wire lies on the straight line between the two ports.
        let other: &[Point] = &[[5.0, 0.0], [45.0, 0.0]];
        let nets = vec![vec![marker([0.0, 0.0]), marker([50.0, 0.0])]];
        let boxes = [nets[0][0].part, nets[0][1].part];
        let w = &clear_wires(&boxes, &[other], &nets)[0][0];
        for s in w.windows(2) {
            let along = s[0][1] == 0.0 && s[1][1] == 0.0;
            assert!(!along || s[0][0].max(s[1][0]) <= 5.0 || s[0][0].min(s[1][0]) >= 45.0);
        }
        // Nor over another net's port: a second net routed after the first goes round it.
        let nets = vec![
            vec![marker([0.0, 0.0]), marker([50.0, 0.0])],
            vec![marker([25.0, 0.0]), marker([25.0, 30.0])],
        ];
        let boxes: Vec<Rect> = nets.iter().flatten().map(|e| e.part).collect();
        let wires = clear_wires(&boxes, &[], &nets);
        let first = &wires[0][0];
        assert!(
            !crosses_box(first, ([22.0, -3.0], [28.0, 3.0])),
            "{first:?}"
        );
        // The second crosses the first square on, never along it.
        let second = &wires[1][0];
        assert_eq!(second.len(), 2, "{second:?}");
        // Nor over the end of a valve's port, which lies on its symbol's unpadded side.
        let valve = End {
            at: [25.0, 0.0],
            part: ([20.0, -10.0], [30.0, 0.0]),
            any_side: false,
        };
        let nets = vec![
            vec![marker([0.0, 0.0]), marker([50.0, 0.0])],
            vec![valve, marker([25.0, 20.0])],
        ];
        let boxes: Vec<Rect> = nets.iter().flatten().map(|e| e.part).collect();
        let got = clear_wires(&boxes, &[], &nets);
        assert!(!on([25.0, 0.0], &got[0][0]), "{:?}", got[0][0]);
        assert_eq!(got[1][0].len(), 2, "{:?}", got[1][0]);
    }

    #[test]
    fn a_wire_keeps_the_gap_from_another_nets_end() {
        // Another net's wire ends at 10, 10; the straight line between these ports would pass
        // a quarter of a millimetre beyond that end, which reads as a junction.
        let other: &[Point] = &[[-20.0, 10.0], [10.0, 10.0]];
        let nets = vec![vec![marker([10.25, -20.0]), marker([10.25, 30.0])]];
        let boxes: Vec<Rect> = nets[0].iter().map(|e| e.part).collect();
        let w = &clear_wires(&boxes, &[other], &nets)[0][0];
        let near = |p: Point| (p[0] - 10.0).abs() < GAP && (p[1] - 10.0).abs() < GAP;
        // No run of the wire passes within the gap of that end.
        for s in w.windows(2) {
            let steps = 100;
            let close = (0..=steps).map(|t| t as f32 / steps as f32).any(|t| {
                near([
                    s[0][0] + (s[1][0] - s[0][0]) * t,
                    s[0][1] + (s[1][1] - s[0][1]) * t,
                ])
            });
            assert!(!close, "{w:?}");
        }
    }

    #[test]
    fn three_ports_join_as_a_tree() {
        // A supply on the left, two consumers above a common line.
        let nets = vec![vec![
            marker([0.0, 0.0]),
            marker([40.0, -20.0]),
            marker([60.0, -20.0]),
        ]];
        let boxes: Vec<Rect> = nets[0].iter().map(|e| e.part).collect();
        let routed = clear_wires(&boxes, &[], &nets);
        assert_eq!(routed[0].len(), 2);
        // The second branch ends on the first wire, not back at the supply.
        let (a, b) = (&routed[0][0], &routed[0][1]);
        let total: f32 = routed[0]
            .iter()
            .flat_map(|w| w.windows(2))
            .map(|s| (s[1][0] - s[0][0]).abs() + (s[1][1] - s[0][1]).abs())
            .sum();
        assert!(b[0] != [0.0, 0.0] && on(b[0], a), "{a:?} {b:?}");
        // Shorter than the star of elbows from the supply (60 + 20 + 40 + 20).
        assert!(total < 100.0, "{total}: {a:?} {b:?}");
    }

    fn on(p: Point, w: &[Point]) -> bool {
        w.windows(2).any(|s| {
            (0..2)
                .all(|k| s[0][k].min(s[1][k]) - SAME <= p[k] && p[k] <= s[0][k].max(s[1][k]) + SAME)
        })
    }

    #[test]
    fn routes_are_the_same_every_time_and_fall_back_to_the_elbow() {
        let nets = || {
            vec![
                vec![
                    marker([0.0, 0.0]),
                    marker([30.0, 25.0]),
                    marker([55.0, -10.0]),
                ],
                vec![marker([10.0, 30.0]), marker([40.0, -15.0])],
            ]
        };
        let boxes: Vec<Rect> = nets()
            .iter()
            .flatten()
            .map(|e| e.part)
            .chain([([15.0, 0.0], [25.0, 15.0])])
            .collect();
        let wires = route(&boxes, &[], &[], &nets());
        assert_eq!(plan(&boxes, &[], &[], &nets()), wires);
        assert_eq!(route(&boxes, &[], &[], &nets()), wires);
        // A port boxed in by another symbol has no clean route: the plain elbows, said so.
        let nets = vec![vec![marker([0.0, 0.0]), marker([30.0, 20.0])]];
        let walled = [([25.0, 15.0], [35.0, 25.0])];
        let plain = Routed {
            wires: vec![
                vec![[0.0, 0.0], [30.0, 0.0]],
                vec![[30.0, 0.0], [30.0, 20.0]],
            ],
            clear: false,
        };
        assert_eq!(route(&walled, &[], &[], &nets), [plain]);
        // Those elbows break the rules anyway, so a later net may run where they go.
        let nets = vec![
            vec![marker([0.0, 0.0]), marker([30.0, 20.0])],
            vec![marker([8.0, 0.0]), marker([22.0, 0.0])],
        ];
        let mut boxes = walled.to_vec();
        boxes.extend(nets[1].iter().map(|e| e.part));
        let got = route(&boxes, &[], &[], &nets);
        assert!(!got[0].clear && got[1].clear);
        assert_eq!(got[1].wires, [vec![[8.0, 0.0], [22.0, 0.0]]]);
    }

    #[test]
    fn the_plain_elbows_break_at_every_branch() {
        // A trunk each way from the first port, broken at each branch, so the renderer dots
        // the T at 20 and at the first port, and not the last corner at 40.
        let ends = [
            marker([0.0, 0.0]),
            marker([20.0, 10.0]),
            marker([40.0, 10.0]),
            marker([-10.0, -5.0]),
        ];
        assert_eq!(
            elbows(&ends),
            [
                vec![[0.0, 0.0], [-10.0, 0.0]],
                vec![[0.0, 0.0], [20.0, 0.0]],
                vec![[20.0, 0.0], [40.0, 0.0]],
                vec![[20.0, 0.0], [20.0, 10.0]],
                vec![[40.0, 0.0], [40.0, 10.0]],
                vec![[-10.0, 0.0], [-10.0, -5.0]],
            ]
        );
    }

    #[test]
    fn too_big_a_grid_or_too_long_a_search_gets_the_plain_elbows() {
        // Hundreds of distinct coordinates each way make too many grid points to route on.
        let stairs: Vec<Point> = (0..600).map(|i| [i as f32 * 3.0, i as f32 * 3.0]).collect();
        let nets = vec![vec![marker([-20.0, 0.0]), marker([-20.0, 30.0])]];
        let boxes: Vec<Rect> = nets[0].iter().map(|e| e.part).collect();
        let got = route(&boxes, &[], &[&stairs[..]], &nets);
        assert_eq!(got[0].wires, elbows(&nets[0]));
        assert!(!got[0].clear);
        // A first port nothing can reach: the other port's search runs out, bounded.
        let walled = [([25.0, 15.0], [35.0, 25.0])];
        let nets = vec![vec![marker([30.0, 20.0]), marker([0.0, 0.0])]];
        assert!(!route(&walled, &[], &[], &nets)[0].clear);
    }

    #[test]
    fn a_nets_first_port_is_entered_straight_and_every_exit_has_a_stub() {
        // The first port is a valve's, on the bottom of its symbol; the other port lies off to
        // the right and below, so the wire can come up into it rather than turn at its tip.
        let valve = End {
            at: [25.0, 0.0],
            part: ([20.0, -10.0], [30.0, 0.0]),
            any_side: false,
        };
        let nets = vec![vec![valve, marker([45.0, 20.0])]];
        let boxes: Vec<Rect> = nets[0].iter().map(|e| e.part).collect();
        let w = &clear_wires(&boxes, &[], &nets)[0][0];
        assert_eq!((w[0], w[1][0]), ([25.0, 0.0], 25.0), "{w:?}");
        assert!(w[1][1] >= CLEAR, "{w:?}");
        // A grid line a hair outside a port's tip doesn't let its wire turn there.
        let valve = End {
            at: [30.0, 10.0],
            part: ([25.0, 0.0], [35.0, 10.0]),
            any_side: false,
        };
        let nets = vec![vec![marker([0.0, 10.1]), valve]];
        let boxes: Vec<Rect> = nets[0].iter().map(|e| e.part).collect();
        let w = &clear_wires(&boxes, &[], &nets)[0][0];
        let n = w.len();
        assert_eq!(w[n - 2][0], 30.0, "{w:?}");
        assert!(w[n - 2][1] >= 10.0 + CLEAR, "{w:?}");
    }

    #[test]
    fn automatic_wires_pass_the_drawn_wire_checks_on_every_demo_turned_and_mirrored() {
        use hydra_core::{Catalogue, Circuit, Rotation, Wire};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap();
        let catalogue: Catalogue = serde_json::from_str(&read(dir.join("catalogue.json"))).unwrap();
        let (mut checked, mut plain) = (0, 0);
        for entry in std::fs::read_dir(dir.join("circuits")).unwrap() {
            let path = entry.unwrap().path();
            let base = Circuit::from_json(&read(path.clone())).unwrap();
            for turns in 0..4 {
                for mirror in [false, true] {
                    let mut c = base.clone();
                    // Turned parts leave hand-drawn wires behind, so route every net then.
                    if turns > 0 || mirror {
                        c.layout.wires.clear();
                    }
                    for place in c.layout.components.values_mut() {
                        for _ in 0..turns {
                            place.rotation = match place.rotation {
                                Rotation::R0 => Rotation::R90,
                                Rotation::R90 => Rotation::R180,
                                Rotation::R180 => Rotation::R270,
                                Rotation::R270 => Rotation::R0,
                            };
                        }
                        place.mirror ^= mirror;
                    }
                    let what = format!("{} turned {turns}, mirrored {mirror}", path.display());
                    // Each net routed clear, traced as the router drew it.
                    for (net, r) in crate::automatic_wires(&c, &catalogue) {
                        if r.clear {
                            let wires = r.wires.into_iter().map(Wire::from).collect();
                            c.layout.wires.insert(net, wires);
                            checked += 1;
                        } else {
                            plain += 1;
                        }
                    }
                    let problems = crate::wire_problems(&c, &catalogue);
                    assert_eq!(problems, Vec::<String>::new(), "{what}");
                    let through: Vec<String> = (crate::wire_warnings(&c, &catalogue).into_iter())
                        .filter(|w| w.contains("runs through"))
                        .collect();
                    assert_eq!(through, Vec::<String>::new(), "{what}");
                }
            }
        }
        assert!(checked > 100, "{checked} nets checked");
        // Parts turned where they stand land on each other now and then, and those nets get the
        // plain elbows; more than that means one net's elbows are blocking the next.
        assert!(
            plain * 10 <= checked + plain,
            "{plain} of {} nets",
            checked + plain
        );
    }

    #[test]
    fn a_net_drawn_with_the_plain_elbows_is_warned_about() {
        use hydra_core::{Catalogue, Circuit};
        // Port R sits under orifice Z's symbol, so no clean route reaches it.
        let json = r#"{"schema": 5, "name": "w", "components": [
            {"id": "Q", "kind": "port", "label": "Q"}, {"id": "R", "kind": "port", "label": "R"},
            {"id": "Z", "kind": "item", "symbol": {"kind": "orifice"}}],
            "nets": [{"id": "n", "ports": [{"component": "Q", "port": "port"}, {"component": "R", "port": "port"}]}],
            "layout": {"components": {"Q": {"x": 0, "y": 0}, "R": {"x": 30, "y": 20},
                                      "Z": {"x": 30, "y": 20}}}}"#;
        let circuit = Circuit::from_json(json).unwrap();
        let catalogue = Catalogue::default();
        assert!(!crate::automatic_wires(&circuit, &catalogue)["n"].clear);
        let warnings = crate::wire_warnings(&circuit, &catalogue);
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with("net n couldn't be routed clear")),
            "{warnings:?}"
        );
        // Routed clear once Z moves off it: no warning.
        let mut moved = circuit.clone();
        moved.layout.components.get_mut("Z").unwrap().x = 60.0;
        assert_eq!(
            crate::wire_warnings(&moved, &catalogue),
            Vec::<String>::new()
        );
    }
}
