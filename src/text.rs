//! Text/Markdown -> G-code. Google Fonts (full TTF) with cache, word wrap, pages, 4 letter styles:
//! pen (one continuous centre line, handwritten) / single (fast thin contour) / fill (dense solid) / hatch (diagonal).
use crate::gcode::{bbox, paper, polys_to_conv, Conv, Opt, Polys};
use crate::skeleton::{glyph_strokes, Outline};
use std::{fs, io::Read, path::PathBuf};
use ttf_parser::OutlineBuilder;

pub const FONTS: [&str; 14] = ["Roboto", "Open Sans", "PT Sans", "PT Serif", "Noto Serif", "Lora", "Playfair Display", "Ubuntu", "Comfortaa", "Caveat", "Marck Script", "Neucha", "Philosopher", "Underdog"];
const TEST: &str = "äöõüÄÖÕÜжЖя"; // Estonian + Cyrillic coverage probe

fn score(data: &[u8]) -> i32 { ttf_parser::Face::parse(data, 0).map(|f| TEST.chars().filter(|&c| f.glyph_index(c).is_some()).count() as i32).unwrap_or(-1) }

fn get(url: &str) -> Result<Vec<u8>, String> {
    let r = ureq::get(url).timeout(std::time::Duration::from_secs(20)).call().map_err(|e| e.to_string())?;
    let mut v = vec![]; r.into_reader().take(20_000_000).read_to_end(&mut v).map_err(|e| e.to_string())?; Ok(v)
}

fn download(name: &str, file: &PathBuf) -> Result<(), String> {
    let q: String = name.replace(' ', "+");
    // no browser User-Agent -> Google returns full TTF (not woff2 subsets)
    let css = String::from_utf8_lossy(&get(&format!("https://fonts.googleapis.com/css?family={q}&subset=cyrillic,latin"))?).to_string();
    let urls: Vec<String> = css.split("url(").skip(1).filter_map(|s| s.split(')').next()).filter(|u| u.starts_with("https:")).map(String::from).collect();
    if urls.is_empty() { return Err(format!("Font not found: {name}")); }
    let (mut best, mut bs) = (vec![], -2);
    for u in urls { let b = get(&u)?; let s = score(&b); if s > bs { bs = s; best = b; } }
    fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?; fs::write(file, best).map_err(|e| e.to_string())
}

fn load_font(name: &str, warn: &mut Vec<String>) -> Result<Vec<u8>, String> {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".cache/plotter-controller/fonts");
    let file = dir.join(format!("{}.ttf", name.replace(|c: char| !c.is_alphanumeric(), "_")));
    let r = if file.exists() { Ok(()) } else { download(name, &file) };
    match r.and_then(|_| fs::read(&file).map_err(|e| e.to_string())) {
        Ok(d) => Ok(d),
        Err(e) => {
            for p in ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf"] {
                if let Ok(d) = fs::read(p) { warn.push(format!("Font \"{name}\" unavailable ({e}); using DejaVu Sans")); return Ok(d); }
            }
            Err(format!("Cannot load font \"{name}\" ({e}). Internet needed for first download."))
        }
    }
}

fn strip_md(t: &str) -> String {
    t.lines().map(|l| { let l = l.trim_start_matches('#').trim_start(); let l = l.strip_prefix('>').map(str::trim_start).unwrap_or(l);
        let l = if l.starts_with("- ") || l.starts_with("* ") || l.starts_with("+ ") { format!("• {}", &l[2..]) } else { l.to_string() };
        l.replace(['*', '_', '`'], "") }).collect::<Vec<_>>().join("\n")
}

/// Drop consecutive duplicate points (zero-length moves after chaining).
fn dedupe(p: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    let mut o: Vec<[f64; 2]> = vec![];
    for v in p { if o.last().map_or(true, |l| (l[0] - v[0]).abs() > 1e-9 || (l[1] - v[1]).abs() > 1e-9) { o.push(v); } }
    o
}

