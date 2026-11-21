//! SVG -> polyline/arc segments. Curves (C/Q/A, circles) are flattened adaptively
//! (error-bounded, not the old fixed 16 chords) and then re-fitted into true circular
//! arcs G2/G3 — the structure program №1 uses, which cuts the line count by an order
//! of magnitude and lets the GRBL planner chain curves without micro-braking.
use crate::gcode::{emit_program, paper, position_split, seg_end, segpath_points, segs_to_conv, Conv, Opt, Part, PartId, Polys, Seg, SegPath, SegPolys};
use crate::skeleton::{bitmap_strokes, raster_rule, stamp_band};
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

/// mm per source unit: the width attribute (mm/cm/in/pt/px) measured against the viewBox,
/// or the 96-dpi default. Shared by the standard and single-line (0.5.30) converters.
fn svg_unit(root: &roxmltree::Node) -> f64 {
    let vb: Vec<f64> = root.attribute("viewBox").map(|v| v.split(|c: char| c.is_whitespace() || c == ',').filter_map(|s| s.parse().ok()).collect()).unwrap_or_default();
    let mut unit = 25.4 / 96.0;
    if let Some(w) = root.attribute("width") {
        let t = w.trim(); let k = t.find(|c: char| c.is_alphabetic() || c == '%').unwrap_or(t.len());
        if let (Ok(val), Some(um)) = (t[..k].parse::<f64>(), match &t[k..] { "mm" => Some(1.0), "cm" => Some(10.0), "in" => Some(25.4), "pt" => Some(25.4 / 72.0), "px" | "" => Some(25.4 / 96.0), _ => None }) {
            unit = if vb.len() == 4 && vb[2] != 0.0 { val * um / vb[2] } else { um };
        }
    }
    unit
}

/// 0.5.27: parse + flatten only — segments in SOURCE units plus the mm-per-unit factor (the
/// fitting tolerance follows the scale). Shared by svg_to_conv and the combined SVG+Text program.
pub fn svg_to_segs(svg: &str, o: &Opt, warn: &mut Vec<String>) -> Result<(SegPolys, f64), String> {
    let doc = roxmltree::Document::parse_with_options(svg, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).map_err(|e| format!("Invalid SVG: {e}"))?;
    let root = doc.root_element();
    let unit = svg_unit(&root);
    // fitting tolerance in source units: FIT_TOL_MM on the sheet (auto-fit can only shrink the
    // scale afterwards, which only makes the deviation smaller than the budget)
    let mm_per_unit = (unit * o.scale / 100.0).abs();
    let tol = if mm_per_unit > 1e-9 { (FIT_TOL_MM / mm_per_unit).max(1e-9) } else { 1.0 };
    let mut warn_d: Vec<String> = vec![]; let mut segs: SegPolys = vec![];
    for nd in doc.descendants().filter(|n| n.is_element()) {
        if let Some(d) = shape_d(&nd) {
            if nd.attribute("transform").is_some() { warn_d.push("transform attribute ignored".into()); }
            segs.extend(flatten(&d, tol, warn_d.as_mut()));
        }
    }
    warn_d.dedup();
    warn.extend(warn_d);
    Ok((segs, unit))
}

pub fn svg_to_conv(svg: &str, o: &Opt) -> Result<Conv, String> {
    if o.svg_mode == "single" { return svg_single_conv(svg, o); }
    let mut warn = vec![];
    let (segs, unit) = svg_to_segs(svg, o, &mut warn)?;
    Ok(segs_to_conv(segs, unit, o, warn))
}

/// One shape for the single-line mode: point chains positioned on the sheet (mm), whether the
/// shape is explicitly filled, its fill rule, the stroke-band width in mm (0 = no band) and
/// the ink thickness in mm that drives the bitmap resolution.
struct Sh { cs: Polys, fill: bool, eo: bool, band: f64, thick: f64 }

