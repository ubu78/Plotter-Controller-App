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
    /// 0.5.22: per-side sheet margins in mm — left, right, top, bottom. While all four are
    /// zero the legacy single `margin` applies to every side (old files / API compatibility).
    /// left/top shift the drawing, right/bottom bound the printable area for auto-scale.
    pub margin_l: f64, pub margin_r: f64, pub margin_t: f64, pub margin_b: f64,
    pub offset_x: f64, pub offset_y: f64, pub feed: f64, pub pen_mode: String, pub z_up: f64, pub z_down: f64,
    pub z_feed: f64, pub landscape: bool, pub laser_off: bool, pub dwell: f64,
    /// 0.5.22: separate feed for pen-up XY moves (path-to-path hops and the return home).
    /// 0 keeps the old behavior: servo travels ride the modal drawing feed, Z-mode rapids
    /// stay G0 at the board's $110/$111 max rates. A value > 0 turns every XY travel into
    /// `G1 ... F<travel_feed>` (both pen modes — a feed move honors the exact number), so
    /// travel time stops depending on the drawing feed or the board's rapid settings.
    pub travel_feed: f64,
    /// 0.5.19: the default servo convention after the author flipped the servo horn —
    /// the ACTIVE angle (M3 S<val>) lifts the pen, M5 (spindle stop, pin low = rest) lowers it.
    /// Both stay free-form commands editable in the pen panel.
    pub servo_down: String, pub servo_up: String,
    /// 0.5.15: mirror the drawing vertically (for machines whose Y+ travels "up the page")
    pub invert_y: bool,
    /// 0.5.18: where the machine's Y zero sits relative to the SHEET: "top" (default, SVG
    /// convention: Y = distance from the top edge, what the preview shows), "bottom" (standard
    /// CNC: zero at the bottom edge of the sheet, Y travels up — the drawing lands at the top
    /// of the real sheet) or "reverse" (Y = -Y: feed-style machines whose Y+ pulls the sheet
    /// backwards; negated coordinates make the sheet feed forward instead)
    pub y_origin: String,
    /// 0.5.30: SVG drawing mode — "standard" (default: every shape traced along its own
    /// geometry, filled shapes as contours) or "single" (fills and thick strokes are thinned
    /// to one centerline by the skeleton tracer, like the text «Ручка» style).
    pub svg_mode: String,
    // text
    pub font: String, pub size: f64, pub line_height: f64, pub markdown: bool, pub fill_mode: String,
    /// Infill step for fill/hatch styles, mm; 0 = automatic from the font size
    pub fill_spacing: f64,
    /// Diagonal hatching angle in degrees (hatch style only)
    pub hatch_angle: f64,
    /// 0.5.31: hatch pattern variant (letter style «Штриховка»): "lines" (default, the old
    /// diagonal scanlines), "dots" (staggered grid of pen dots), "triangle" (regular zigzag
    /// saw-teeth) or "lightning" (irregular seeded zigzag). "fill" ignores it — always solid.
    pub hatch_style: String,
    /// 0.5.26: text alignment inside the printable width (from start_x to the right margin):
    /// "left" (default, as before) / "center" / "right" shift whole lines;
    /// "justify" stretches inter-word spaces of every continuing line to the right edge
    pub align: String,
    pub start_x: f64, pub start_y: f64, pub page: Option<usize>,
    #[serde(skip)] pub keep_origin: bool,
}
impl Default for Opt {
    fn default() -> Self {
        Opt { paper: "A4".into(), paper_w: 0.0, paper_h: 0.0, margin: 0.0, margin_l: 0.0, margin_r: 0.0, margin_t: 0.0, margin_b: 0.0, scale: 100.0, center: false, offset_x: 0.0, offset_y: 0.0,
            feed: 3000.0, pen_mode: "servo".into(), z_up: 5.0, z_down: 0.0, z_feed: 500.0, servo_down: "M5".into(), servo_up: "M3 S255".into(),
            travel_feed: 0.0, landscape: false, laser_off: true, dwell: 0.15, invert_y: false, y_origin: "top".into(), svg_mode: "standard".into(), font: "Roboto".into(), size: 6.0, line_height: 1.4, markdown: false,
            fill_mode: "pen".into(), fill_spacing: 0.0, hatch_angle: 45.0, hatch_style: "lines".into(), align: "left".into(), start_x: 0.0, start_y: 0.0, page: None, keep_origin: false }
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

/// 0.5.28: identity of one part inside a program. It is written as a `; [PART <id>]`
/// comment before the part's first travel; the marker block in the UI scans these
/// comments and can assign every part its own marker (an M0 pause is inserted at every
/// boundary where the marker must change). Infill = the fill/hatch infill of the TEXT —
/// it is split from the outlines so it can run with a different marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartId { Svg, Text, Infill }
impl PartId {
    pub fn as_str(self) -> &'static str { match self { PartId::Svg => "svg", PartId::Text => "text", PartId::Infill => "infill" } }
}

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

/// Effective per-side margins (left, right, top, bottom) in mm. 0.5.22: the four per-side
/// values rule as soon as any of them is set; a legacy single `margin` spreads over all four.
pub fn margins(o: &Opt) -> (f64, f64, f64, f64) {
    if o.margin_l > 0.0 || o.margin_r > 0.0 || o.margin_t > 0.0 || o.margin_b > 0.0 {
        (o.margin_l, o.margin_r, o.margin_t, o.margin_b)
    } else { (o.margin, o.margin, o.margin, o.margin) }
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

/// Mirror all paths around the horizontal line y = ky/2 (y' = ky - y). Mirroring reverses
/// orientation, so every arc swaps G2 <-> G3; I/J are derived from the moved points by the
/// emitter and stay exact. Shared by «Invert Y» (0.5.15) and «Y origin» (0.5.18).
fn mirror_y(paths: SegPolys, ky: f64) -> SegPolys {
    paths.into_iter().map(|p| p.into_iter().map(|seg| match seg {
        Seg::L(v) => Seg::L([v[0], ky - v[1]]),
        Seg::A { s, e, c, cw } => Seg::A { s: [s[0], ky - s[1]], e: [e[0], ky - e[1]], c: [c[0], ky - c[1]], cw: !cw },
    }).collect()).collect()
}

/// One source inside a combined program: positioned paths in mm + the drawing feed the part
/// runs at (the text letter-style multiplier changes it — a modal `G1 F` line switches)
/// and the part identity for the `; [PART <id>]` comment (0.5.28 marker colors).
pub struct Part { pub id: PartId, pub feed: f64, pub paths: SegPolys }

/// Segments in source units -> positioned on the sheet (origin top-left, Y down; the whole
/// transform pipeline: auto-fit, per-side margins, center, offsets, invert-Y, Y-origin).
/// Shared by every converter and by the combined SVG+Text program (0.5.27).
fn position_paths(segs: SegPolys, unit: f64, o: &Opt, pw: f64, ph: f64, warn: &mut Vec<String>) -> SegPolys {
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
    // 0.5.22: per-side margins bound the printable area; a degenerate area (margins >= sheet)
    // is clamped so the auto-scale keeps a sane factor instead of flipping sign
    let (ml, mr, mt, mb) = margins(o);
    if ml + mr >= pw || mt + mb >= ph { warn.push("Margins leave no room on the sheet".into()); }
    let (aw, ah) = ((pw - ml - mr).max(1.0), (ph - mt - mb).max(1.0));
    if !o.keep_origin && (w > aw || h > ah) {
        let f = (aw / w).min(ah / h); s *= f; w *= f; h *= f;
        warn.push(format!("Auto-scaled to {:.0}% to fit paper", f * 100.0));
    }
    // centered between the margins (with uniform margins this equals the old full-sheet center)
    let ox = if o.center { ml + (aw - w) / 2.0 } else { ml } + o.offset_x;
    let oy = if o.center { mt + (ah - h) / 2.0 } else { mt } + o.offset_y;
    let tf = |v: [f64; 2]| [(v[0] - x0) * s + ox, (v[1] - y0) * s + oy];
    // transform: all three arc points move together (uniform scale + translation, no flip)
    let paths: SegPolys = segs.into_iter().map(|p| p.into_iter().map(|seg| match seg {
        Seg::L(v) => Seg::L(tf(v)),
        Seg::A { s, e, c, cw } => Seg::A { s: tf(s), e: tf(e), c: tf(c), cw },
    }).collect()).collect();
    // 0.5.15: optional vertical mirror for machines whose Y+ travels "up the page" (the usual
    // CNC direction: origin at the start corner, Y away from the operator). The sheet space
    // here is Y-down from the top-left corner (SVG convention); a Y-up machine turns that
    // into a vertical mirror on paper. The flip runs around the drawing's own bbox, so the
    // plotted footprint does not move — only the orientation flips. Mirroring reverses
    // orientation, so every arc swaps G2 <-> G3; I/J are derived from the mirrored points
    // and stay exact.
    let paths: SegPolys = if o.invert_y {
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for path in &paths {
            for seg in path {
                for p in seg_bbox_pts(seg) { b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]; }
            }
        }
        let my = if b[1].is_finite() { b[1] + b[3] } else { 0.0 }; // 2 × mid-Y
        mirror_y(paths, my)
    } else { paths };
    // 0.5.18: the sheet space above counts Y from the TOP edge (SVG convention — what the
    // preview shows). Machines that count Y the other way round need the whole sheet mirrored:
    // "bottom" flips around the sheet height, so the drawing moves to the OPPOSITE edge of the
    // real sheet (a letterhead previewed at the top is plotted at the top of an A4 sheet loaded
    // with its bottom edge at the machine zero); "reverse" flips around Y=0, giving negative
    // coordinates — for feed machines whose Y+ scrolls the sheet backwards: the sheet then
    // feeds forward and the pen draws the distance-from-the-top-edge content correctly. Both
    // are mirrors, so arcs swap G2 <-> G3; I/J stay exact (recomputed from the moved points).
    match o.y_origin.as_str() {
        "bottom" => mirror_y(paths, ph),
        "reverse" => mirror_y(paths, 0.0),
        _ => paths,
    }
}

/// Sample a fitted path back to display points: lines as-is, arcs as ~15° chords from their
/// own start. Shared by the preview in emit_program and by the SVG single-line mode
/// (the skeleton rasterizer consumes point chains, not arcs).
pub fn segpath_points(p: &SegPath) -> Vec<[f64; 2]> {
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
}

/// Positioned paths (mm) -> one G-code program. `title` names the sources in the header
/// comment; several parts (0.5.27: SVG + Text) share one header and one return home —
/// between them the pen is already up, so the next part's first travel becomes the hop,
/// and a modal `G1 F` line switches to that part's own drawing feed. Pub since 0.5.28:
/// text_to_conv assembles its own parts (outline + infill for different markers).
pub fn emit_program(title: &str, parts: &[Part], o: &Opt, pw: f64, ph: f64, mut warn: Vec<String>) -> Conv {
    // display polylines (preview): arcs are sampled every ~15 degrees
    let disp: Polys = parts.iter().flat_map(|pt| pt.paths.iter().map(segpath_points)).collect();
    let bb = bbox(&disp);
    // "reverse" intentionally produces negative Y (the sheet top edge starts at the machine zero
    // and Y+ points away from the sheet) — that is not an "exceeds paper" situation
    let neg_y_ok = o.y_origin == "reverse";
    if bb[0] < 0.0 || bb[2] > pw || bb[3] > ph || (!neg_y_ok && bb[1] < 0.0) { warn.push("Drawing exceeds paper after offset".into()); }

    let dw = if o.dwell > 0.0 { format!("\nG4 P{}", o.dwell) } else { String::new() };
    let up = format!("{}{}", if o.pen_mode == "z" { format!("G0 Z{}", o.z_up) } else { o.servo_up.clone() }, dw);
    let down = format!("{}{}", if o.pen_mode == "z" { format!("G1 Z{} F{}", o.z_down, o.z_feed) } else { o.servo_down.clone() }, dw);
    let mut g: Vec<String> = vec![format!("; Plotter Controller — {title}")];
    if o.laser_off { g.push("$32=0".into()); }
    // Feed rate is written ONCE at the top of the file (GRBL F is modal — it stays until changed),
    // so the drawing moves carry no F. Only the Z plunge keeps its own F (z_feed), and in that
    // mode the drawing feed is restored right after the plunge. Arc commands (G2/G3) get no F.
    // 0.5.27: a combined program re-arms the feed per part (the text letter-style multiplier).
    let mut cur_feed = parts.first().map(|p| p.feed).unwrap_or(o.feed);
    g.extend(["G21 G90".to_string(), format!("G1 F{cur_feed}"), up.clone()]);
    let zmode = o.pen_mode == "z";
    // 0.5.12: travels follow program №1 — G1 at the modal drawing feed in servo mode
    // (the old G0 depended on the board's $110/$111 rapid rates and could crawl);
    // the Z mode keeps rapids for the XY travel and the vertical pen lifts.
    // 0.5.22: when a travel speed is set, every pen-up XY move becomes `G1 ... F{travel_feed}`
    // in BOTH modes (a plain feed move honors the exact number on any GRBL board) and the
    // drawing feed is re-armed right after the pen down — F is modal, without the
    // restore the whole path would draw at the travel speed. The vertical Z lifts stay G0.
    let tset = o.travel_feed > 0.0 && o.travel_feed != o.feed;
    let (trav, ret) = if zmode && !tset { ("G0", "G0 X0 Y0") } else { ("G1", "G1 X0 Y0") };
    for part in parts {
        // 0.5.28: the part marker comment sits right after the previous part's pen-up —
        // the marker block (UI) uses it to find the boundaries where an M0 pause goes.
        g.push(format!("; [PART {}]", part.id.as_str()));
        if part.feed != cur_feed { g.push(format!("G1 F{}", part.feed)); cur_feed = part.feed; }
        let restore = format!("G1 F{}", part.feed); // undo the plunge's F{z_feed} (z mode) or a travel F
        for p in &part.paths {
            let Some(first) = p.first() else { continue }; // empty path: nothing to draw
            // travel target: an arc carries its start, a line-first path starts with the anchor
            // L(p0) whose END is the polyline origin (polys_to_conv / fit always emit that anchor)
            let p0 = match first { Seg::L(p) => *p, Seg::A { s, .. } => *s };
            if tset { g.push(format!("{trav} X{:.3} Y{:.3} F{}", p0[0], p0[1], o.travel_feed)); }
            else { g.push(format!("{trav} X{:.3} Y{:.3}", p0[0], p0[1])); }
            g.push(down.clone());
            if zmode || tset { g.push(restore.clone()); }
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
    }
    g.push(if tset { format!("{ret} F{}", o.travel_feed) } else { ret.into() }); // the return home is a travel too
    Conv { gcode: g.join("\n"), warnings: warn, paths: disp, bbox: Some(bb), paper: [pw, ph], pages: None, page: None }
}

/// Segments in source units -> positioned on the sheet (origin top-left, Y down) -> G-code.
pub fn segs_to_conv(segs: SegPolys, unit: f64, o: &Opt, mut warn: Vec<String>) -> Conv {
    let (pw, ph) = paper(o, &mut warn);
    if segs.is_empty() { warn.push("No drawable shapes found".into()); return Conv { gcode: String::new(), warnings: warn, paths: vec![], bbox: None, paper: [pw, ph], pages: None, page: None }; }
    let paths = position_paths(segs, unit, o, pw, ph, &mut warn);
    emit_program("SVG", &[Part { id: PartId::Svg, feed: o.feed, paths }], o, pw, ph, warn)
}

/// 0.5.28: position SEVERAL path groups of one source as a single transform unit (common
/// bbox for the auto-scale and the invert-Y / Y-origin mirrors), then split the positioned
/// paths back by group. Used by the text converter to keep the infill and the outlines
/// aligned when the infill becomes its own part for a different marker.
pub fn position_split(groups: Vec<Polys>, unit: f64, o: &Opt, pw: f64, ph: f64, warn: &mut Vec<String>) -> Vec<SegPolys> {
    let mut lens: Vec<usize> = Vec::new();
    let mut all: SegPolys = Vec::new();
    for g in groups {
        lens.push(g.len());
        all.extend(g.into_iter().map(|p| p.into_iter().map(Seg::L).collect()));
    }
    let pos = position_paths(all, unit, o, pw, ph, warn);
    let mut out: Vec<SegPolys> = Vec::with_capacity(lens.len());
    let mut rest: &[SegPath] = &pos;
    for l in lens { out.push(rest[..l].to_vec()); rest = &rest[l..]; }
    out
}

/// 0.5.28 marker colors: insert an M0 program pause at every part boundary where the
/// marker must change. `colors` maps part ids ("svg" / "text" / "infill") to marker
/// names; an empty value means "keep the current marker". The inserted block lifts the
/// pen explicitly, dwells and pauses (M0) — the operator swaps the marker and presses
/// Resume (cycle start). Comparison is case-insensitive on the trimmed names.
/// Idempotent: previously inserted blocks (`; [MARKER: …]` … M0 inclusive) are removed
/// first, so re-applying with new colors rewrites the pauses in place.
pub fn apply_marker_colors(gcode: &str, colors: &std::collections::BTreeMap<String, String>, penup: &str, dwell: f64) -> (String, usize) {
    // pass 1: strip any previously inserted marker blocks (block = `; [MARKER:` line up to
    // and including its M0; a small cap guards against a truncated block)
    let ls: Vec<&str> = gcode.lines().collect();
    let mut base: Vec<&str> = Vec::new();
    let mut k = 0usize;
    while k < ls.len() {
        if ls[k].trim_start().starts_with("; [MARKER:") {
            let mut j = k;
            while j < ls.len() && j - k < 6 && ls[j].trim() != "M0" { j += 1; }
            k = if j < ls.len() && ls[j].trim() == "M0" { j + 1 } else { k + 1 }; // truncated: keep the M0
            continue;
        }
        base.push(ls[k]); k += 1;
    }
    // pass 2: the parts of the program, in print order
    let parts: Vec<(usize, &str)> = base.iter().enumerate().filter_map(|(i, l)| {
        l.trim_start().strip_prefix("; [PART ").map(|r| (i, r.trim_end_matches(']').trim()))
    }).collect();
    if parts.len() < 2 { return (base.join("\n"), 0); } // nothing to change between/inside
    let col = |id: &str| colors.get(id).map(|s| s.trim().to_string()).unwrap_or_default();
    let cols: Vec<String> = parts.iter().map(|(_, id)| col(id)).collect();
    // pass 3: boundaries where the next part needs a DIFFERENT marker (empty = keep current,
    // so it never triggers a pause; the first part never gets one — it starts as installed)
    let lc = |s: &str| s.to_lowercase();
    let ins: Vec<(usize, &str)> = (1..parts.len())
        .filter(|&k| !cols[k].is_empty() && lc(&cols[k]) != lc(&cols[k - 1]))
        .map(|k| (parts[k].0, cols[k].as_str())).collect();
    let mut out: Vec<String> = Vec::with_capacity(base.len() + ins.len() * 4);
    let mut it = ins.iter(); let mut cur = it.next(); let mut pauses = 0usize;
    for (i, l) in base.iter().enumerate() {
        while let Some((li, c)) = cur {
            if *li != i { break; }
            out.push(format!("; [MARKER: {c}] swap the marker, then press Resume"));
            if !penup.trim().is_empty() { out.push(penup.trim().to_string()); }
            if dwell > 0.0 { out.push(format!("G4 P{}", dwell)); }
            out.push("M0".into());
            pauses += 1;
            cur = it.next();
        }
        out.push((*l).to_string());
    }
    (out.join("\n"), pauses)
}

/// 0.5.27: ONE print from BOTH blocks — the SVG drawing and the text (typed or loaded from
/// a .txt/.md/.docx file) — as a single G-code program: one header, one return home; between
/// the parts the pen is already up, so the first travel of the next part becomes the hop.
/// Each part keeps its own placement on the SAME sheet (SVG: scale/center/drag offset; text:
/// Start X/Y, font, letter style, alignment) and its own drawing feed (the letter-style
/// multiplier, switched by a modal `G1 F`). Either source may be empty — that side is simply
/// skipped. `options` carries the sheet/pen settings; `text_options` only the text panel's
/// own fields (font, size, style, alignment, Start X/Y).
pub fn combine_to_conv(svg_src: &str, text_src: &str, o: &Opt, to: &Opt) -> Result<Conv, String> {
    let mut warn = vec![];
    let (pw, ph) = paper(o, &mut warn);
    let mut parts: Vec<Part> = vec![];
    let (mut pages, mut page, mut has_text) = (1usize, 0usize, false);
    if !svg_src.trim().is_empty() {
        // 0.5.30: the SVG part honors the drawing mode — "single" returns skeleton strokes
        // already positioned on the sheet, "standard" positions the fitted segments as before
        let paths = if o.svg_mode == "single" {
            crate::svg::svg_single_paths(svg_src, o, pw, ph, &mut warn)?
        } else {
            let (segs, unit) = crate::svg::svg_to_segs(svg_src, o, &mut warn)?;
            position_paths(segs, unit, o, pw, ph, &mut warn)
        };
        if !paths.is_empty() { parts.push(Part { id: PartId::Svg, feed: o.feed, paths }); }
    }
    if !text_src.trim().is_empty() {
        // text-panel fields from text_options; sheet, margins, pen and the base feed stay shared
        let t = Opt { font: to.font.clone(), size: to.size, line_height: to.line_height, markdown: to.markdown,
            fill_mode: to.fill_mode.clone(), fill_spacing: to.fill_spacing, hatch_angle: to.hatch_angle, hatch_style: to.hatch_style.clone(),
            align: to.align.clone(), start_x: to.start_x, start_y: to.start_y, page: to.page, ..o.clone() };
        let tp = crate::text::text_to_parts(text_src, &t, &mut warn)?;
        let (pgs, pgi) = (tp.pages, tp.page);
        if !tp.main.is_empty() {
            let mut o2 = t.clone(); o2.keep_origin = true; o2.margin = 0.0; o2.margin_l = 0.0; o2.margin_r = 0.0; o2.margin_t = 0.0; o2.margin_b = 0.0;
            o2.center = false; o2.scale = 100.0; o2.offset_x = t.start_x; o2.offset_y = t.start_y;
            o2.feed *= crate::text::feed_mult(&t.fill_mode);
            // 0.5.28: the fill/hatch infill becomes its own part (so it can run with a
            // different marker), but it is positioned TOGETHER with the outlines — one
            // common bbox for the auto-scale and the mirrors, nothing shifts apart
            let mut groups: Vec<Polys> = Vec::new();
            if let Some(inf) = tp.infill.as_ref() { if !inf.is_empty() { groups.push(inf.clone()); } }
            groups.push(tp.main.clone());
            let pos = position_split(groups, 1.0, &o2, pw, ph, &mut warn);
            let mut pi = 0usize;
            if pos.len() == 2 { parts.push(Part { id: PartId::Infill, feed: o2.feed, paths: pos[0].clone() }); pi = 1; }
            parts.push(Part { id: PartId::Text, feed: o2.feed, paths: pos[pi].clone() });
            pages = pgs; page = pgi; has_text = true;
        }
    }
    warn.dedup();
    if parts.is_empty() { warn.push("No drawable shapes found".into()); return Ok(Conv { gcode: String::new(), warnings: warn, paths: vec![], bbox: None, paper: [pw, ph], pages: None, page: None }); }
    // the infill sub-part (0.5.28) does not turn a text-only program into "SVG + Text"
    let has_svg = parts.iter().any(|p| p.id == PartId::Svg);
    let title = if has_text && has_svg { "SVG + Text" } else if has_text { "Text" } else { "SVG" };
    let mut c = emit_program(title, &parts, o, pw, ph, warn);
    if has_text { c.pages = Some(pages); c.page = Some(page); }
    Ok(c)
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
        // 0.5.19 convention: M3 S255 lifts the pen, M5 lowers it; the file must start with the
        // pen UP and lower it only before the first drawing move.
        let up = g.find("M3 S255").expect("pen-up M3 S255 present");
        let down = g.find("\nM5").expect("pen-down M5 present");
        assert!(up < down, "pen up (M3 S255) before the first pen down (M5)");
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
    fn travel_feed_is_a_separate_speed() {
        // 0.5.22: pen-up moves run at their own feed — travels carry F inline, the modal
        // drawing feed is re-armed after every pen down, the return home rides F too.
        let o = Opt { travel_feed: 6000.0, ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0]], vec![[20.0, 0.0], [30.0, 0.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G1 X0.000 Y0.000 F6000"), "first travel at the travel feed: {}", c.gcode);
        assert!(c.gcode.contains("G1 X20.000 Y0.000 F6000"), "travel between paths at the travel feed: {}", c.gcode);
        let feeds: Vec<&str> = c.gcode.lines().filter(|l| *l == "G1 F3000").collect();
        assert_eq!(feeds.len(), 3, "top feed line + one re-arm after each pen down: {}", c.gcode);
        assert!(c.gcode.contains("G1 X0 Y0 F6000"), "return home at the travel feed: {}", c.gcode);
        // drawing moves themselves must stay F-free (the re-armed modal feed drives them)
        for l in c.gcode.lines() {
            if (l.starts_with("G1 X") && !l.ends_with("F6000")) || l.starts_with("G2 ") || l.starts_with("G3 ") {
                assert!(!l.contains('F'), "drawing line must be F-free: {l}");
            }
        }
    }

    #[test]
    fn zero_travel_feed_keeps_the_old_output() {
        // 0 = backward compatible: no F on travel lines, no extra restore lines
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0]], vec![[20.0, 0.0], [30.0, 0.0]]], 1.0, &Opt::default(), vec![]);
        assert!(c.gcode.contains("G1 X20.000 Y0.000"), "travel rides the modal drawing feed");
        assert!(c.gcode.contains("G1 X0 Y0"), "plain return home");
        assert!(!c.gcode.contains("F6000"), "no travel feed anywhere: {}", c.gcode);
    }

