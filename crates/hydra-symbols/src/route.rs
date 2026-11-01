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
// ponytail: past this (a part placed far off, or thousands of traced points) every net gets the
// plain elbows; a sparser grid round each net would lift it.
const MAX_NODES: usize = 250_000;
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

type Wires = Vec<Vec<Vec<Point>>>;
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
    let exits: Vec<Vec<Vec<(Point, u8)>>> = (nets.iter())
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
        .map(|(lo, hi)| ([lo[0] - 1.0, lo[1] - 1.0], [hi[0] + 1.0, hi[1] + 1.0]))
        .collect();
    let keys = (padded.iter().chain(&texts).flat_map(|&(lo, hi)| [lo, hi]))
        .chain(nets.iter().flatten().map(|e| e.at))
        .chain(exits.iter().flatten().flatten().map(|(p, _)| *p))
        .chain(traced.iter().flat_map(|w| w.iter().copied()));
    let (xs, ys): (Vec<f32>, Vec<f32>) = keys.map(|[x, y]| (x, y)).unzip();
    let g = Grid {
        xs: lines(xs),
        ys: lines(ys),
    };
    let n = g.xs.len() * g.ys.len();
    if n > MAX_NODES {
        return nets.iter().map(|ends| elbows(ends)).collect();
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
    // Where each port's line leaves its symbol belongs to its net.
    for (net, ends) in exits.iter().enumerate() {
        for &(p, _) in ends.iter().flatten() {
            if let Some(k) = r.g.node(p) {
                r.owner[k] = net;
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
            out.push(Vec::new());
            continue;
        };
        // The targets, and the boxes round their runs that A* steers by.
        let mut tree: Vec<usize> = Vec::new();
        let mut goals: Vec<Rect> = Vec::new();
        for &(p, axis) in &exits[net][0] {
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
        for i in order {
            let e = &ends[i];
            let Some(path) = r.search(net, &exits[net][i], &target, &goals) else {
                wires.push(elbow(first.at, e.at));
                continue;
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
        for w in &wires {
            mark(&r.g, &mut r.used, w);
        }
        out.push(wires);
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

/// Where a line from `e` starts on the grid, and the axis it leaves along: a port marker's from
/// its clearance on any side, a symbol's port's from its end, straight out of the symbol.
fn exits(e: &End) -> Vec<(Point, u8)> {
    let ([x0, y0], [x1, y1]) = e.part;
    let [x, y] = e.at;
    if !e.any_side {
        return vec![(e.at, if side(e) < 2 { H } else { V })];
    }
    vec![
        ([x0 - CLEAR, y], H),
        ([x1 + CLEAR, y], H),
        ([x, y0 - CLEAR], V),
        ([x, y1 + CLEAR], V),
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

/// Today's plain wires: from the first port across, then straight into each other port.
fn elbows(ends: &[End]) -> Vec<Vec<Point>> {
    let Some((first, rest)) = ends.split_first() else {
        return Vec::new();
    };
    rest.iter().map(|e| elbow(first.at, e.at)).collect()
}

fn elbow(a: Point, b: Point) -> Vec<Point> {
    vec![a, [b[0], a[1]], b]
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

/// Mark the grid points `w` passes: the axis of each run along it, also on grid lines nearer
/// than the gap beside it, and both axes at its corners and ends, where nothing may touch it.
/// Slanted runs are not marked.
fn mark(g: &Grid, used: &mut [u8], w: &[Point]) {
    let nx = g.xs.len();
    // The lines within the gap of `lines[at]`.
    let beside = |lines: &[f32], at: usize| {
        let v = lines[at];
        lines.partition_point(|&l| l <= v - GAP)..lines.partition_point(|&l| l < v + GAP)
    };
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
        used[k] = H | V;
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
        from: &[(Point, u8)],
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
            (near * 100.0) as u64
        };
        let mut heap = BinaryHeap::new();
        for &(p, axis) in from {
            let Some(k) = self.g.node(p).filter(|&k| self.owner[k] == net) else {
                continue;
            };
            let s = 2 * k + usize::from(axis == V);
            self.reach(s, 0, u32::MAX);
            heap.push(Reverse((h(p), s)));
        }
        while let Some(Reverse((f, s))) = heap.pop() {
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
            // Out of a port straight, and across another net's wire straight over.
            let start = self.prev[s] == u32::MAX;
            let straight = start || self.used[k] != 0;
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
                let len = (((bx - ax).abs() + (by - ay).abs()) * 100.0).round() as u64;
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

    fn crosses_box(w: &[Point], (lo, hi): Rect) -> bool {
        w.windows(2)
            .any(|s| (0..2).all(|k| s[0][k].min(s[1][k]) < hi[k] && s[0][k].max(s[1][k]) > lo[k]))
    }

    #[test]
    fn a_wire_goes_round_a_symbol_in_its_way() {
        let part: Rect = ([20.0, -5.0], [30.0, 5.0]);
        let nets = vec![vec![marker([0.0, 0.0]), marker([50.0, 0.0])]];
        let boxes = [part, nets[0][0].part, nets[0][1].part];
        let wires = route(&boxes, &[], &[], &nets);
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
        let w = &route(&boxes, &[], &[], &nets)[0][0];
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
        let w = &route(&boxes, &[], &[other], &nets)[0][0];
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
        let wires = route(&boxes, &[], &[], &nets);
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
        let wires = route(&boxes, &[], &[], &nets);
        assert!(!on([25.0, 0.0], &wires[0][0]), "{:?}", wires[0][0]);
        assert_eq!(wires[1][0].len(), 2, "{:?}", wires[1][0]);
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
        let wires = route(&boxes, &[], &[], &nets);
        assert_eq!(wires[0].len(), 2);
        // The second branch ends on the first wire, not back at the supply.
        let (a, b) = (&wires[0][0], &wires[0][1]);
        let total: f32 = wires[0]
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
        // A port boxed in by another symbol has no clean route: the plain elbow.
        let nets = vec![vec![marker([0.0, 0.0]), marker([30.0, 20.0])]];
        let walled = [([25.0, 15.0], [35.0, 25.0])];
        assert_eq!(
            route(&walled, &[], &[], &nets)[0],
            [vec![[0.0, 0.0], [30.0, 0.0], [30.0, 20.0]]]
        );
    }
}
