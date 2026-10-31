//! Shared options, paper handling and G-code emission (port of svgToGcode.js core).
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

/// polys in source units -> positioned on the sheet (origin top-left, Y down) -> G-code
pub fn polys_to_conv(polys: Polys, unit: f64, o: &Opt, mut warn: Vec<String>) -> Conv {
    let (pw, ph) = paper(o, &mut warn);
    if polys.is_empty() { warn.push("No drawable shapes found".into()); return Conv { gcode: String::new(), warnings: warn, paths: vec![], bbox: None, paper: [pw, ph], pages: None, page: None }; }
    let [mut x0, mut y0, x1, y1] = bbox(&polys);
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
    let paths: Polys = polys.iter().map(|p| p.iter().map(|v| [(v[0] - x0) * s + ox, (v[1] - y0) * s + oy]).collect()).collect();
    let bb = bbox(&paths);
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
    for p in &paths {
        g.push(format!("G0 X{:.3} Y{:.3}", p[0][0], p[0][1])); g.push(down.clone());
        if zmode { g.push(restore.clone()); }
        for v in &p[1..] { g.push(format!("G1 X{:.3} Y{:.3}", v[0], v[1])); }
        g.push(up.clone());
    }
    g.push("G0 X0 Y0".into());
    Conv { gcode: g.join("\n"), warnings: warn, paths, bbox: Some(bb), paper: [pw, ph], pages: None, page: None }
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
        assert!(g.contains("G0 X0.000 Y0.000"), "starts from the corner");
    }

    #[test]
    fn z_mode_emission() {
        let o = Opt { pen_mode: "z".into(), ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]]], 1.0, &o, vec![]);
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
        assert!(c.gcode.contains("G0 X12.000 Y7.000"));
    }
}
