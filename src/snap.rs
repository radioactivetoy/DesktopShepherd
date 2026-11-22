//! Snapping a box to the other boxes and to the screen edges while it is moved or resized.
//!
//! A box edge that comes within `dist` of a snap line jumps onto it:
//! * next to another box, leaving `gap` between them (my left edge to its right edge + gap, …);
//! * lined up with another box (my left edge to its left edge, my top to its top, …);
//! * the screen's usable area (without the taskbar), `gap` inside its edges.
//!
//! Other boxes only count when they are near in the other direction (side by side or stacked),
//! so a box far away on the screen doesn't pull. All values are in one unit chosen by the
//! caller (pixels for moving, points for resizing). Pure arithmetic: no Windows calls.

/// A rectangle by its edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl R {
    pub fn new(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self { l, t, r, b }
    }
}

/// What a moving/resizing box can snap to.
#[derive(Clone, Debug, Default)]
pub struct Snap {
    /// The other boxes.
    pub boxes: Vec<R>,
    /// The usable area of each monitor.
    pub screens: Vec<R>,
    /// How close an edge must come to jump (e.g. 10 points).
    pub dist: f32,
    /// Space left between neighbouring boxes and inside the screen edges.
    pub gap: f32,
    /// Optional grid (0 = off): when no box or screen edge is close, edges land on multiples
    /// of this size.
    pub grid: f32,
}

impl Snap {
    /// How far apart two ranges may be (in the other direction) for boxes to count as neighbours.
    fn near(&self) -> f32 {
        self.gap + self.dist * 4.0
    }

    /// Snap lines for vertical edges (x values) of a box spanning `t..b` vertically.
    fn lines_x(&self, t: f32, b: f32, out: &mut Vec<(f32, Side)>) {
        let near = self.near();
        for o in &self.boxes {
            if t < o.b + near && b > o.t - near {
                out.push((o.r + self.gap, Side::Low)); // my left edge right of it
                out.push((o.l - self.gap, Side::High)); // my right edge left of it
                out.push((o.l, Side::Low)); // left edges lined up
                out.push((o.r, Side::High)); // right edges lined up
            }
        }
        for s in &self.screens {
            if t < s.b && b > s.t {
                out.push((s.l + self.gap, Side::Low));
                out.push((s.r - self.gap, Side::High));
            }
        }
    }

    /// Snap lines for horizontal edges (y values) of a box spanning `l..r` horizontally.
    fn lines_y(&self, l: f32, r: f32, out: &mut Vec<(f32, Side)>) {
        let near = self.near();
        for o in &self.boxes {
            if l < o.r + near && r > o.l - near {
                out.push((o.b + self.gap, Side::Low)); // my top below it
                out.push((o.t - self.gap, Side::High)); // my bottom above it
                out.push((o.t, Side::Low)); // tops lined up
                out.push((o.b, Side::High)); // bottoms lined up
            }
        }
        for s in &self.screens {
            if l < s.r && r > s.l {
                out.push((s.t + self.gap, Side::Low));
                out.push((s.b - self.gap, Side::High));
            }
        }
    }

    /// The shift that puts `v` on the nearest grid line (0 when the grid is off).
    fn to_grid(&self, v: f32) -> f32 {
        if self.grid >= 2.0 {
            (v / self.grid).round() * self.grid - v
        } else {
            0.0
        }
    }

    /// Where a box being moved should go: `m` is where the mouse puts it; returns its snapped
    /// top-left corner (its size never changes).
    pub fn snap_move(&self, m: R) -> (f32, f32) {
        let mut lines = Vec::new();
        self.lines_x(m.t, m.b, &mut lines);
        let dx = best(&lines, m.l, m.r, self.dist).unwrap_or_else(|| self.to_grid(m.l));
        lines.clear();
        self.lines_y(m.l, m.r, &mut lines);
        let dy = best(&lines, m.t, m.b, self.dist).unwrap_or_else(|| self.to_grid(m.t));
        (m.l + dx, m.t + dy)
    }

    /// Where the right edge of a box being resized should go (`x`; the box spans `t..b`).
    pub fn snap_right(&self, x: f32, t: f32, b: f32) -> f32 {
        let mut lines = Vec::new();
        self.lines_x(t, b, &mut lines);
        x + best_one(&lines, x, Side::High, self.dist).unwrap_or_else(|| self.to_grid(x))
    }

