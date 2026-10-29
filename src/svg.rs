//! SVG -> polylines. Arcs, H/V, S/T are handled by svgtypes' simplifying parser.
use crate::gcode::{polys_to_conv, Conv, Opt, Polys};
use svgtypes::{SimplePathSegment as S, SimplifyingPathParser};

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

pub fn flatten(d: &str, warn: &mut Vec<String>) -> Polys {
    let (mut polys, mut cur): (Polys, Vec<[f64; 2]>) = (vec![], vec![]);
    let (mut x, mut y, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    let fin = |cur: &mut Vec<[f64; 2]>, polys: &mut Polys| { if cur.len() > 1 { polys.push(std::mem::take(cur)); } else { cur.clear(); } };
    for seg in SimplifyingPathParser::from(d) {
        let seg = match seg { Ok(s) => s, Err(_) => { warn.push("Bad path data skipped".into()); break; } };
        match seg {
            S::MoveTo { x: a, y: b } => { fin(&mut cur, &mut polys); cur.push([a, b]); x = a; y = b; sx = a; sy = b; }
            S::LineTo { x: a, y: b } => { if cur.is_empty() { cur.push([x, y]); } cur.push([a, b]); x = a; y = b; }
            S::Quadratic { x1, y1, x: a, y: b } => {
                if cur.is_empty() { cur.push([x, y]); }
                for i in 1..=16 { let (t, u) = (i as f64 / 16.0, 1.0 - i as f64 / 16.0);
                    cur.push([u * u * x + 2.0 * u * t * x1 + t * t * a, u * u * y + 2.0 * u * t * y1 + t * t * b]); }
                x = a; y = b;
            }
            S::CurveTo { x1, y1, x2, y2, x: a, y: b } => {
                if cur.is_empty() { cur.push([x, y]); }
                for i in 1..=16 { let (t, u) = (i as f64 / 16.0, 1.0 - i as f64 / 16.0);
                    cur.push([u.powi(3) * x + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t.powi(3) * a,
                              u.powi(3) * y + 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t.powi(3) * b]); }
                x = a; y = b;
            }
            S::ClosePath => { if !cur.is_empty() { cur.push([sx, sy]); } fin(&mut cur, &mut polys); x = sx; y = sy; }
        }
    }
    fin(&mut cur, &mut polys);
    polys
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
    let mut warn = vec![]; let mut polys = vec![];
    for nd in doc.descendants().filter(|n| n.is_element()) {
        if let Some(d) = shape_d(&nd) {
            if nd.attribute("transform").is_some() { warn.push("transform attribute ignored".into()); }
            polys.extend(flatten(&d, &mut warn));
        }
    }
    warn.dedup();
    Ok(polys_to_conv(polys, unit, o, warn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcode::Opt;

    #[test]
    fn rect_arc_polyline() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='100mm' viewBox='0 0 100 100'><rect x='5' y='5' width='20' height='10'/><path d='M10 80 A20 20 0 0 1 50 80'/><polyline points='1,1 2,2 3,3'/></svg>";
        let c = svg_to_conv(svg, &Opt::default()).unwrap();
        assert!(!c.gcode.is_empty());
        assert!(c.paths.len() >= 3, "three shapes -> three polylines");
        assert!(c.warnings.is_empty());
        assert!((c.paper[0] - 210.0).abs() < 0.1 && (c.paper[1] - 297.0).abs() < 0.1);
    }

    #[test]
    fn mm_width_sets_unit() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='20mm' viewBox='0 0 20 10'><rect x='0' y='0' width='20' height='10'/></svg>";
        let c = svg_to_conv(svg, &Opt::default()).unwrap();
        let bb = c.bbox.unwrap();
        assert!((bb[2] - bb[0] - 20.0).abs() < 0.01, "20 viewBox units = 20 mm");
    }

    #[test]
    fn circle_flattens() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='50mm' viewBox='0 0 50 50'><circle cx='25' cy='25' r='10'/></svg>";
        let c = svg_to_conv(svg, &Opt::default()).unwrap();
        assert_eq!(c.paths.len(), 1);
        assert!(c.paths[0].len() >= 60, "four cubic segments x 16");
    }

    #[test]
    fn transform_warns_but_draws() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='50mm' viewBox='0 0 50 50'><rect x='1' y='1' width='10' height='5' transform='translate(2 2)'/></svg>";
        let c = svg_to_conv(svg, &Opt::default()).unwrap();
        assert!(c.warnings.iter().any(|s| s.contains("transform")));
        assert!(!c.paths.is_empty());
    }

    #[test]
    fn invalid_svg_is_error() {
        assert!(svg_to_conv("not an svg", &Opt::default()).is_err());
    }
}