    #[test]
    fn z_mode_travel_feed_switches_rapids_to_feed_moves() {
        // with a travel speed set even the Z mode travels as G1 F (an exact number on any
        // board) — only the vertical pen lifts stay G0 rapids
        let o = Opt { pen_mode: "z".into(), travel_feed: 6000.0, ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G1 X0.000 Y0.000 F6000"), "XY travel at the set speed in z mode: {}", c.gcode);
        assert!(!c.gcode.contains("G0 X"), "no G0 XY travels once the travel speed is set: {}", c.gcode);
        assert!(c.gcode.contains("G0 Z5"), "vertical pen lift stays a rapid: {}", c.gcode);
    }

    #[test]
    fn travel_feed_equal_to_draw_feed_is_free() {
        // setting the same number as the drawing feed must not add any extra lines
        let o = Opt { travel_feed: 3000.0, ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0]]], 1.0, &o, vec![]);
        assert!(!c.gcode.contains("F6000"));
        let feeds: Vec<&str> = c.gcode.lines().filter(|l| *l == "G1 F3000").collect();
        assert_eq!(feeds.len(), 1, "only the single top feed line: {}", c.gcode);
        assert!(c.gcode.contains("G1 X0 Y0"), "plain return home");
    }

    #[test]
    fn per_side_margins_shift_the_drawing() {
        // 0.5.22: left/top margins move the normalized drawing into the printable area
        let o = Opt { margin_l: 15.0, margin_t: 8.0, ..Default::default() };
        let c = polys_to_conv(vec![vec![[5.0, 5.0], [35.0, 15.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G1 X15.000 Y8.000"), "first travel at (ml, mt): {}", c.gcode);
        assert!(c.gcode.contains("G1 X45.000 Y18.000"), "second point shifted too: {}", c.gcode);
        assert!(!c.warnings.iter().any(|s| s.contains("exceeds")));
    }

    #[test]
    fn margins_center_between_the_sides() {
        // A4, left 20 right 40 -> printable 150 wide; a 100 mm line is centered inside it
        let o = Opt { margin_l: 20.0, margin_r: 40.0, center: true, ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [100.0, 0.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G1 X45.000 Y148.500"), "start at ml + (aw-w)/2 (center also fits Y): {}", c.gcode);
        assert!(c.gcode.contains("G1 X145.000 Y148.500"), "end leaves the same gap to the right margin");
    }

    #[test]
    fn right_bottom_margins_bound_autoscale() {
        // 200 mm wide drawing into 210-30=180 -> 90%; the right/bottom margins act as bounds
        let o = Opt { margin_r: 30.0, margin_b: 15.0, ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [200.0, 0.0], [200.0, 100.0]]], 1.0, &o, vec![]);
        assert!(c.warnings.iter().any(|s| s.contains("Auto-scaled")));
        assert!(c.gcode.contains("G1 X180.000 Y0.000"), "max X = sheet - right margin: {}", c.gcode);
    }

    #[test]
    fn legacy_margin_equals_four_equal_sides() {
        let poly = vec![vec![[0.0, 0.0], [50.0, 20.0]]];
        let a = polys_to_conv(poly.clone(), 1.0, &Opt { margin: 12.0, ..Default::default() }, vec![]);
        let b = polys_to_conv(poly, 1.0, &Opt { margin_l: 12.0, margin_r: 12.0, margin_t: 12.0, margin_b: 12.0, ..Default::default() }, vec![]);
        assert_eq!(a.gcode, b.gcode, "legacy single margin must behave like four equal sides");
        assert!(a.gcode.contains("G1 X12.000 Y12.000"));
    }

    #[test]
    fn margins_bigger_than_the_sheet_warn_and_clamp() {
        let o = Opt { margin_l: 300.0, margin_r: 300.0, ..Default::default() };
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 10.0]]], 1.0, &o, vec![]);
        assert!(c.warnings.iter().any(|s| s.contains("no room")), "warning present: {:?}", c.warnings);
        assert!(!c.gcode.is_empty(), "still emits G-code, no panic");
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
    fn invert_y_mirrors_the_drawing_in_place() {
        // 0.5.15: the sheet transform normalizes the drawing to the margin corner, then the
        // mirror flips it around its own bbox mid-Y: a diagonal laid as (0,0)->(30,10) must
        // travel to (0,10) and end at (30,0); the footprint stays identical
        let o = Opt::default();
        let base = polys_to_conv(vec![vec![[5.0, 5.0], [35.0, 15.0]]], 1.0, &o, vec![]);
        let mir = polys_to_conv(vec![vec![[5.0, 5.0], [35.0, 15.0]]], 1.0, &Opt { invert_y: true, ..Default::default() }, vec![]);
        assert!(base.gcode.contains("G1 X0.000 Y0.000") && base.gcode.contains("G1 X30.000 Y10.000"), "baseline diagonal: {}", base.gcode);
        assert!(mir.gcode.contains("G1 X0.000 Y10.000"), "travel flips to the mirrored end: {}", mir.gcode);
        assert!(mir.gcode.contains("G1 X30.000 Y0.000"), "line end flips too: {}", mir.gcode);
        assert!(!mir.gcode.contains("G1 X30.000 Y10.000"), "no unmirrored motion left: {}", mir.gcode);
        let (b1, b2) = (base.bbox.unwrap(), mir.bbox.unwrap());
        assert!((b1[1] - b2[1]).abs() < 0.01 && (b1[3] - b2[3]).abs() < 0.01, "footprint unchanged: {b1:?} vs {b2:?}");
    }

    #[test]
    fn invert_y_swaps_arc_direction() {
        // the 270° CCW arc from arcs_emit_g2_g3_with_ij must come out as a G2 with the
        // mirrored endpoint; I/J are recomputed from the mirrored center and stay radius-exact
        let o = Opt::default();
        let segs: SegPolys = vec![vec![Seg::A { s: [0.0, 0.0], e: [10.0, 10.0], c: [10.0, 0.0], cw: false }]];
        let base = segs_to_conv(segs, 1.0, &o, vec![]);
        assert!(base.gcode.contains("G3 X10.000 Y20.000"), "baseline is G3: {}", base.gcode);
        let segs: SegPolys = vec![vec![Seg::A { s: [0.0, 0.0], e: [10.0, 10.0], c: [10.0, 0.0], cw: false }]];
        let mir = segs_to_conv(segs, 1.0, &Opt { invert_y: true, ..Default::default() }, vec![]);
        assert!(mir.gcode.contains("G2 X10.000 Y0.000 I10.000 J0.000"), "mirrored arc is G2 with mirrored I/J: {}", mir.gcode);
        assert!(!mir.gcode.contains("G3 "), "no G3 left: {}", mir.gcode);
        let (b1, b2) = (base.bbox.unwrap(), mir.bbox.unwrap());
        assert!((b1[1] - b2[1]).abs() < 0.01 && (b1[3] - b2[3]).abs() < 0.01, "arc footprint unchanged: {b1:?} vs {b2:?}");
    }

    #[test]
    fn y_origin_bottom_moves_drawing_to_the_top_of_the_sheet() {
        // 0.5.18: a machine whose Y zero sits at the BOTTOM edge of the sheet (standard CNC).
        // The sheet mirror moves the whole drawing to the opposite edge: a diagonal drawn at
        // Y 0..10 on an A4 sheet must be emitted at Y 287..297 — the letterhead previewed at
        // the top of the sheet lands at the top of the real sheet
        let o = Opt { y_origin: "bottom".into(), ..Default::default() };
        let c = polys_to_conv(vec![vec![[5.0, 5.0], [35.0, 15.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G1 X0.000 Y297.000"), "travel to the sheet-top corner: {}", c.gcode);
        assert!(c.gcode.contains("G1 X30.000 Y287.000"), "line end flips to Y = height - y: {}", c.gcode);
        assert!(!c.gcode.contains("Y10.000") && !c.gcode.contains("Y0.000\n"), "no unflipped motion left: {}", c.gcode);
        let bb = c.bbox.unwrap();
        assert!((bb[1] - 287.0).abs() < 0.01 && (bb[3] - 297.0).abs() < 0.01, "bbox moved to the top edge: {bb:?}");
        assert!(c.warnings.is_empty(), "no paper warning: {:?}", c.warnings);
    }

    #[test]
    fn y_origin_reverse_negates_y_without_warning() {
        // feed machines whose Y+ scrolls the sheet backwards: Y = -Y, coordinates go negative,
        // and that must NOT raise the "exceeds paper" warning (the sheet top edge starts at the
        // machine zero and Y+ points away from the sheet)
        let o = Opt { y_origin: "reverse".into(), ..Default::default() };
        let c = polys_to_conv(vec![vec![[5.0, 5.0], [35.0, 15.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G1 X0.000 Y0.000"), "start corner sits on the zero: {}", c.gcode);
        assert!(c.gcode.contains("G1 X30.000 Y-10.000"), "line end flips to -y: {}", c.gcode);
        assert!(!c.gcode.contains("G1 X30.000 Y10.000"), "no unflipped motion left: {}", c.gcode);
        assert!(!c.warnings.iter().any(|s| s.contains("exceeds paper")), "negative Y is intentional: {:?}", c.warnings);
        // with the default "top" origin a negative Y can only come from a negative offset —
        // and there it IS a misplacement worth warning about
        let o2 = Opt { offset_y: -10.0, ..Default::default() };
        let c2 = polys_to_conv(vec![vec![[5.0, 5.0], [35.0, 15.0]]], 1.0, &o2, vec![]);
        assert!(c2.warnings.iter().any(|s| s.contains("exceeds paper")));
    }

    #[test]
    fn y_origin_bottom_swaps_arc_direction() {
        // the 270-degree CCW arc from arcs_emit_g2_g3_with_ij, mirrored around the A4 height:
        // G3 becomes G2, the endpoint moves to Y = 297 - y and I/J stay radius-exact
        let o = Opt { y_origin: "bottom".into(), ..Default::default() };
        let segs: SegPolys = vec![vec![Seg::A { s: [0.0, 0.0], e: [10.0, 10.0], c: [10.0, 0.0], cw: false }]];
        let c = segs_to_conv(segs, 1.0, &o, vec![]);
        // sheet space: s(0,10) e(10,20) c(10,10); bottom mirror: s(0,287) e(10,277) c(10,287)
        assert!(c.gcode.contains("G2 X10.000 Y277.000 I10.000 J0.000"), "{}", c.gcode);
        assert!(!c.gcode.contains("G3 "), "no G3 left: {}", c.gcode);
    }

    #[test]
    fn y_origin_bottom_respects_paper_setting() {
        // the flip anchor is the configured sheet height (custom paper), not a hard-coded A4
        let o = Opt { y_origin: "bottom".into(), paper: "custom".into(), paper_w: 100.0, paper_h: 150.0, ..Default::default() };
        let c = polys_to_conv(vec![vec![[5.0, 5.0], [35.0, 15.0]]], 1.0, &o, vec![]);
        assert!(c.gcode.contains("G1 X0.000 Y150.000"), "flip around the custom sheet height: {}", c.gcode);
        assert!(c.gcode.contains("G1 X30.000 Y140.000"), "{}", c.gcode);
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

#[cfg(test)]
mod combine_tests {
    //! 0.5.27: the combined SVG+Text program — one print from both blocks.
    use super::*;

    const RECT: &str = "<svg xmlns='http://www.w3.org/2000/svg' width='10mm' viewBox='0 0 10 10'><rect width='10' height='10' fill='none'/></svg>";

    #[test]
    fn single_part_matches_standalone() {
        // SVG-only and text-only via combine must byte-match the standalone converters —
        // combining one source is just that source (same header feed, same placement).
        let o = Opt::default();
        let c1 = combine_to_conv(RECT, "", &o, &Opt::default()).unwrap();
        let c2 = crate::svg::svg_to_conv(RECT, &o).unwrap();
        assert_eq!(c1.gcode, c2.gcode);
        let to = Opt { font: "DejaVu Sans".into(), size: 6.0, start_x: 50.0, start_y: 40.0, ..Default::default() };
        let t1 = combine_to_conv("", "Привет мир", &o, &to).unwrap();
        let t2 = crate::text::text_to_conv("Привет мир", &to).unwrap();
        // since 0.5.28 both sides title the program "Text" — no more SVG title on a text file
        assert_eq!(t1.gcode, t2.gcode, "text-only combine == text_to_conv");
        assert_eq!(t1.pages, t2.pages);
        assert_eq!(t1.page, t2.page);
    }

    #[test]
    fn two_parts_one_program() {
        let o = Opt::default();
        let to = Opt { font: "DejaVu Sans".into(), size: 6.0, start_x: 50.0, start_y: 40.0, ..Default::default() };
        let c = combine_to_conv(RECT, "Тест", &o, &to).unwrap();
        assert!(c.gcode.contains("; Plotter Controller — SVG + Text"), "header names both parts: {}", c.gcode);
        assert_eq!(c.gcode.matches("G21 G90").count(), 1, "one setup block");
        assert_eq!(c.gcode.matches("\nG1 X0 Y0").count(), 1, "one return home");
        assert!(c.gcode.trim_end().ends_with("G1 X0 Y0"), "home is the last move");
        // both parts drawn: the preview path count is the sum of the standalone parts
        let s = crate::svg::svg_to_conv(RECT, &o).unwrap();
        let t = crate::text::text_to_conv("Тест", &to).unwrap();
        assert_eq!(c.paths.len(), s.paths.len() + t.paths.len());
        // the text part sits at its Start X/Y: the union reaches far past the svg part (0..10 mm)
        let bb = c.bbox.unwrap();
        assert!(bb[2] > 55.0 && bb[3] > 40.0, "text drawn near Start (50, 40): {bb:?}");
        assert!(bb[0] < 1.0, "the svg part is still at the left edge: {bb:?}");
    }

    #[test]
    fn per_part_feed_switch() {
        // the letter-style multiplier applies to the text part only: a modal G1 F line
        // switches between the parts (single = 1.5 × 3000 = 4500), the svg part keeps 3000
        let o = Opt::default();
        let to = Opt { font: "DejaVu Sans".into(), size: 6.0, fill_mode: "single".into(), ..Default::default() };
        let c = combine_to_conv(RECT, "Аб", &o, &to).unwrap();
        let sw = c.gcode.find("\nG1 F4500").expect("feed switch to the text part feed");
        let first_down = c.gcode.find("\nM5\n").expect("the svg part draws first (pen down)");
        assert!(sw > first_down, "the switch must come after the svg part started drawing");
        let home = c.gcode.rfind("\nG1 X0 Y0").unwrap();
        assert!(sw < home, "the switch precedes the return home");
        // text-only via combine: no redundant switch, the header already carries the part feed
        let t = combine_to_conv("", "Аб", &o, &to).unwrap();
        assert_eq!(t.gcode.matches("\nG1 F4500").count(), 1, "header only: {}", t.gcode);
    }

    #[test]
    fn per_part_restore_after_pen_down() {
        // with a travel feed set, every pen-down re-arms THAT part's drawing feed:
        // svg paths restore F3000, text paths restore F4500 (single = 1.5×)
        let o = Opt { travel_feed: 6000.0, ..Default::default() };
        let to = Opt { font: "DejaVu Sans".into(), size: 6.0, fill_mode: "single".into(), ..Default::default() };
        let c = combine_to_conv(RECT, "Аб", &o, &to).unwrap();
        let ls: Vec<&str> = c.gcode.lines().collect();
        let mut saw_text = false; let mut saw_svg = false;
        for (i, l) in ls.iter().enumerate() {
            if *l == "M5" {
                match ls.get(i + 2) {
                    Some(&"G1 F3000") => saw_svg = true,
                    Some(&"G1 F4500") => saw_text = true,
                    other => panic!("bad re-arm after pen down: {other:?}: {}", c.gcode),
                }
            }
        }
        assert!(saw_svg && saw_text, "both parts restore their own feed: {}", c.gcode);
    }

    #[test]
    fn nothing_to_draw() {
        let c = combine_to_conv("", "", &Opt::default(), &Opt::default()).unwrap();
        assert!(c.gcode.is_empty());
        assert!(c.warnings.iter().any(|w| w.contains("No drawable shapes")));
        assert_eq!(c.bbox, None);
    }

    #[test]
    fn text_only_with_infill_keeps_the_text_title() {
        // the infill sub-part must not make a text-only program "SVG + Text"
        let to = Opt { font: "DejaVu Sans".into(), size: 6.0, fill_mode: "hatch".into(), start_x: 50.0, start_y: 40.0, ..Default::default() };
        let c = combine_to_conv("", "Аб", &Opt::default(), &to).unwrap();
        assert!(c.gcode.contains("— Text"), "title stays Text: {}", c.gcode.lines().next().unwrap());
        assert!(c.gcode.contains("; [PART infill]") && c.gcode.contains("; [PART text]"));
        assert!(!c.gcode.contains("; [PART svg]"));
    }

    #[test]
    fn combine_single_mode_svg_part() {
        // 0.5.30: svgMode=single rides the shared options — the SVG part becomes skeleton
        // lines (no arcs), the text part keeps its own conversion and the part structure holds
        let o = Opt { svg_mode: "single".into(), ..Default::default() };
        let to = Opt { font: "DejaVu Sans".into(), size: 6.0, start_x: 50.0, start_y: 40.0, ..Default::default() };
        let c = combine_to_conv(RECT, "Аб", &o, &to).unwrap();
        assert!(c.gcode.contains("— SVG + Text"), "title unchanged: {}", c.gcode.lines().next().unwrap());
        assert!(c.gcode.contains("; [PART svg]") && c.gcode.contains("; [PART text]"), "{}", c.gcode);
        assert!(!c.gcode.contains("G2 ") && !c.gcode.contains("G3 "), "skeleton svg part draws lines only: {}", c.gcode);
        assert!(!c.warnings.iter().any(|w| w.contains("No drawable")), "rect drawn: {:?}", c.warnings);
    }

    #[test]
    fn combine_text_hatch_style_rides_text_options() {
        // 0.5.31: hatchStyle must reach the text part of the combined program — the dots
        // pattern (one pen-down per dot) lifts the pen far more often than line hatching
        let mk = |hs: &str| {
            let to = Opt { font: "DejaVu Sans".into(), size: 12.0, fill_mode: "hatch".into(), fill_spacing: 0.6, hatch_style: hs.into(), start_x: 50.0, start_y: 40.0, ..Default::default() };
            combine_to_conv(RECT, "ЯЯ", &Opt::default(), &to).unwrap()
        };
        let lines = mk("lines"); let dots = mk("dots");
        assert!(lines.gcode.contains("; [PART infill]") && lines.gcode.contains("; [PART text]"));
        assert_ne!(lines.gcode, dots.gcode, "hatchStyle must change the text infill part");
        let lift = |s: &str| s.matches("M3 S255").count();
        assert!(lift(&dots.gcode) > lift(&lines.gcode), "dots lift per dot: {}", dots.gcode);
    }
}

#[cfg(test)]
mod parts_tests {
    //! 0.5.28: the `; [PART <id>]` structure comments in the emitted programs.
    use super::*;

    #[test]
    fn svg_program_marks_its_part() {
        let c = polys_to_conv(vec![vec![[0.0, 0.0], [10.0, 0.0]]], 1.0, &Opt::default(), vec![]);
        assert!(c.gcode.contains("; [PART svg]"), "{}", c.gcode);
        // the comment sits before the part's first travel
        let ci = c.gcode.find("; [PART svg]").unwrap();
        let tr = c.gcode.find("G1 X0.000 Y0.000").unwrap();
        assert!(ci < tr, "part comment precedes the first travel");
    }

    #[test]
    fn combined_program_marks_both_parts() {
        const RECT: &str = "<svg xmlns='http://www.w3.org/2000/svg' width='10mm' viewBox='0 0 10 10'><rect width='10' height='10' fill='none'/></svg>";
        let to = Opt { font: "DejaVu Sans".into(), size: 6.0, start_x: 50.0, start_y: 40.0, ..Default::default() };
        let c = combine_to_conv(RECT, "Аб", &Opt::default(), &to).unwrap();
        let si = c.gcode.find("; [PART svg]").expect("svg part");
        let ti = c.gcode.find("; [PART text]").expect("text part");
        assert!(si < ti, "svg draws first");
        assert!(!c.gcode.contains("; [PART infill]"), "pen-style text has no infill part: {}", c.gcode);
    }

    #[test]
    fn position_split_keeps_groups_aligned_under_mirror() {
        // the point of position_split: with invert_y the mirror runs around the COMMON bbox,
        // so a small group keeps its place relative to the big one (a separately positioned
        // small group would mirror around its own mid-Y and shift apart)
        let big = vec![vec![[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [0.0, 50.0], [0.0, 0.0]]];
        let small = vec![vec![[45.0, 20.0], [55.0, 20.0]]];
        let o = Opt { invert_y: true, ..Default::default() };
        // baseline: one part with everything
        let mut all = big.clone(); all.extend(small.clone());
        let single = polys_to_conv(all, 1.0, &o, vec![]);
        let mut w = vec![];
        let split = position_split(vec![big, small], 1.0, &o, 210.0, 297.0, &mut w);
        let flat: Vec<Vec<[f64; 2]>> = split.into_iter().flatten().map(|p| p.iter().map(|s| match s { Seg::L(v) => *v, Seg::A { e, .. } => *e }).collect()).collect();
        assert_eq!(flat.len(), single.paths.len());
        for (a, b) in flat.iter().zip(single.paths.iter()) {
            for (pa, pb) in a.iter().zip(b.iter()) {
                assert!((pa[0] - pb[0]).abs() < 1e-9 && (pa[1] - pb[1]).abs() < 1e-9, "{pa:?} vs {pb:?}");
            }
        }
    }
}

#[cfg(test)]
mod markers_tests {
    //! 0.5.28: apply_marker_colors — M0 pauses at part boundaries where the marker changes.
    use super::*;

    fn prog() -> String {
        // svg part + text part, the shape emit_program produces (simplified but faithful:
        // pen-up between parts, part comments before the travels)
        ["; Plotter Controller — SVG + Text", "G21 G90", "G1 F3000", "M3 S255", "; [PART svg]",
         "G1 X0.000 Y0.000", "M5", "G4 P0.15", "M3 S255", "; [PART text]", "G1 X50.000 Y40.000",
         "M5", "M3 S255", "G1 X0 Y0"].join("\n")
    }

    fn colors(list: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn pause_inserted_before_the_part_with_new_marker() {
        let (g, n) = apply_marker_colors(&prog(), &colors(&[("svg", "чёрный"), ("text", "красный")]), "M3 S255", 0.15);
        assert_eq!(n, 1, "{g}");
        let ti = g.find("; [PART text]").unwrap();
        let block = &g[..ti];
        assert!(block.contains("; [MARKER: красный]"), "marker comment before the part: {g}");
        assert!(block.trim_end().ends_with("M0"), "M0 right before the part comment: {g}");
        // the block: comment, pen up, dwell, M0 — in this order
        let bi = g.find("; [MARKER: красный]").unwrap();
        let ls: Vec<&str> = g[bi..ti].lines().collect();
        assert_eq!(ls, vec!["; [MARKER: красный] swap the marker, then press Resume", "M3 S255", "G4 P0.15", "M0"], "{ls:?}");
        assert!(g.contains("; [PART text]"), "the part comment survives");
        assert_eq!(g.matches("M0").count(), 1);
    }

    #[test]
    fn same_or_empty_colors_never_pause() {
        let (_, n) = apply_marker_colors(&prog(), &colors(&[("svg", "чёрный"), ("text", "Чёрный")]), "M3 S255", 0.15);
        assert_eq!(n, 0, "case-insensitive equality: no pause");
        let (_, n) = apply_marker_colors(&prog(), &colors(&[("svg", ""), ("text", "")]), "M3 S255", 0.15);
        assert_eq!(n, 0);
        // empty next = keep current: no pause even from a named first part
        let (g, n) = apply_marker_colors(&prog(), &colors(&[("svg", "красный")]), "M3 S255", 0.15);
        assert_eq!(n, 0);
        assert!(!g.contains("M0"), "no M0 anywhere: {g}");
        // named next after empty first: a pause IS needed (start with what is installed)
        let (_, n) = apply_marker_colors(&prog(), &colors(&[("text", "красный")]), "M3 S255", 0.15);
        assert_eq!(n, 1);
    }

    #[test]
    fn infill_boundary_gets_its_own_pause() {
        // parts in order: svg -> infill -> text; colors svg="", infill="красный", text="чёрный"
        let mut p = prog();
        p = p.replace("; [PART text]", "; [PART infill]\nG1 X9.000 Y9.000\nM5\nM3 S255\n; [PART text]");
        let (g, n) = apply_marker_colors(&p, &colors(&[("infill", "красный"), ("text", "чёрный")]), "M3 S255", 0.15);
        assert_eq!(n, 2, "pause before infill and before the outline: {g}");
        let i1 = g.find("; [MARKER: красный]").unwrap();
        let i2 = g.find("; [MARKER: чёрный]").unwrap();
        assert!(i1 < g.find("; [PART infill]").unwrap() && i2 < g.find("; [PART text]").unwrap());
    }

    #[test]
    fn reapply_is_idempotent() {
        let c1 = colors(&[("svg", "чёрный"), ("text", "красный")]);
        let (g1, n1) = apply_marker_colors(&prog(), &c1, "M3 S255", 0.15);
        assert_eq!(n1, 1);
        let (g2, n2) = apply_marker_colors(&g1, &c1, "M3 S255", 0.15);
        assert_eq!((g2, n2), (g1.clone(), n1), "second apply must not duplicate the block");
        // and changing the palette rewrites the pauses in place
        let (g3, n3) = apply_marker_colors(&g1, &colors(&[("svg", "чёрный"), ("text", "")]), "M3 S255", 0.15);
        assert_eq!(n3, 0);
        assert!(!g3.contains("[MARKER:"), "old block removed: {g3}");
        assert!(g3.contains("; [PART text]"), "program structure intact");
    }

    #[test]
    fn no_parts_or_single_part_untouched() {
        let plain = "G21 G90\nG1 X1 Y1\nM5\nM0\nG1 X2 Y2";
        let (g, n) = apply_marker_colors(plain, &colors(&[("svg", "чёрный")]), "M3 S255", 0.15);
        assert_eq!((g, n), (plain.to_string(), 0), "no [PART] comments: unchanged (the stray M0 stays)");
        let single = "G21 G90\n; [PART svg]\nG1 X1 Y1\n; [PART svg]\nG1 X2 Y2";
        let (g, n) = apply_marker_colors(single, &colors(&[("svg", "чёрный"), ("text", "красный")]), "M3 S255", 0.15);
        assert_eq!(n, 0, "same-id parts share the marker: no pause between them");
        assert_eq!(g, single);
    }

    #[test]
    fn zero_dwell_and_empty_penup_are_fine() {
        let (g, n) = apply_marker_colors(&prog(), &colors(&[("text", "красный")]), "", 0.0);
        assert_eq!(n, 1);
        let bi = g.find("; [MARKER: красный]").unwrap();
        let block: Vec<&str> = g[bi..].lines().take(2).collect();
        assert_eq!(block, vec!["; [MARKER: красный] swap the marker, then press Resume", "M0"], "no penup, no dwell line: {block:?}");
    }

    // -- pen lift invariant: a generated program must open on a pen-up, alternate up/down
    // (repeated ups — e.g. a marker block re-lifting right after the part's own lift — are
    // harmless and collapse) and close on an up. A missed lift means the servo draws a stray
    // line across the sheet or parks the pen down mid-program. Every generator (svg standard
    // and single, text with every letter style, combine, pdf) must keep it in every pen mode.
    fn assert_pen_alternation(gcode: &str, up: &str, down: &str, what: &str) {
        let seq: Vec<bool> = gcode.lines().filter_map(|l| { // true = up
            let l = l.trim();
            if l == up { Some(true) } else if l == down { Some(false) } else { None }
        }).collect();
        assert!(seq.first().copied().unwrap_or(false), "{what}: the file must open on a pen-up ({up}):\n{gcode}");
        let mut c: Vec<bool> = vec![];
        for &u in &seq { if !(u && c.last() == Some(&true)) { c.push(u); } } // collapse consecutive ups
        assert!(c.last().copied().unwrap_or(false), "{what}: must close on a pen-up:\n{gcode}");
        for w in c.windows(2) { assert_ne!(w[0], w[1], "{what}: ups/downs must alternate: {seq:?}"); }
        let (nu, nd) = (c.iter().filter(|&&u| u).count(), c.len() - c.iter().filter(|&&u| u).count());
        assert_eq!(nu, nd + 1, "{what}: one more up than downs");
    }

    #[test]
    fn pen_lift_alternates_across_every_generator() {
        const RECT: &str = "<svg xmlns='http://www.w3.org/2000/svg' width='10mm' viewBox='0 0 10 10'><rect width='10' height='10' fill='none'/><line x1='0' y1='10' x2='10' y2='0' stroke='black'/></svg>";
        let font = |fm: &str| Opt { font: "DejaVu Sans".into(), size: 6.0, fill_mode: fm.into(), ..Default::default() };
        // svg standard: defaults, dwell off, travel feed, the flipped servo convention, z mode
        assert_pen_alternation(&crate::svg::svg_to_conv(RECT, &Opt::default()).unwrap().gcode, "M3 S255", "M5", "svg default");
        assert_pen_alternation(&crate::svg::svg_to_conv(RECT, &Opt { dwell: 0.0, ..Default::default() }).unwrap().gcode, "M3 S255", "M5", "svg no dwell");
        assert_pen_alternation(&crate::svg::svg_to_conv(RECT, &Opt { travel_feed: 6000.0, ..Default::default() }).unwrap().gcode, "M3 S255", "M5", "svg travel feed");
        assert_pen_alternation(&crate::svg::svg_to_conv(RECT, &Opt { servo_up: "M5".into(), servo_down: "M3 S45".into(), ..Default::default() }).unwrap().gcode, "M5", "M3 S45", "svg flipped convention");
        assert_pen_alternation(&crate::svg::svg_to_conv(RECT, &Opt { pen_mode: "z".into(), ..Default::default() }).unwrap().gcode, "G0 Z5", "G1 Z0 F500", "svg z mode");
        assert_pen_alternation(&crate::svg::svg_to_conv(RECT, &Opt { pen_mode: "z".into(), travel_feed: 6000.0, ..Default::default() }).unwrap().gcode, "G0 Z5", "G1 Z0 F500", "svg z travel feed");
        assert_pen_alternation(&crate::svg::svg_to_conv(RECT, &Opt { svg_mode: "single".into(), ..Default::default() }).unwrap().gcode, "M3 S255", "M5", "svg single");
        // text: every letter style (fill/hatch with all pattern variants, justify alignment)
        for fm in ["pen", "fill", "hatch"] {
            assert_pen_alternation(&crate::text::text_to_conv("Аб Тест", &font(fm)).unwrap().gcode, "M3 S255", "M5", &format!("text {fm}"));
        }
        for hs in ["lines", "dots", "triangle", "lightning"] {
            assert_pen_alternation(&crate::text::text_to_conv("Аб", &Opt { hatch_style: hs.into(), ..font("hatch") }).unwrap().gcode, "M3 S255", "M5", &format!("text hatch {hs}"));
        }
        assert_pen_alternation(&crate::text::text_to_conv("Слово другое ещё и ещё последнее", &Opt { align: "justify".into(), ..font("hatch") }).unwrap().gcode, "M3 S255", "M5", "text justify");
        // combine svg + text (three parts incl. the infill), with and without a travel feed
        let to = font("hatch");
        assert_pen_alternation(&combine_to_conv(RECT, "Аб Тест", &Opt::default(), &to).unwrap().gcode, "M3 S255", "M5", "combine");
        assert_pen_alternation(&combine_to_conv(RECT, "Аб Тест", &Opt { travel_feed: 6000.0, ..Default::default() }, &to).unwrap().gcode, "M3 S255", "M5", "combine travel feed");
        // combine + marker pauses on top: the inserted block lifts again — still alternates
        let c = combine_to_conv(RECT, "Аб", &Opt::default(), &to).unwrap();
        let cols = std::collections::BTreeMap::from([("svg".to_string(), "black".to_string()), ("infill".to_string(), "red".to_string()), ("text".to_string(), "blue".to_string())]);
        let (mg, n) = apply_marker_colors(&c.gcode, &cols, "M3 S255", 0.15);
        assert!(n >= 2, "markers must add pauses between the three parts: {}", c.gcode);
        assert_pen_alternation(&mg, "M3 S255", "M5", "combine + markers");
    }
}