/// 0.5.30 single-line mode: fills and thick strokes become ONE centerline. Shapes are
/// rasterized on the sheet (explicitly filled ones by their fill rule, unfilled ones as a
/// band of their stroke width — SVG default 1 unit), the bitmap is thinned to a skeleton and
/// traced (skeleton.rs — the same machinery as the text «Ручка» style and PDF pages), and the
/// strokes go out as line-only G-code. Line art keeps its geometry (the band thins back to
/// the centerline); filled bold fonts and logos collapse to single pen strokes. Returns
/// positioned strokes in sheet mm — shared by svg_to_conv and the combined SVG+Text program.
pub fn svg_single_paths(svg: &str, o: &Opt, pw: f64, ph: f64, warn: &mut Vec<String>) -> Result<SegPolys, String> {
    let doc = roxmltree::Document::parse_with_options(svg, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).map_err(|e| format!("Invalid SVG: {e}"))?;
    let root = doc.root_element();
    let unit = svg_unit(&root);
    let mpu = (unit * o.scale / 100.0).abs(); // mm per source unit after the scale
    let tol = if mpu > 1e-9 { (FIT_TOL_MM / mpu).max(1e-9) } else { 1.0 };
    let mut warn_d: Vec<String> = vec![];
    let mut src: Vec<(SegPolys, bool, bool, f64)> = vec![]; // subpaths, filled, evenodd, band width (units)
    for nd in doc.descendants().filter(|n| n.is_element()) {
        let Some(d) = shape_d(&nd) else { continue };
        if nd.attribute("transform").is_some() { warn_d.push("transform attribute ignored".into()); }
        let paths = flatten(&d, tol, warn_d.as_mut());
        if paths.is_empty() { continue; }
        // SVG renders shapes black-filled by default, but line art for plotters is authored
        // with explicit fills — an IMPLICIT fill is treated as a stroke line (a bare rect
        // stays its outline instead of collapsing into a plus-shaped skeleton).
        let filled = nd.attribute("fill").map(|f| !f.trim().eq_ignore_ascii_case("none")).unwrap_or(false);
        let eo = nd.attribute("fill-rule").map(|f| f.trim().eq_ignore_ascii_case("evenodd")).unwrap_or(false);
        let stroked = nd.attribute("stroke").map(|s| !s.trim().eq_ignore_ascii_case("none")).unwrap_or(false);
        let sw = nd.attribute("stroke-width").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(1.0);
        let band = if stroked { sw } else if !filled { 1.0 } else { 0.0 };
        src.push((paths, filled, eo, band));
    }
    warn_d.dedup();
    warn.extend(warn_d);
    if src.is_empty() { return Ok(vec![]); }
    // one common transform (auto-fit, margins, center, invert-Y, Y-origin), split back per shape
    let groups: Vec<Polys> = src.iter().map(|(paths, ..)| paths.iter().map(segpath_points).collect()).collect();
    let pos = position_split(groups, unit, o, pw, ph, warn);
    let mut shs: Vec<Sh> = vec![];
    let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (i, (_, filled, eo, band)) in src.iter().enumerate() {
        let cs: Polys = pos[i].iter().map(|p| p.iter().map(seg_end).collect()).collect();
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for c in &cs { for v in c { b = [b[0].min(v[0]), b[1].min(v[1]), b[2].max(v[0]), b[3].max(v[1])]; } }
        if b[0] > b[2] { continue; } // nothing positioned (degenerate subpaths)
        bb = [bb[0].min(b[0]), bb[1].min(b[1]), bb[2].max(b[2]), bb[3].max(b[3])];
        // ink thickness drives the bitmap resolution: a filled shape is as thick as its
        // smaller side, a band as its width
        let thick = if *filled { ((b[2] - b[0]).min(b[3] - b[1])).max(band * mpu) } else { band * mpu };
        shs.push(Sh { cs, fill: *filled, eo: *eo, band: band * mpu, thick });
    }
    if shs.is_empty() { return Ok(vec![]); }
    // resolution: ~12 px/mm at best, never more than ~300 px across the thickest ink (thinning
    // costs O(width) passes over the whole bitmap; 300 px stays well under a second) and never
    // a bitmap wider than ~2400 px
    let thick = shs.iter().map(|s| s.thick).fold(0.0f64, f64::max).max(0.5);
    let dim = (bb[2] - bb[0]).max(bb[3] - bb[1]).max(1.0);
    let mut res = (300.0 / thick).clamp(2.0, 12.0).min(2400.0 / dim).max(1.0);
    // pad: 1 mm plus half the widest stroke band — the stamped round caps must never touch
    // the bitmap border (thinning never deletes border pixels, so border-touching caps would
    // leave arc-shaped skeleton spurs and fork the trace)
    let pad = 1.0 + shs.iter().filter(|s| s.band > 0.0).map(|s| s.band / 2.0).fold(0.0f64, f64::max);
    let org = [bb[0] - pad, bb[1] - pad];
    let mut w = (((bb[2] - bb[0]) + 2.0 * pad) * res).ceil() as usize;
    let mut h = (((bb[3] - bb[1]) + 2.0 * pad) * res).ceil() as usize;
    if w * h > 10_000_000 { // hard memory/time cap: shrink the resolution and re-size once
        res *= (10_000_000.0 / (w as f64 * h as f64)).sqrt();
        w = (((bb[2] - bb[0]) + 2.0 * pad) * res).ceil() as usize;
        h = (((bb[3] - bb[1]) + 2.0 * pad) * res).ceil() as usize;
    }
    let (w, h) = (w.max(2), h.max(2));
    let mut g = vec![0u8; w * h];
    for s in &shs {
        let px = |v: &[f64; 2]| [(v[0] - org[0]) * res, (v[1] - org[1]) * res];
        if s.fill {
            let polys: Vec<Vec<[f64; 2]>> = s.cs.iter().map(|c| c.iter().map(px).collect()).collect();
            for (i, ink) in raster_rule(&polys, w, h, s.eo).into_iter().enumerate() { if ink == 1 { g[i] = 1; } }
        }
        if s.band > 0.0 {
            let wpx = (s.band * res).max(2.0); // at least ~2 px so thinning keeps the line
            for c in &s.cs { let cp: Vec<[f64; 2]> = c.iter().map(px).collect(); stamp_band(&mut g, w, h, &cp, wpx); }
        }
    }
    let mut strokes = bitmap_strokes(&mut g, w, h, 1.0, 3);
    drop_shadow_strokes(&mut strokes, 2.2);
    let k = 1.0 / res;
    let mut out: SegPolys = vec![];
    for s in strokes {
        let pts: Vec<[f64; 2]> = s.iter().map(|q| [org[0] + q[0] * k, org[1] + q[1] * k]).collect();
        let len: f64 = pts.windows(2).map(|wn| d2(wn[0], wn[1])).sum();
        if len >= 0.3 { out.push(pts.into_iter().map(Seg::L).collect()); } // specks below 0.3 mm die
    }
    Ok(out)
}

