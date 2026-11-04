//! SVG -> polyline/arc segments. Curves (C/Q/A, circles) are flattened adaptively
//! (error-bounded, not the old fixed 16 chords) and then re-fitted into true circular
//! arcs G2/G3 — the structure program №1 uses, which cuts the line count by an order
//! of magnitude and lets the GRBL planner chain curves without micro-braking.
use crate::gcode::{segs_to_conv, Conv, Opt, Seg, SegPath, SegPolys};
use svgtypes::{SimplePathSegment as S, SimplifyingPathParser};

/// Max deviation of fitted segments from the true curve, in final sheet millimeters.
const FIT_TOL_MM: f64 = 0.02;
/// Arcs with a radius beyond this are treated as lines (a 2 m "circle" is a line).
const MAX_ARC_R: f64 = 2000.0;
/// Never emit a fitted arc sweeping more than ~355 degrees.
const MAX_SWEEP: f64 = 6.2; // rad

fn n(node: &roxmltree::Node, k: &str) -> f64 {
    node.attribute(k).and_then(|v| v.trim().trim_end_matches(|c: char| c.is_alphabetic() || c == '%').parse().ok()).unwrap_or(0.0)
}

fn shape_d(nd: &roxmltree::Node) -> Option<String> {
    Some(match nd.tag_name().name() {
        "path" => nd.attribute("d")?.to_string(),
        "line" => format!("M{} {}L{} {}", n(nd, "x1"), n(nd, "y1"), n(nd, "x2"), n(nd, "y2")),
        "rect" => { let (x, y, w, h) = (n(nd, "x"), n(nd, "y"), n(nd, "width"), n(nd, "height")); format!("M{x} {y}h{w}v{h}h{}z", -w) }
        "polyline" | "polygon" => {
            let v: Vec<f64> = nd.attribute("points")?.split(|c: char| c.is_whitespace() || c == ',').filter_map(|s| s.parse().ok()).collect();
            let mut d = String::new();
            for (i, c) in v.chunks(2).filter(|c| c.len() == 2).enumerate() { d += &format!("{}{} {}", if i == 0 { "M" } else { "L" }, c[0], c[1]); }
            if nd.tag_name().name() == "polygon" { d += "z"; }
            d
        }
        "circle" | "ellipse" => {
            let (cx, cy) = (n(nd, "cx"), n(nd, "cy"));
            let (rx, ry) = if nd.tag_name().name() == "circle" { (n(nd, "r"), n(nd, "r")) } else { (n(nd, "rx"), n(nd, "ry")) };
            let k = 0.5523;
            format!("M{} {cy}C{} {} {} {} {cx} {}C{} {} {} {} {} {cy}C{} {} {} {} {cx} {}C{} {} {} {} {} {cy}z",
                cx + rx, cx + rx, cy + ry * k, cx + rx * k, cy + ry, cy + ry,
                cx - rx * k, cy + ry, cx - rx, cy + ry * k, cx - rx,
                cx - rx, cy - ry * k, cx - rx * k, cy - ry, cy - ry,
                cx + rx * k, cy - ry, cx + rx, cy - ry * k, cx + rx)
        }
        _ => return None,
    })
}

fn d2(a: [f64; 2], b: [f64; 2]) -> f64 { ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() }

/// Distance from p to the infinite line a-b (for the flatness test).
fn dist_line(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (ux, uy) = (b[0] - a[0], b[1] - a[1]);
    let l = (ux * ux + uy * uy).sqrt();
    if l < 1e-12 { return d2(p, a); }
    ((ux * (a[1] - p[1]) - uy * (a[0] - p[0])) / l).abs()
}