    /// Where the bottom edge of a box being resized should go (`y`; the box spans `l..r`).
    pub fn snap_bottom(&self, y: f32, l: f32, r: f32) -> f32 {
        let mut lines = Vec::new();
        self.lines_y(l, r, &mut lines);
        y + best_one(&lines, y, Side::High, self.dist).unwrap_or_else(|| self.to_grid(y))
    }
}

/// Which edge of the moving box a line is for: the low one (left/top) or the high one
/// (right/bottom).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Side {
    Low,
    High,
}

/// The smallest shift (within `dist`) that puts the low edge `lo` or the high edge `hi` on a
/// line meant for it; None if none is close enough.
fn best(lines: &[(f32, Side)], lo: f32, hi: f32, dist: f32) -> Option<f32> {
    let mut shift = None;
    let mut found = dist;
    for &(v, side) in lines {
        let edge = if side == Side::Low { lo } else { hi };
        let d = v - edge;
        if d.abs() <= found {
            found = d.abs();
            shift = Some(d);
        }
    }
    shift
}

fn best_one(lines: &[(f32, Side)], edge: f32, want: Side, dist: f32) -> Option<f32> {
    let mut shift = None;
    let mut found = dist;
    for &(v, side) in lines {
        if side != want {
            continue;
        }
        let d = v - edge;
        if d.abs() <= found {
            found = d.abs();
            shift = Some(d);
        }
    }
    shift
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snap {
        Snap { boxes: vec![R::new(100.0, 100.0, 300.0, 300.0)], screens: vec![R::new(0.0, 0.0, 1000.0, 800.0)], dist: 10.0, gap: 8.0, grid: 0.0 }
    }

    #[test]
    fn snaps_beside_another_box_with_gap() {
        // 50 wide box dropped 5 px from the right side of the other box (+gap).
        let (x, y) = snap().snap_move(R::new(313.0, 150.0, 363.0, 200.0));
        assert_eq!((x, y), (308.0, 150.0));
    }

    #[test]
    fn lines_up_left_edges_when_stacked() {
        let (x, y) = snap().snap_move(R::new(104.0, 312.0, 204.0, 362.0));
        assert_eq!((x, y), (100.0, 308.0)); // left edges aligned, top just below with the gap
    }

    #[test]
    fn far_boxes_do_not_pull() {
        // Same x as the other box's right edge, but far below it: only the screen counts.
        let (x, _) = snap().snap_move(R::new(304.0, 600.0, 354.0, 650.0));
        assert_eq!(x, 304.0);
    }

    #[test]
    fn snaps_to_screen_edge_inside_gap() {
        let (x, y) = snap().snap_move(R::new(3.0, 790.0 - 50.0, 53.0, 795.0));
        assert_eq!((x, y), (8.0, 737.0));
    }

    #[test]
    fn nothing_close_means_no_change() {
        let (x, y) = snap().snap_move(R::new(500.0, 500.0, 560.0, 560.0));
        assert_eq!((x, y), (500.0, 500.0));
    }

    #[test]
    fn grid_when_nothing_is_close() {
        let mut s = snap();
        s.grid = 20.0;
        // Far from everything: the top-left lands on the 20 grid.
        assert_eq!(s.snap_move(R::new(507.0, 493.0, 567.0, 553.0)), (500.0, 500.0));
        // Next to the other box: the box edge wins over the grid (x), grid for y.
        assert_eq!(s.snap_move(R::new(313.0, 151.0, 363.0, 201.0)), (308.0, 160.0));
        assert_eq!(s.snap_right(611.0, 500.0, 560.0), 620.0);
    }

    #[test]
    fn resize_edges_snap() {
        let s = snap();
        assert_eq!(s.snap_right(95.0, 120.0, 200.0), 92.0); // just left of the box, with gap
        assert_eq!(s.snap_bottom(297.0, 120.0, 200.0), 300.0); // bottoms lined up
        assert_eq!(s.snap_right(600.0, 120.0, 200.0), 600.0); // nothing near
    }
}
