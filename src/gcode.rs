//! Shared options, paper handling and G-code emission (port of svgToGcode.js core).
//!
//! 0.5.12 paths carry two segment kinds: straight lines and true circular arcs (G2/G3).
//! Sources that already know their geometry (SVG curves fitted in svg.rs) feed segs_to_conv;
//! the point-based converters (text/skeleton/pdf) keep going through polys_to_conv, where every
//! segment is a line. 0.5.12 also travels at the drawing feed in servo mode (G1, like program
//! №1) instead of G0, so travel speed no longer depends on the board's $110/$111 rapid rates.
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct Opt {
    pub paper: String, pub paper_w: f64, pub paper_h: f64, pub margin: f64, pub scale: f64, pub center: bool,
    pub offset_x: f64, pub offset_y: f64, pub feed: f64, pub pen_mode: String, pub z_up: f64, pub z_down: f64,
    pub z_feed: f64, pub servo_down: String, pub servo_up: String, pub landscape: bool, pub laser_off: bool, pub dwell: f64,
    // text
    pub font: String, pub size: f64, pub line_height: f64, pub markdown: bool, pub fill_mode: String,
    /// Infill step for fill/hatch styles, mm; 0 = automatic from the font size
    pub fill_spacing: f64,
    /// Diagonal hatching angle in degrees (hatch style only)
    pub hatch_angle: f64,
    pub start_x: f64, pub start_y: f64, pub page: Option<usize>,
    #[serde(skip)] pub keep_origin: bool,
}
impl Default for Opt {
    fn default() -> Self {
        Opt { paper: "A4".into(), paper_w: 0.0, paper_h: 0.0, margin: 0.0, scale: 100.0, center: false, offset_x: 0.0, offset_y: 0.0,
            feed: 3000.0, pen_mode: "servo".into(), z_up: 5.0, z_down: 0.0, z_feed: 500.0, servo_down: "M3 S255".into(), servo_up: "M5".into(),
            landscape: false, laser_off: true, dwell: 0.15, font: "Roboto".into(), size: 6.0, line_height: 1.4, markdown: false,
            fill_mode: "pen".into(), fill_spacing: 0.0, hatch_angle: 45.0, start_x: 0.0, start_y: 0.0, page: None, keep_origin: false }
    }
}

pub type Polys = Vec<Vec<[f64; 2]>>;

/// One path element: a straight line to `p`, or a circular arc `s -> e` around the center `c`.
/// All three points are absolute; the emitter derives GRBL's I/J as `c - current_position`,
/// so a chain of arcs stays radius-consistent for the board no matter how it was fitted.
/// `cw` selects G2 (clockwise in the machine's number space) vs G3 (counter-clockwise).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    L([f64; 2]),
    A { s: [f64; 2], e: [f64; 2], c: [f64; 2], cw: bool },
}
pub type SegPath = Vec<Seg>;
pub type SegPolys = Vec<SegPath>;

#[derive(Serialize)]
pub struct Conv {
    pub gcode: String, pub warnings: Vec<String>, pub paths: Polys, pub bbox: Option<[f64; 4]>, pub paper: [f64; 2],
    #[serde(skip_serializing_if = "Option::is_none")] pub pages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")] pub page: Option<usize>,
}

/// Sheet size in mm, capped at A4, optionally landscape.
pub fn paper(o: &Opt, warn: &mut Vec<String>) -> (f64, f64) {
    let (mut w, mut h) = match o.paper.as_str() { "A5" => (148.0, 210.0), "Letter" => (215.9, 279.4), "custom" => (o.paper_w, o.paper_h), _ => (210.0, 297.0) };
    if w > 210.0 || h > 297.0 { warn.push("Paper limited to A4 (210×297 mm)".into()); w = w.min(210.0); h = h.min(297.0); }
    if o.landscape { std::mem::swap(&mut w, &mut h); }
    (w, h)
}

pub fn bbox(p: &Polys) -> [f64; 4] {
    let mut b: [f64; 4] = [1e9, 1e9, -1e9, -1e9];
    for q in p { for v in q { b = [b[0].min(v[0]), b[1].min(v[1]), b[2].max(v[0]), b[3].max(v[1])]; } }
    b
}