/// Drop skeleton fragments that hug a longer stroke within `eps` px along their whole length
/// and are far shorter than it: thinning a rasterized fill occasionally sheds a short
/// parallel strand next to the main line (a boundary kink bypassed by the trace) — it
/// duplicates ink the long stroke already covers, so plotting it would just double the pass.
fn drop_shadow_strokes(strokes: &mut Vec<Vec<[f64; 2]>>, eps: f64) {
    if strokes.len() < 2 { return; }
    let lens: Vec<f64> = strokes.iter().map(|s| s.windows(2).map(|w| d2(w[0], w[1])).sum()).collect();
    let mut ord: Vec<usize> = (0..strokes.len()).collect();
    ord.sort_by(|&a, &b| lens[b].partial_cmp(&lens[a]).unwrap_or(std::cmp::Ordering::Equal));
    let mut drop = vec![false; strokes.len()];
    let mut kept: Vec<usize> = vec![];
    for &i in &ord {
        let shadow = kept.iter().any(|&j| {
            lens[i] < 0.25 * lens[j]
                && strokes[i].iter().all(|p| strokes[j].windows(2).any(|w| dist_seg(*p, w[0], w[1]) <= eps))
        });
        if shadow { drop[i] = true; } else { kept.push(i); }
    }
    let mut k = 0;
    strokes.retain(|_| { k += 1; !drop[k - 1] });
}