/// Distance from p to the segment a-b (for straight-run merging).
fn dist_seg(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (ux, uy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = ux * ux + uy * uy;
    if l2 < 1e-18 { return d2(p, a); }
    let t = (((p[0] - a[0]) * ux + (p[1] - a[1]) * uy) / l2).clamp(0.0, 1.0);
    d2(p, [a[0] + t * ux, a[1] + t * uy])
}

/// Circle through three points: Some((center, radius, ccw)); None when collinear.
fn circum(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Option<([f64; 2], f64, bool)> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 { return None; }
    let (a2, b2, c2) = (a[0] * a[0] + a[1] * a[1], b[0] * b[0] + b[1] * b[1], c[0] * c[0] + c[1] * c[1]);
    let u = [(a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d,
             (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d];
    Some((u, d2(u, a), d > 0.0))
}

/// Adaptive cubic Bézier flattening: recurse until the control points sit within `tol`
/// of the chord (the hull property then bounds the true curve deviation by `tol` too).
fn flat_cubic(cur: &mut Vec<[f64; 2]>, p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], tol: f64, depth: u8) {
    let flat = dist_line(p1, p0, p3) <= tol && dist_line(p2, p0, p3) <= tol;
    if flat || depth >= 18 { cur.push(p3); return; }
    // de Casteljau at t=0.5
    let l01 = [(p0[0] + p1[0]) / 2.0, (p0[1] + p1[1]) / 2.0];
    let l12 = [(p1[0] + p2[0]) / 2.0, (p1[1] + p2[1]) / 2.0];
    let l23 = [(p2[0] + p3[0]) / 2.0, (p2[1] + p3[1]) / 2.0];
    let l012 = [(l01[0] + l12[0]) / 2.0, (l01[1] + l12[1]) / 2.0];
    let l123 = [(l12[0] + l23[0]) / 2.0, (l12[1] + l23[1]) / 2.0];
    let m = [(l012[0] + l123[0]) / 2.0, (l012[1] + l123[1]) / 2.0];
    flat_cubic(cur, p0, l01, l012, m, tol, depth + 1);
    flat_cubic(cur, m, l123, l23, p3, tol, depth + 1);
}

/// Greedy fit of a dense sample run into chains of exact-endpoint arcs and merged lines.
/// Every arc's circle passes through its own start/mid/end samples, so start and end lie
/// EXACTLY on the circle (GRBL validates the endpoint radius and rejects sloppy arcs),
/// and consecutive segments share endpoints exactly, keeping the whole chain continuous.
pub fn fit(pts: &[[f64; 2]], tol: f64) -> SegPath {
    let n = pts.len();
    if n < 2 { return vec![]; }
    let mut out: SegPath = vec![];
    let mut i = 0;
    while i < n - 1 {
        // longest arc [i..=j] whose (start, mid, end) circle keeps every sample within tol
        let mut best: Option<(usize, [f64; 2], bool)> = None;
        let mut j = i + 2;
        while j < n {
            let m = (i + j) / 2;
            let Some((c, r, ccw)) = circum(pts[i], pts[m], pts[j]) else { break };
            if r > MAX_ARC_R || !r.is_finite() { break; }
            let (a0, a1) = ((pts[i][1] - c[1]).atan2(pts[i][0] - c[0]), (pts[j][1] - c[1]).atan2(pts[j][0] - c[0]));
            let sweep = if ccw { (a1 - a0).rem_euclid(2.0 * std::f64::consts::PI) } else { (a0 - a1).rem_euclid(2.0 * std::f64::consts::PI) };
            if sweep > MAX_SWEEP { break; }
            // dense support: every sample gap must be small against the radius (arc chunk <= ~30°).
            // Without this a polygon's sparse corners define a circumcircle with zero deviation
            // and straight edges would bulge into arcs.
            if (i + 1..=j).any(|k| d2(pts[k], pts[k - 1]) > 0.5 * r) { break; }
            if pts[i + 1..j].iter().any(|&p| (d2(p, c) - r).abs() > tol) { break; }
            best = Some((j, c, !ccw));
            j += 1;
        }
        if let Some((j, c, cw)) = best {
            out.push(Seg::A { s: pts[i], e: pts[j], c, cw });
            i = j;
        } else {
            // straight run: extend the chord while every sample stays within tol
            let mut k = i + 1;
            while k + 1 < n && pts[i + 1..k + 1].iter().all(|&p| dist_seg(p, pts[i], pts[k + 1]) <= tol) { k += 1; }
            out.push(Seg::L(pts[k]));
            i = k;
        }
    }
    // anchor: the first segment's implicit start must be explicit for the travel target
    if !out.is_empty() { out.insert(0, Seg::L(pts[0])); }
    out
}

/// SVG path data -> segment paths. `tol` is the fitting tolerance in SOURCE units.
pub fn flatten(d: &str, tol: f64, warn: &mut Vec<String>) -> SegPolys {
    let (mut out, mut cur): (SegPolys, Vec<[f64; 2]>) = (vec![], vec![]);
    let (mut x, mut y, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    let flush = |cur: &mut Vec<[f64; 2]>, out: &mut SegPolys| {
        if cur.len() > 1 {
            let f = fit(cur, tol);
            if !f.is_empty() { out.push(f); }
        }
        cur.clear();
    };
    for seg in SimplifyingPathParser::from(d) {
        let seg = match seg { Ok(s) => s, Err(_) => { warn.push("Bad path data skipped".into()); break; } };
        match seg {
            S::MoveTo { x: a, y: b } => { flush(&mut cur, &mut out); cur.push([a, b]); x = a; y = b; sx = a; sy = b; }
            S::LineTo { x: a, y: b } => { if cur.is_empty() { cur.push([x, y]); } cur.push([a, b]); x = a; y = b; }
            S::Quadratic { x1, y1, x: a, y: b } => {
                if cur.is_empty() { cur.push([x, y]); }
                // elevate the quadratic to a cubic and reuse the same flattener
                let p0 = *cur.last().unwrap();
                let c1 = [p0[0] + 2.0 / 3.0 * (x1 - p0[0]), p0[1] + 2.0 / 3.0 * (y1 - p0[1])];
                let c2 = [a + 2.0 / 3.0 * (x1 - a), b + 2.0 / 3.0 * (y1 - b)];
                flat_cubic(&mut cur, p0, c1, c2, [a, b], tol, 0);
                x = a; y = b;
            }
            S::CurveTo { x1, y1, x2, y2, x: a, y: b } => {
                if cur.is_empty() { cur.push([x, y]); }
                let p0 = *cur.last().unwrap();
                flat_cubic(&mut cur, p0, [x1, y1], [x2, y2], [a, b], tol, 0);
                x = a; y = b;
            }
            S::ClosePath => { if !cur.is_empty() { cur.push([sx, sy]); } flush(&mut cur, &mut out); x = sx; y = sy; }
        }
    }
    flush(&mut cur, &mut out);
    out
}

pub fn svg_to_conv(svg: &str, o: &Opt) -> Result<Conv, String> {
    let doc = roxmltree::Document::parse_with_options(svg, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).map_err(|e| format!("Invalid SVG: {e}"))?;
    let root = doc.root_element();
    let vb: Vec<f64> = root.attribute("viewBox").map(|v| v.split(|c: char| c.is_whitespace() || c == ',').filter_map(|s| s.parse().ok()).collect()).unwrap_or_default();
    let mut unit = 25.4 / 96.0;
    if let Some(w) = root.attribute("width") {
        let t = w.trim(); let k = t.find(|c: char| c.is_alphabetic() || c == '%').unwrap_or(t.len());
        if let (Ok(val), Some(um)) = (t[..k].parse::<f64>(), match &t[k..] { "mm" => Some(1.0), "cm" => Some(10.0), "in" => Some(25.4), "pt" => Some(25.4 / 72.0), "px" | "" => Some(25.4 / 96.0), _ => None }) {
            unit = if vb.len() == 4 && vb[2] != 0.0 { val * um / vb[2] } else { um };
        }
    }
    // fitting tolerance in source units: FIT_TOL_MM on the sheet (auto-fit can only shrink the
    // scale afterwards, which only makes the deviation smaller than the budget)
    let mm_per_unit = (unit * o.scale / 100.0).abs();
    let tol = if mm_per_unit > 1e-9 { (FIT_TOL_MM / mm_per_unit).max(1e-9) } else { 1.0 };
    let mut warn = vec![]; let mut segs: SegPolys = vec![];
    for nd in doc.descendants().filter(|n| n.is_element()) {
        if let Some(d) = shape_d(&nd) {
            if nd.attribute("transform").is_some() { warn.push("transform attribute ignored".into()); }
            segs.extend(flatten(&d, tol, &mut warn));
        }
    }
    warn.dedup();
    Ok(segs_to_conv(segs, unit, o, warn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcode::Opt;

    fn circle_svg(r: f64) -> String {
        format!("<svg xmlns='http://www.w3.org/2000/svg' width='100mm' viewBox='0 0 100 100'><circle cx='50' cy='50' r='{r}'/></svg>")
    }

    /// Rebuild the drawn geometry from the emitted G-code (lines + arcs) while the pen is DOWN
    /// and measure how far it wanders from the true circle — the ultimate guard for the arc
    /// fitter and its G2/G3 direction convention.
    fn gcode_deviation_from_circle(gcode: &str, cx: f64, cy: f64, r: f64) -> (f64, usize, usize) {
        let (mut x, mut y) = (0.0, 0.0);
        let (mut worst, mut n_arc, mut n_line) = (0.0f64, 0usize, 0usize);
        let mut down = false;
        for l in gcode.lines() {
            let t: Vec<&str> = l.split_whitespace().collect();
            let num = |s: &str| s[1..].parse::<f64>().ok();
            match t.first().copied().unwrap_or("") {
                // 0.5.19 convention: M3 (active angle) = pen up, M5 (rest) = pen down
                "M3" => down = false,
                "M5" => down = true,
                "G1" => {
                    let (mut px, mut py) = (None, None);
                    for w in &t[1..] { match w.chars().next() { Some('X') => px = num(w), Some('Y') => py = num(w), _ => {} } }
                    if let (Some(px), Some(py)) = (px, py) {
                        // position advances on every move (travels included), but only pen-down
                        // moves are measured as "drawn" geometry
                        if down {
                            for k in 0..=8 { let f = k as f64 / 8.0; worst = worst.max(((x + (px - x) * f - cx).hypot(y + (py - y) * f - cy) - r).abs()); }
                            n_line += 1;
                        }
                        x = px; y = py;
                    }
                }
                "G2" | "G3" => {
                    let mut v = [None, None, None, None];
                    for w in &t[1..] { match w.chars().next() { Some('X') => v[0] = num(w), Some('Y') => v[1] = num(w), Some('I') => v[2] = num(w), Some('J') => v[3] = num(w), _ => {} } }
                    if let [Some(ex), Some(ey), Some(iv), Some(jv)] = v {
                        let c = [x + iv, y + jv];
                        let cw = t[0] == "G2";
                        let (a0, a1) = ((y - c[1]).atan2(x - c[0]), (ey - c[1]).atan2(ex - c[0]));
                        let sweep = if cw { (a0 - a1).rem_euclid(2.0 * std::f64::consts::PI) } else { (a1 - a0).rem_euclid(2.0 * std::f64::consts::PI) };
                        let r0 = d2([x, y], c);
                        if down {
                            for k in 0..=32 {
                                let a = if cw { a0 - sweep * k as f64 / 32.0 } else { a0 + sweep * k as f64 / 32.0 };
                                worst = worst.max(((c[0] + r0 * a.cos() - cx).hypot(c[1] + r0 * a.sin() - cy) - r).abs());
                            }
                            n_arc += 1;
                        }
                        x = ex; y = ey;
                    }
                }
                _ => {}
            }
        }
        (worst, n_arc, n_line)
    }

    #[test]
    fn rect_arc_polyline() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='100mm' viewBox='0 0 100 100'><rect x='5' y='5' width='20' height='10'/><path d='M10 80 A20 20 0 0 1 50 80'/><polyline points='1,1 2,2 3,3'/></svg>";
        let c = svg_to_conv(&svg, &Opt::default()).unwrap();
        assert!(!c.gcode.is_empty());
        assert!(c.paths.len() >= 3, "three shapes -> three polylines");
        assert!(c.warnings.is_empty());
        assert!((c.paper[0] - 210.0).abs() < 0.1 && (c.paper[1] - 297.0).abs() < 0.1);
    }

    #[test]
    fn mm_width_sets_unit() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='20mm' viewBox='0 0 20 10'><rect x='0' y='0' width='20' height='10'/></svg>";
        let c = svg_to_conv(&svg, &Opt::default()).unwrap();
        let bb = c.bbox.unwrap();
        assert!((bb[2] - bb[0] - 20.0).abs() < 0.01, "20 viewBox units = 20 mm");
    }

    #[test]
    fn circle_becomes_arcs_and_stays_a_circle() {
        // 0.5.12 point 2: the circle must come out as a couple of G2/G3 arcs (not 64 chords),
        // and the drawn geometry must still sit on the true circle within the fitting budget
        let svg = circle_svg(20.0);
        let c = svg_to_conv(&svg, &Opt::default()).unwrap();
        assert!(c.gcode.contains("G2 ") || c.gcode.contains("G3 "), "arcs emitted: {}", c.gcode);
        let motion = c.gcode.lines().filter(|l| l.starts_with("G1 X") || l.starts_with("G2 ") || l.starts_with("G3 ")).count();
        assert!(motion < 20, "a circle must not flatten back into chords ({motion} lines): {}", c.gcode);
        // measure against the SHEET-space circle derived from the emitted bbox
        let bb = c.bbox.unwrap();
        let (ccx, ccy) = ((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0);
        let rr = (bb[2] - bb[0]).min(bb[3] - bb[1]) / 2.0;
        let (dev, n_arc, _) = gcode_deviation_from_circle(&c.gcode, ccx, ccy, rr);
        assert!(n_arc >= 2, "at least two arcs around the circle");
        // budget: 0.02 fit tolerance + 0.0055 kappa-cubic circle approximation + rounding;
        // a wrongly-directioned arc would deviate by ~2*r instead
        assert!(dev <= 0.08, "drawn circle deviates {dev:.3} mm from the true one");
    }

    #[test]
    fn rect_stays_straight_lines() {
        // regression: the fitter must never bend intentionally-straight strokes into arcs
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='100mm' viewBox='0 0 100 100'><rect x='10' y='10' width='40' height='30'/></svg>";
        let c = svg_to_conv(&svg, &Opt::default()).unwrap();
        assert!(!c.gcode.contains("G2 ") && !c.gcode.contains("G3 "), "{}", c.gcode);
    }

    #[test]
    fn circle_flattens() {
        let svg = circle_svg(10.0);
        let c = svg_to_conv(&svg, &Opt::default()).unwrap();
        assert_eq!(c.paths.len(), 1);
        assert!(c.paths[0].len() >= 12, "preview keeps a smooth sampled outline: {}", c.paths[0].len());
    }

    #[test]
    fn transform_warns_but_draws() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='50mm' viewBox='0 0 50 50'><rect x='1' y='1' width='10' height='5' transform='translate(2 2)'/></svg>";
        let c = svg_to_conv(&svg, &Opt::default()).unwrap();
        assert!(c.warnings.iter().any(|s| s.contains("transform")));
        assert!(!c.paths.is_empty());
    }

    #[test]
    fn invalid_svg_is_error() {
        assert!(svg_to_conv("not an svg", &Opt::default()).is_err());
    }

    #[test]
    fn fit_direction_follows_the_samples() {
        // sample a half circle clockwise in number space -> G2 (cw: true), and vice versa
        let mut cw_pts = vec![];
        for k in 0..=32 { let a = std::f64::consts::PI - std::f64::consts::PI * k as f64 / 32.0; cw_pts.push([50.0 + 20.0 * a.cos(), 50.0 + 20.0 * a.sin()]); }
        let segs = fit(&cw_pts, 0.02);
        assert!(matches!(segs.get(1), Some(Seg::A { cw: true, .. })), "clockwise samples -> G2 (after the anchor): {segs:?}");
        let mut ccw_pts = vec![];
        for k in 0..=32 { let a = std::f64::consts::PI + std::f64::consts::PI * k as f64 / 32.0; ccw_pts.push([50.0 + 20.0 * a.cos(), 50.0 + 20.0 * a.sin()]); }
        let segs = fit(&ccw_pts, 0.02);
        assert!(matches!(segs.get(1), Some(Seg::A { cw: false, .. })), "counter-clockwise samples -> G3 (after the anchor): {segs:?}");
    }

    #[test]
    fn fit_merges_collinear_runs() {
        let pts = [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0], [3.0, 1.0], [3.0, 2.0], [3.0, 3.0]];
        let segs = fit(&pts, 0.02);
        assert_eq!(segs.len(), 3, "anchor + two straight runs: {segs:?}");
        assert!(matches!(segs[0], Seg::L([0.0, 0.0])), "anchor first");
        assert!(matches!(segs[1], Seg::L([3.0, 0.0])));
        assert!(matches!(segs[2], Seg::L([3.0, 3.0])));
    }
}