/// End point of a segment.
pub fn seg_end(s: &Seg) -> [f64; 2] { match s { Seg::L(p) | Seg::A { e: p, .. } => *p } }

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 { ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() }

/// Arc geometry from `from` (the actual current position when emitting): Some((center, radius,
/// signed sweep in radians, start angle)). Signed sweep: negative = clockwise (G2), positive =
/// counter-clockwise (G3). `None` for lines and degenerate/inconsistent arcs (line fallback).
fn arc_geo(from: [f64; 2], seg: &Seg) -> Option<([f64; 2], f64, f64, f64)> {
    let Seg::A { e, c, cw, .. } = seg else { return None };
    let r0 = dist(from, *c);
    let r1 = dist(*e, *c);
    if r0 < 1e-9 || (r0 - r1).abs() > 0.05 * r0 { return None; } // inconsistent radii -> not a valid arc
    let a0 = (from[1] - c[1]).atan2(from[0] - c[0]);
    let a1 = (e[1] - c[1]).atan2(e[0] - c[0]);
    let raw = if *cw { a0 - a1 } else { a1 - a0 }; // positive in the travel direction
    let sweep = raw.rem_euclid(2.0 * std::f64::consts::PI);
    if sweep < 1e-9 { return None; } // zero-length arc
    let r = (r0 + r1) / 2.0;
    Some((*c, r, if *cw { -sweep } else { sweep }, a0))
}

/// Sample an arc for display/bbox: `steps` chords from `from` to the end (start point included).
fn arc_points(from: [f64; 2], seg: &Seg, steps: usize) -> Vec<[f64; 2]> {
    let Some((c, r, sweep, a0)) = arc_geo(from, seg) else { return vec![seg_end(seg)] };
    let n = steps.max(1);
    (0..=n).map(|k| {
        let a = a0 + sweep * (k as f64) / (n as f64);
        [c[0] + r * a.cos(), c[1] + r * a.sin()]
    }).collect()
}

/// Exact bbox contributors of a segment: endpoints plus axis extremes (0/90/180/270°) inside the sweep.
fn seg_bbox_pts(seg: &Seg) -> Vec<[f64; 2]> {
    let Seg::A { s, e, c, cw } = seg else { return vec![seg_end(seg)] };
    let r0 = dist(*s, *c);
    let r1 = dist(*e, *c);
    if r0 < 1e-9 || (r0 - r1).abs() > 0.05 * r0 { return vec![*e]; }
    let a0 = (s[1] - c[1]).atan2(s[0] - c[0]);
    let a1 = (e[1] - c[1]).atan2(e[0] - c[0]);
    let (cw, sweep) = if *cw { (true, (a0 - a1).rem_euclid(2.0 * std::f64::consts::PI)) } else { (false, (a1 - a0).rem_euclid(2.0 * std::f64::consts::PI)) };
    if sweep < 1e-9 { return vec![*e]; }
    let r = (r0 + r1) / 2.0;
    let mut pts = vec![*s, *e];
    for k in 0..4 {
        let ex = std::f64::consts::FRAC_PI_2 * k as f64;
        let d = if cw { (a0 - ex).rem_euclid(2.0 * std::f64::consts::PI) } else { (ex - a0).rem_euclid(2.0 * std::f64::consts::PI) };
        if d <= sweep + 1e-12 { pts.push([c[0] + r * ex.cos(), c[1] + r * ex.sin()]); }
    }
    pts
}

/// polys in source units -> positioned on the sheet (origin top-left, Y down) -> G-code (all lines).
pub fn polys_to_conv(polys: Polys, unit: f64, o: &Opt, warn: Vec<String>) -> Conv {
    let segs: SegPolys = polys.into_iter().map(|p| p.into_iter().map(Seg::L).collect()).collect();
    segs_to_conv(segs, unit, o, warn)
}