/// Join the strokes of one glyph into a single continuous path (nearest-endpoint chaining):
/// the pen runs along the middle of every stroke without lifting, like handwriting.
fn chain(mut s: Polys) -> Polys {
    if s.len() < 2 { return s; }
    let mut cur = dedupe(s.remove(0));
    while !s.is_empty() {
        let e = *cur.last().unwrap();
        let bi = s.iter().enumerate().min_by(|x, y| {
            let d = |p: &[[f64; 2]], r: bool| { let q = if r { p[p.len() - 1] } else { p[0] }; (e[0] - q[0]).hypot(e[1] - q[1]) };
            let da = d(x.1, false).min(d(x.1, true)); let db = d(y.1, false).min(d(y.1, true));
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        }).unwrap().0;
        let mut next = s.remove(bi);
        let d0 = (e[0] - next[0][0]).hypot(e[1] - next[0][1]);
        let d1 = (e[0] - next[next.len() - 1][0]).hypot(e[1] - next[next.len() - 1][1]);
        if d1 < d0 { next.reverse(); }
        cur.extend(next); cur = dedupe(cur);
    }
    vec![cur]
}

/// Even-odd infill of closed polygons: straight lines every `spacing` mm at `angle` degrees,
/// zigzag-connected between neighbouring scanlines (the pen lifts only over real gaps).
fn fill_lines(polys: &Polys, spacing: f64, angle: f64) -> Polys {
    let th = angle.to_radians(); let (sn, cs) = th.sin_cos();
    let rp: Polys = polys.iter().map(|p| p.iter().map(|v| [v[0] * cs + v[1] * sn, -v[0] * sn + v[1] * cs]).collect()).collect();
    let [_x0, y0, _x1, y1] = bbox(&rp);
    let mut out: Polys = vec![];
    let (mut flip, mut y) = (false, y0 + spacing * 0.5);
    while y < y1 {
        let mut xs: Vec<f64> = vec![];
        for p in &rp { let n = p.len();
            for i in 0..n { let (a, b) = (p[i], p[(i + 1) % n]);
                if (a[1] <= y) != (b[1] <= y) { xs.push(a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0])); } } }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut segs: Vec<(f64, f64)> = xs.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).filter(|(a, b)| b - a > 0.05).collect();
        if !segs.is_empty() {
            if flip { segs.reverse(); for s in &mut segs { std::mem::swap(&mut s.0, &mut s.1); } }
            flip = !flip;
            for (a, b) in segs {
                let (p, q) = ([a, y], [b, y]);
                let near = out.last().and_then(|l| l.last())
                    .map_or(false, |pp| ((pp[0] - p[0]).powi(2) + (pp[1] - p[1]).powi(2)).sqrt() < spacing * 2.5);
                if near { out.last_mut().unwrap().extend([p, q]); } else { out.push(vec![p, q]); }
            }
        }
        y += spacing;
    }
    out.into_iter().map(|p| p.iter().map(|v| [v[0] * cs - v[1] * sn, v[0] * sn + v[1] * cs]).collect()).collect()
}

/// Relative plotting speed of each letter style (multiplies the print speed setting).
fn feed_mult(mode: &str) -> f64 {
    match mode { "single" => 1.5, "fill" => 0.5, "hatch" => 0.8, _ => 1.0 }
}

/// Infill step for fill/hatch: user override (`fill_spacing` > 0) or automatic from the font size.
fn infill_step(o: &Opt, auto: f64, lo: f64, hi: f64) -> f64 {
    if o.fill_spacing > 0.0 { o.fill_spacing.clamp(0.05, 10.0) } else { auto.clamp(lo, hi) }
}