pub fn svg_single_conv(svg: &str, o: &Opt) -> Result<Conv, String> {
    let mut warn = vec![];
    let (pw, ph) = paper(o, &mut warn);
    let paths = svg_single_paths(svg, o, pw, ph, &mut warn)?;
    if paths.is_empty() {
        warn.push("No drawable shapes found".into());
        return Ok(Conv { gcode: String::new(), warnings: warn, paths: vec![], bbox: None, paper: [pw, ph], pages: None, page: None });
    }
    Ok(emit_program("SVG", &[Part { id: PartId::Svg, feed: o.feed, paths }], o, pw, ph, warn))
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

    // 0.5.30: single-line mode — fills and thick strokes become one centerline

    #[test]
    fn default_mode_is_standard() {
        assert_eq!(Opt::default().svg_mode, "standard");
    }

    #[test]
    fn single_mode_donut_draws_one_centerline() {
        // filled ring (outer r=20, inner r=10, evenodd): the skeleton must be one closed
        // stroke near the mid radius 15, far inside the standard outline footprint
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='50mm' viewBox='0 0 50 50'><path fill='#000' fill-rule='evenodd' d='M5 25A20 20 0 1 0 45 25A20 20 0 1 0 5 25ZM15 25A10 10 0 1 1 35 25A10 10 0 1 1 15 25Z'/></svg>";
        let o = Opt { svg_mode: "single".into(), ..Default::default() };
        let c = svg_to_conv(svg, &o).unwrap();
        assert!(!c.gcode.is_empty());
        assert!(!c.gcode.contains("G2 ") && !c.gcode.contains("G3 "), "skeleton emits lines only: {}", c.gcode);
        assert_eq!(c.gcode.matches("\nM5\n").count(), 1, "the ring collapses into ONE stroke: {}", c.gcode);
        // the ring is normalized to the sheet corner: center (20,20), mid radius 15
        let (mut rmin, mut rmax) = (1e9f64, 0.0f64);
        for p in &c.paths { for v in p { let r = (v[0] - 20.0).hypot(v[1] - 20.0); rmin = rmin.min(r); rmax = rmax.max(r); } }
        assert!((rmin - 15.0).abs() < 3.0 && (rmax - 15.0).abs() < 3.0, "skeleton sits on the mid radius 15: [{rmin:.2}, {rmax:.2}]");
        // the standard mode still draws the full outline (40x40 mm bbox)
        let s = svg_to_conv(svg, &Opt::default()).unwrap();
        let bb = s.bbox.unwrap();
        assert!((bb[2] - bb[0] - 40.0).abs() < 0.1, "standard mode keeps the outline: {bb:?}");
    }

    #[test]
    fn single_mode_open_stroke_stays_the_centerline() {
        // an unfilled line keeps its geometry: one pen-down stroke along the same diagonal
        // (not a doubled outline), every point on the line y = x/3 (tip erosion is bounded)
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='40mm' viewBox='0 0 40 20'><path d='M5 5 L35 15' fill='none' stroke='#000' stroke-width='2'/></svg>";
        let o = Opt { svg_mode: "single".into(), ..Default::default() };
        let c = svg_to_conv(svg, &o).unwrap();
        assert_eq!(c.gcode.matches("\nM5\n").count(), 1, "one stroke, not a doubled outline: {}", c.gcode);
        let mut xmax = 0.0f64;
        for p in &c.paths { for v in p {
            assert!((v[1] - v[0] / 3.0).abs() <= 0.8, "off the centerline: {v:?}");
            xmax = xmax.max(v[0]);
        } }
        assert!(xmax > 27.0, "the line reaches its far end: xmax={xmax}");
    }

    #[test]
    fn single_mode_bare_rect_keeps_its_outline() {
        // SVG renders shapes black-filled by default, but a BARE rect (no fill attribute) is
        // line-art intent: it must stay its outline, not collapse into a plus-shaped skeleton
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='50mm' viewBox='0 0 50 50'><rect x='5' y='5' width='40' height='30'/></svg>";
        let o = Opt { svg_mode: "single".into(), ..Default::default() };
        let c = svg_to_conv(svg, &o).unwrap();
        let bb = c.bbox.unwrap();
        assert!((bb[2] - bb[0] - 40.0).abs() < 0.6 && (bb[3] - bb[1] - 30.0).abs() < 0.6, "outline kept: {bb:?}");
    }

    #[test]
    fn single_mode_no_shapes_warns() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='40mm' viewBox='0 0 40 20'></svg>";
        let o = Opt { svg_mode: "single".into(), ..Default::default() };
        let c = svg_to_conv(svg, &o).unwrap();
        assert!(c.gcode.is_empty());
        assert!(c.warnings.iter().any(|w| w.contains("No drawable shapes")));
        assert_eq!(c.bbox, None);
    }
}