/// Segments in source units -> positioned on the sheet (origin top-left, Y down) -> G-code.
pub fn segs_to_conv(segs: SegPolys, unit: f64, o: &Opt, mut warn: Vec<String>) -> Conv {
    let (pw, ph) = paper(o, &mut warn);
    if segs.is_empty() { warn.push("No drawable shapes found".into()); return Conv { gcode: String::new(), warnings: warn, paths: vec![], bbox: None, paper: [pw, ph], pages: None, page: None }; }
    // source-unit bbox over lines and exact arc extremes
    let mut bb0: [f64; 4] = [1e9, 1e9, -1e9, -1e9];
    for path in &segs {
        for seg in path {
            for p in seg_bbox_pts(seg) { bb0 = [bb0[0].min(p[0]), bb0[1].min(p[1]), bb0[2].max(p[0]), bb0[3].max(p[1])]; }
        }
    }
    let [mut x0, mut y0, x1, y1] = bb0;
    let mut s = unit * o.scale / 100.0;
    if o.keep_origin { x0 = 0.0; y0 = 0.0; } // layout coordinates are already in mm from the block origin
    let (mut w, mut h) = ((x1 - x0) * s, (y1 - y0) * s);
    let (aw, ah) = (pw - 2.0 * o.margin, ph - 2.0 * o.margin);
    if !o.keep_origin && (w > aw || h > ah) {
        let f = (aw / w).min(ah / h); s *= f; w *= f; h *= f;
        warn.push(format!("Auto-scaled to {:.0}% to fit paper", f * 100.0));
    }
    let ox = if o.center { (pw - w) / 2.0 } else { o.margin } + o.offset_x;
    let oy = if o.center { (ph - h) / 2.0 } else { o.margin } + o.offset_y;
    let tf = |v: [f64; 2]| [(v[0] - x0) * s + ox, (v[1] - y0) * s + oy];
    // transform: all three arc points move together (uniform scale + translation, no flip)
    let paths: SegPolys = segs.into_iter().map(|p| p.into_iter().map(|seg| match seg {
        Seg::L(v) => Seg::L(tf(v)),
        Seg::A { s, e, c, cw } => Seg::A { s: tf(s), e: tf(e), c: tf(c), cw },
    }).collect()).collect();
    // display polylines (preview): arcs are sampled every ~15 degrees
    let disp: Polys = paths.iter().map(|p| {
        let mut out: Vec<[f64; 2]> = vec![];
        for seg in p {
            match seg {
                Seg::L(v) => out.push(*v),
                Seg::A { s, .. } => {
                    let sweep = arc_geo(*s, seg).map(|(_, _, sw, _)| sw.abs()).unwrap_or(0.0);
                    let steps = (sweep / (std::f64::consts::PI / 12.0)).ceil().max(1.0) as usize;
                    out.extend(arc_points(*s, seg, steps));
                }
            }
        }
        out
    }).collect();
    let bb = bbox(&disp);
    if bb[0] < 0.0 || bb[1] < 0.0 || bb[2] > pw || bb[3] > ph { warn.push("Drawing exceeds paper after offset".into()); }

    let dw = if o.dwell > 0.0 { format!("\nG4 P{}", o.dwell) } else { String::new() };
    let up = format!("{}{}", if o.pen_mode == "z" { format!("G0 Z{}", o.z_up) } else { o.servo_up.clone() }, dw);
    let down = format!("{}{}", if o.pen_mode == "z" { format!("G1 Z{} F{}", o.z_down, o.z_feed) } else { o.servo_down.clone() }, dw);
    let mut g: Vec<String> = vec!["; Plotter Controller — SVG".into()];
    if o.laser_off { g.push("$32=0".into()); }
    // Feed rate is written ONCE at the top of the file (GRBL F is modal — it stays until changed),
    // so the drawing moves carry no F. Only the Z plunge keeps its own F (z_feed), and in that
    // mode the drawing feed is restored right after the plunge. Arc commands (G2/G3) get no F.
    g.extend(["G21 G90".to_string(), format!("G1 F{}", o.feed), up.clone()]);
    let restore = format!("G1 F{}", o.feed); // z mode only: undo the plunge's modal F{z_feed}
    let zmode = o.pen_mode == "z";
    // 0.5.12: travels follow program №1 — G1 at the modal drawing feed in servo mode
    // (the old G0 depended on the board's $110/$111 rapid rates and could crawl);
    // the Z mode keeps rapids for the XY travel and the vertical pen lifts.
    let (trav, ret) = if zmode { ("G0", "G0 X0 Y0") } else { ("G1", "G1 X0 Y0") };
    for p in &paths {
        let Some(first) = p.first() else { continue }; // empty path: nothing to draw
        // travel target: an arc carries its start, a line-first path starts with the anchor
        // L(p0) whose END is the polyline origin (polys_to_conv / fit always emit that anchor)
        let p0 = match first { Seg::L(p) => *p, Seg::A { s, .. } => *s };
        g.push(format!("{trav} X{:.3} Y{:.3}", p0[0], p0[1]));
        g.push(down.clone());
        if zmode { g.push(restore.clone()); }
        let mut cur = p0;
        for seg in p {
            match seg {
                Seg::L(v) => {
                    if dist(cur, *v) <= 1e-9 { continue; } // the anchor point itself: nothing to draw
                    cur = *v; g.push(format!("G1 X{:.3} Y{:.3}", v[0], v[1]));
                }
                Seg::A { e, c, cw, .. } => {
                    // I/J from the real machine position; a broken arc degrades to a line, never an error
                    let ok = arc_geo(cur, seg).is_some() && dist(cur, *e) > 1e-6;
                    let (iv, jv) = (c[0] - cur[0], c[1] - cur[1]);
                    let line = format!("G1 X{:.3} Y{:.3}", e[0], e[1]);
                    cur = *e;
                    g.push(if ok {
                        format!("{} X{:.3} Y{:.3} I{:.3} J{:.3}", if *cw { "G2" } else { "G3" }, e[0], e[1], iv, jv)
                    } else { line });
                }
            }
        }
        g.push(up.clone());
    }
    g.push(ret.into());
    Conv { gcode: g.join("\n"), warnings: warn, paths: disp, bbox: Some(bb), paper: [pw, ph], pages: None, page: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_capped_to_a4_with_warning() {
        let o = Opt { paper: "Letter".into(), ..Default::default() };
        let mut w = vec![];
        let (pw, ph) = paper(&o, &mut w);
        assert!(pw <= 210.01 && ph <= 297.01);
        assert!(w.iter().any(|s| s.contains("A4")));
    }

    #[test]
    fn landscape_swaps_sheet() {
        let o = Opt { landscape: true, ..Default::default() };
        let mut w = vec![];
        let (pw, ph) = paper(&o, &mut w);
        assert!((pw - 297.0).abs() < 0.01 && (ph - 210.0).abs() < 0.01);
    }

    #[test]
    fn servo_emission_with_dwell_and_laser_off() {
        let o = Opt::default();
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 0.0]]], 1.0, &o, vec![]);
        let g = &c.gcode;
        assert!(g.contains("$32=0"), "laser off first");
        assert!(g.contains("M5") && g.contains("M3 S255"), "servo up/down");
        assert!(g.contains("G4 P0.15"), "dwell after pen commands");
        assert!(g.starts_with("; Plotter Controller"));
        assert!(g.contains("G1 X0 Y0"), "return home at the drawing feed (program №1 style)");
    }

    #[test]
    fn servo_travel_runs_at_the_draw_feed_not_g0() {
        // 0.5.12 point 3: in servo mode every XY travel is G1 (modal feed), like program №1
        let o = Opt::default();
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]], vec![[20.0, 0.0], [30.0, 5.0]]], 1.0, &o, vec![]);
        assert!(!c.gcode.contains("G0 X"), "no G0 travels in servo mode: {}", c.gcode);
        assert!(c.gcode.contains("G1 X20.000 Y0.000"), "travel between paths at the draw feed");
    }

    #[test]
    fn z_mode_keeps_rapid_travels() {
        let o = Opt { pen_mode: "z".into(), ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G0 X"), "z mode travel stays G0");
        assert!(c.gcode.contains("G0 Z5") && c.gcode.contains("G1 Z0 F500"));
    }

    #[test]
    fn feed_is_written_once_at_the_top() {
        // 0.5.11 rule: the feed lives in a single `G1 F<feed>` line right after G21 G90;
        // drawing moves carry no F, arcs (G2/G3) get none either.
        let o = Opt::default();
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 0.0]]], 1.0, &o, vec![]);
        let feed_lines: Vec<&str> = c.gcode.lines().filter(|l| l.starts_with("G1 F")).collect();
        assert_eq!(feed_lines, vec!["G1 F3000"], "exactly one feed line, per settings");
        assert!(c.gcode.contains("G21 G90\nG1 F3000"), "feed sits right after the unit/mode setup");
        for l in c.gcode.lines() {
            if l.starts_with("G1 X") || l.starts_with("G2 ") || l.starts_with("G3 ") {
                assert!(!l.contains('F'), "motion line must be F-free: {l}");
            }
        }
    }

    #[test]
    fn z_mode_restores_drawing_feed_after_plunge() {
        // F is modal: the plunge `G1 Z.. F{z_feed}` would otherwise slow every following move.
        let o = Opt { pen_mode: "z".into(), ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]]], 1.0, &o, vec![]);
        let mut lines = c.gcode.lines();
        let mut after_plunge = false;
        let mut restored = false;
        for l in lines.by_ref() {
            if l.starts_with("G1 Z") { after_plunge = true; }
            if after_plunge && l == "G1 F3000" { restored = true; break; }
        }
        assert!(restored, "drawing feed must be restored after the Z plunge");
    }

    #[test]
    fn autoscale_warns_and_fits() {
        let o = Opt::default();
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [1000.0, 0.0], [1000.0, 500.0], [0.0, 500.0], [0.0, 0.0]]], 1.0, &o, vec![]);
        assert!(c.warnings.iter().any(|s| s.contains("Auto-scaled")));
        let bb = c.bbox.unwrap();
        assert!(bb[2] <= 210.01 && bb[3] <= 297.01);
    }

    #[test]
    fn keep_origin_uses_start_point() {
        let o = Opt { keep_origin: true, offset_x: 12.0, offset_y: 7.0, ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G1 X12.000 Y7.000"), "servo travel is G1 now: {}", c.gcode);
    }

    #[test]
    fn arcs_emit_g2_g3_with_ij() {
        // 270-degree CCW arc: start (0,0), center (10,0), end (10,10) -> G3 I10 J0
        // (the transform shifts the source bbox onto the sheet, hence Y + 10 in the output)
        let o = Opt::default();
        let segs: SegPolys = vec![vec![Seg::A { s: [0.0, 0.0], e: [10.0, 10.0], c: [10.0, 0.0], cw: false }]];
        let c = segs_to_conv(segs, 1.0, &o, vec![]);
        assert!(c.gcode.contains("G3 X10.000 Y20.000 I10.000 J0.000"), "{}", c.gcode);
        // clockwise mirror -> G2
        let segs: SegPolys = vec![vec![Seg::A { s: [0.0, 0.0], e: [10.0, -10.0], c: [10.0, 0.0], cw: true }]];
        let c = segs_to_conv(segs, 1.0, &o, vec![]);
        assert!(c.gcode.contains("G2 X10.000 Y0.000 I10.000 J0.000"), "{}", c.gcode);
    }

    #[test]
    fn degenerate_arc_falls_back_to_a_line() {
        // start == end: GRBL would refuse the arc, the emitter must write a plain G1
        let o = Opt::default();
        let segs: SegPolys = vec![vec![Seg::A { s: [0.0, 0.0], e: [0.0, 0.0], c: [5.0, 0.0], cw: false }]];
        let c = segs_to_conv(segs, 1.0, &o, vec![]);
        assert!(!c.gcode.contains("G2 ") && !c.gcode.contains("G3 "), "{}", c.gcode);
        assert!(c.gcode.contains("G1 X0.000 Y0.000"));
    }

    #[test]
    fn arc_bbox_includes_bulge() {
        // 3/4 circle around (0,0) from (10,0) CCW to (0,-10): passes 90° (0,10) and 180° (-10,0),
        // so the bbox must be [-10..10]² even though the endpoints only span one quadrant
        let o = Opt::default();
        let segs: SegPolys = vec![vec![Seg::A { s: [10.0, 0.0], e: [0.0, -10.0], c: [0.0, 0.0], cw: false }]];
        let c = segs_to_conv(segs, 1.0, &o, vec![]);
        let bb = c.bbox.unwrap();
        // source bbox [-10..10]² is shifted by the transform to [0..20]² on the sheet
        assert!((bb[0] - 0.0).abs() < 0.01 && (bb[1] - 0.0).abs() < 0.01 && (bb[2] - 20.0).abs() < 0.01 && (bb[3] - 20.0).abs() < 0.01, "bbox {bb:?}");
    }
}