pub fn text_to_conv(text: &str, o: &Opt) -> Result<Conv, String> {
    let mut warn = vec![];
    let data = load_font(&o.font, &mut warn)?;
    let face = ttf_parser::Face::parse(&data, 0).map_err(|e| e.to_string())?;
    let (upm, sz) = (face.units_per_em() as f64, o.size);
    let text = if o.markdown { strip_md(text) } else { text.to_string() };
    let (pw, ph) = paper(o, &mut warn);
    let mut miss: Vec<char> = text.chars().filter(|&c| !c.is_whitespace() && face.glyph_index(c).is_none()).collect(); miss.sort(); miss.dedup();
    if !miss.is_empty() { warn.push(format!("Font has no glyphs for: {}", miss.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" "))); }
    let adv = |c: char| face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)).unwrap_or(0) as f64 / upm * sz;
    let width = |s: &str| s.chars().map(adv).sum::<f64>();

    let maxw = pw - o.start_x - o.margin;
    let mut lines: Vec<String> = vec![];
    for para in text.split('\n') {
        let mut line = String::new();
        for w in para.split_whitespace() {
            let t = if line.is_empty() { w.to_string() } else { format!("{line} {w}") };
            if !line.is_empty() && width(&t) > maxw { lines.push(std::mem::take(&mut line)); line = w.to_string(); } else { line = t; }
        }
        lines.push(line);
    }
    let step = sz * o.line_height;
    let max_lines = (((ph - o.start_y - o.margin - sz) / step).floor() as i64 + 1).max(1) as usize;
    let pages = ((lines.len() + max_lines - 1) / max_lines).max(1);
    let page = o.page.unwrap_or(0).min(pages - 1);
    if o.page.is_some() { lines = lines.into_iter().skip(page * max_lines).take(max_lines).collect(); }
    else if lines.len() > max_lines { warn.push(format!("Text truncated: {} line(s) do not fit on the sheet", lines.len() - max_lines)); lines.truncate(max_lines); }
    while lines.last().map_or(false, |l| l.is_empty()) { lines.pop(); }

    let mut polys: Polys = vec![];
    let fm = o.fill_mode.as_str();
    for (i, l) in lines.iter().enumerate() {
        let base = sz * 0.8 + i as f64 * step; let mut x = 0.0;
        for ch in l.chars() {
            match fm {
                "pen" => { // one continuous line through the stroke centres, handwritten look
                    let st: Polys = glyph_strokes(&face, ch).into_iter()
                        .map(|s| s.iter().map(|p| [x + p[0] * sz, base + p[1] * sz]).collect()).collect();
                    polys.extend(chain(st));
                }
                "single" => { // separate centre-line stroke per pen-down: fast thin contour
                    for st in glyph_strokes(&face, ch) { polys.push(st.iter().map(|p| [x + p[0] * sz, base + p[1] * sz]).collect()); }
                }
                "fill" | "hatch" => { // outline + infill: printed look
                    if let Some(g) = face.glyph_index(ch) {
                        let mut ob = Outline::new(x, base, sz / upm); face.outline_glyph(g, &mut ob); ob.close();
                        if !ob.polys.is_empty() {
                            // spacing: user setting wins over the automatic step; angle: 0° for solid fill, adjustable for hatch
                            let (sp, ang) = if fm == "fill" { (infill_step(o, sz / 15.0, 0.3, 1.0), 0.0) } else { (infill_step(o, sz / 4.0, 1.0, 3.0), o.hatch_angle) };
                            polys.extend(fill_lines(&ob.polys, sp, ang));
                            polys.extend(ob.polys);
                        }
                    }
                }
                _ => {}
            }
            x += adv(ch);
        }
    }
    let mut o2 = o.clone(); o2.keep_origin = true; o2.margin = 0.0; o2.center = false; o2.scale = 100.0; o2.offset_x = o.start_x; o2.offset_y = o.start_y;
    o2.feed *= feed_mult(fm);
    if polys.is_empty() { warn.push("Nothing to draw".into()); }
    let mut c = polys_to_conv(polys, 1.0, &o2, warn); c.pages = Some(pages); c.page = Some(page);
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_stripped() {
        assert_eq!(strip_md("# Заголовок\n- пункт\n*курсив*"), "Заголовок\n• пункт\nкурсив");
    }

    fn ensure_test_font() {
        let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".cache/plotter-controller/fonts");
        fs::create_dir_all(&dir).unwrap();
        let dst = dir.join("DejaVu_Sans.ttf");
        if !dst.exists() { fs::copy("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", &dst).unwrap(); }
    }

    #[test]
    fn text_conversion_from_local_cache() {
        ensure_test_font();
        let o = Opt { font: "DejaVu Sans".into(), ..Default::default() };
        let c = text_to_conv("Привет, мир!", &o).unwrap();
        assert!(!c.gcode.is_empty());
        assert!(c.gcode.contains("M3 S255") && c.gcode.contains("M5"));
        assert!(c.warnings.is_empty(), "no missing glyphs: {:?}", c.warnings);
        assert_eq!(c.pages, Some(1));
    }

    #[test]
    fn long_text_is_paginated() {
        ensure_test_font();
        let o = Opt { font: "DejaVu Sans".into(), size: 10.0, page: Some(1), ..Default::default() };
        let text = "слово ".repeat(400);
        let c = text_to_conv(&text, &o).unwrap();
        assert!(c.pages.unwrap() > 1, "many pages expected");
        assert_eq!(c.page, Some(1), "requested page 1");
        assert!(!c.gcode.is_empty());
    }

    #[test]
    fn pen_mode_chains_strokes_of_a_glyph() {
        ensure_test_font();
        let downs = |fm: &str| {
            let o = Opt { font: "DejaVu Sans".into(), fill_mode: fm.into(), ..Default::default() };
            text_to_conv("шш", &o).unwrap().gcode.matches("M3 S255").count()
        };
        let (pen, single) = (downs("pen"), downs("single"));
        assert!(pen > 0 && pen < single, "pen must chain strokes: pen={pen} single={single}");
    }

    #[test]
    fn fill_and_hatch_add_infill_and_scale_feed() {
        ensure_test_font();
        let mk = |fm: &str| {
            let o = Opt { font: "DejaVu Sans".into(), fill_mode: fm.into(), ..Default::default() };
            text_to_conv("Ш", &o).unwrap()
        };
        let (single, fill, hatch) = (mk("single"), mk("fill"), mk("hatch"));
        assert!(single.gcode.contains("F4500"), "single feed = 3000 * 1.5");
        assert!(fill.gcode.contains("F1500"), "fill feed = 3000 * 0.5");
        assert!(hatch.gcode.contains("F2400"), "hatch feed = 3000 * 0.8");
        assert!(fill.gcode.len() > single.gcode.len() * 2, "dense fill must add many lines");
        assert!(hatch.gcode.len() > single.gcode.len(), "hatch must add infill lines");
    }

    #[test]
    fn pen_mode_is_the_default() {
        assert_eq!(Opt::default().fill_mode, "pen");
    }

    #[test]
    fn fill_spacing_override_changes_density() {
        ensure_test_font();
        let mk = |sp: f64| {
            let o = Opt { font: "DejaVu Sans".into(), fill_mode: "fill".into(), fill_spacing: sp, ..Default::default() };
            text_to_conv("ШШ", &o).unwrap()
        };
        let dense = mk(0.25); let sparse = mk(1.2);
        assert!(dense.gcode.len() > sparse.gcode.len() * 2, "0.25 mm fill must be much denser than 1.2 mm");
        let auto = mk(0.0).gcode.len();
        assert!(auto > sparse.gcode.len(), "auto default is denser than 1.2 mm");
    }

    #[test]
    fn hatch_angle_changes_geometry() {
        ensure_test_font();
        let mk = |a: f64| {
            let o = Opt { font: "DejaVu Sans".into(), fill_mode: "hatch".into(), hatch_angle: a, ..Default::default() };
            text_to_conv("Ш", &o).unwrap()
        };
        let (a45, a0, a90) = (mk(45.0), mk(0.0), mk(90.0));
        assert_ne!(a45.paths, a0.paths, "45° vs 0° hatch must differ");
        assert_ne!(a45.paths, a90.paths, "45° vs 90° hatch must differ");
    }

    #[test]
    fn infill_defaults_are_auto_and_45() {
        let o = Opt::default();
        assert_eq!(o.fill_spacing, 0.0, "0 = automatic spacing");
        assert_eq!(o.hatch_angle, 45.0);
    }
}
