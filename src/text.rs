//! Text/Markdown -> G-code. Google Fonts (full TTF) with cache, word wrap, pages, 4 letter styles:
//! pen (one continuous centre line, handwritten) / single (fast thin contour) / fill (dense solid) / hatch (diagonal).
//! Since 0.5.16 every font installed on the system is selectable too: the family comes from the
//! font's own name table, the value sent back to the converter is `sys:<path>` (or `sys:<path>#<n>`
//! for a TrueType collection) and is read straight from disk — no download, no cache.
use crate::gcode::{bbox, paper, polys_to_conv, Conv, Opt, Polys};
use crate::skeleton::{glyph_strokes, Outline};
use std::{fs, io::Read, path::PathBuf};
use ttf_parser::OutlineBuilder;

pub const FONTS: [&str; 14] = ["Roboto", "Open Sans", "PT Sans", "PT Serif", "Noto Serif", "Lora", "Playfair Display", "Ubuntu", "Comfortaa", "Caveat", "Marck Script", "Neucha", "Philosopher", "Underdog"];
const TEST: &str = "äöõüÄÖÕÜжЖя"; // Estonian + Cyrillic coverage probe

/// One available style of a system family: canonical key (regular/bold/italic/bolditalic),
/// display name and the `sys:`-value of the exact face file that provides it.
#[derive(serde::Serialize)]
pub struct SysStyle { pub key: &'static str, pub name: &'static str, pub value: String }

/// A system font family: `value` is the Regular face (or the only one present), `styles`
/// lists every distinct style found for this family in canonical order.
#[derive(serde::Serialize)]
pub struct SysFont { pub family: String, pub value: String, pub styles: Vec<SysStyle> }

const STYLE_KEYS: [(&str, &str, u16); 4] = [("regular", "Regular", 400), ("bold", "Bold", 700), ("italic", "Italic", 400), ("bolditalic", "Bold Italic", 700)];

/// Canonical style of a face from the OS/2 table: bold flag + italic/oblique.
fn style_key(f: &ttf_parser::Face) -> &'static str {
    let it = matches!(f.style(), ttf_parser::Style::Italic | ttf_parser::Style::Oblique);
    match (f.is_bold(), it) { (true, true) => "bolditalic", (true, false) => "bold", (false, true) => "italic", _ => "regular" }
}

fn score(data: &[u8]) -> i32 { ttf_parser::Face::parse(data, 0).map(|f| TEST.chars().filter(|&c| f.glyph_index(c).is_some()).count() as i32).unwrap_or(-1) }

fn get(url: &str) -> Result<Vec<u8>, String> {
    let r = ureq::get(url).timeout(std::time::Duration::from_secs(20)).call().map_err(|e| e.to_string())?;
    let mut v = vec![]; r.into_reader().take(20_000_000).read_to_end(&mut v).map_err(|e| e.to_string())?; Ok(v)
}

fn download(name: &str, variant: &str, file: &PathBuf) -> Result<(), String> {
    let q: String = name.replace(' ', "+");
    let spec = if variant.is_empty() { String::new() } else { format!(":{variant}") }; // legacy CSS API style token
    // no browser User-Agent -> Google returns full TTF (not woff2 subsets)
    let css = String::from_utf8_lossy(&get(&format!("https://fonts.googleapis.com/css?family={q}{spec}&subset=cyrillic,latin"))?).to_string();
    let urls: Vec<String> = css.split("url(").skip(1).filter_map(|s| s.split(')').next()).filter(|u| u.starts_with("https:")).map(String::from).collect();
    if urls.is_empty() { return Err(format!("Font not found: {name}")); }
    let (mut best, mut bs) = (vec![], -2);
    for u in urls { let b = get(&u)?; let s = score(&b); if s > bs { bs = s; best = b; } }
    fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?; fs::write(file, best).map_err(|e| e.to_string())
}

/// (font bytes, face index inside the file) — the index matters only for .ttc collections.
/// Accepts three forms: `sys:<path>[#n]` (system face, exact file), `Family:variant`
/// (Google font with bold/italic/bolditalic) and plain `Family` (Google regular).
fn load_font(name: &str, warn: &mut Vec<String>) -> Result<(Vec<u8>, u32), String> {
    // A system font: the value was produced by system_fonts(), read the file as-is.
    if let Some(p) = name.strip_prefix("sys:") {
        let (file, idx) = match p.rsplit_once('#') { Some((f, i)) => (f, i.parse().unwrap_or(0)), None => (p, 0) };
        let data = fs::read(file).map_err(|e| format!("Font file not available: {file} ({e})"))?;
        return Ok((data, idx));
    }
    let (base, variant) = match name.rsplit_once(':') {
        Some((b, v)) if matches!(v, "bold" | "italic" | "bolditalic") => (b, v),
        _ => (name, ""),
    };
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".cache/plotter-controller/fonts");
    let file = dir.join(format!("{}{}.ttf", base.replace(|c: char| !c.is_alphanumeric(), "_"), if variant.is_empty() { String::new() } else { format!("_{variant}") }));
    let r = if file.exists() { Ok(()) } else { download(base, variant, &file) };
    match r.and_then(|_| fs::read(&file).map_err(|e| e.to_string())) {
        Ok(d) => Ok((d, 0)),
        Err(e) => {
            // Google answers HTTP 400 for a variant the family simply does not have:
            // fall back to the Regular of the SAME family before reaching for DejaVu
            if !variant.is_empty() {
                warn.push(format!("Font \"{base}\" has no {variant} style; using Regular"));
                return load_font(base, warn);
            }
            for p in ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf"] {
                if let Ok(d) = fs::read(p) { warn.push(format!("Font \"{name}\" unavailable ({e}); using DejaVu Sans")); return Ok((d, 0)); }
            }
            Err(format!("Cannot load font \"{name}\" ({e}). Internet needed for first download."))
        }
    }
}

/// Family name from the name table: typographic family (16) preferred over family (1),
/// English (0x409) preferred over the rest; only Unicode-encoded records decode.
fn family_name(face: &ttf_parser::Face) -> Option<String> {
    let pick = |id: u16, lang: u16| face.names().into_iter().find(|n| n.name_id == id && n.language_id == lang).and_then(|n| n.to_string());
    pick(16, 0x409).or_else(|| pick(1, 0x409)).or_else(|| pick(16, 0)).or_else(|| pick(1, 0))
        .or_else(|| face.names().into_iter().find(|n| n.name_id == 16 || n.name_id == 1).and_then(|n| n.to_string()))
        .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

type FamMap = std::collections::HashMap<String, (String, std::collections::HashMap<&'static str, (i32, String)>)>;

fn walk_fonts(dir: &std::path::Path, depth: u8, fams: &mut FamMap) {
    if depth > 6 { return; }
    let mut items: Vec<_> = match fs::read_dir(dir) { Ok(rd) => rd.filter_map(|e| e.ok()).collect(), Err(_) => return };
    items.sort_by_key(|a| a.file_name()); // deterministic: ties are won by the first file seen
    for it in items {
        let p = it.path();
        if it.file_type().map(|t| t.is_dir()).unwrap_or(false) { walk_fonts(&p, depth + 1, fams); continue; } // symlinks never recurse — no loops
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if !matches!(ext.as_str(), "ttf" | "otf" | "ttc") { continue; }
        let Ok(data) = fs::read(&p) else { continue };
        let ps = p.to_string_lossy().to_string();
        let faces: Vec<u32> = if ext == "ttc" {
            (0..ttf_parser::fonts_in_collection(&data).unwrap_or(0)).collect()
        } else { vec![0] };
        for i in faces {
            if let Ok(f) = ttf_parser::Face::parse(&data, i) {
                if let Some(fam) = family_name(&f) {
                    let key = style_key(&f);
                    // among faces of the same canonical style keep the one whose weight is
                    // closest to the target (Thin/Black etc. never shadow Regular/Bold)
                    let target = STYLE_KEYS.iter().find(|k| k.0 == key).map(|k| k.2).unwrap_or(400) as i32;
                    let dist = (f.weight().to_number() as i32 - target).abs();
                    let value = if ext == "ttc" { format!("sys:{ps}#{i}") } else { format!("sys:{ps}") };
                    let e = fams.entry(fam.to_lowercase()).or_insert_with(|| (fam.clone(), std::collections::HashMap::new()));
                    match e.1.get(key) { Some(&(d, _)) if d <= dist => {} _ => { e.1.insert(key, (dist, value)); } }
                }
            }
        }
    }
}

/// Every font family installed on the system: the standard font directories are walked
/// recursively (user first, then system-wide), each .ttf/.otf/.ttc face is parsed and
/// classified (regular/bold/italic/bolditalic from OS/2). `value` is the Regular face;
/// `styles` carries an exact `sys:`-value per available style, sorted by family name.
pub fn system_fonts() -> Vec<SysFont> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let mut fams: FamMap = std::collections::HashMap::new();
    for d in [format!("{home}/.fonts"), format!("{home}/.local/share/fonts"), "/usr/local/share/fonts".into(), "/usr/share/fonts".into()] {
        let p = PathBuf::from(&d);
        if p.is_dir() { walk_fonts(&p, 0, &mut fams); }
    }
    let mut out: Vec<SysFont> = fams.into_iter().map(|(_, (family, sm))| {
        let styles: Vec<SysStyle> = STYLE_KEYS.iter().filter_map(|(k, n, _)|
            sm.get(k).map(|(_, v)| SysStyle { key: k, name: n, value: v.clone() })).collect();
        let value = styles.iter().find(|s| s.key == "regular").map(|s| s.value.clone())
            .or_else(|| styles.first().map(|s| s.value.clone())).unwrap_or_default();
        SysFont { family, value, styles }
    }).filter(|f| !f.styles.is_empty()).collect();
    out.sort_by(|a, b| a.family.to_lowercase().cmp(&b.family.to_lowercase()));
    out
}

/// Live font preview: the given text drawn as glyph contours (the same outlines the
/// fill/hatch styles plot) in mm, Y down, baseline at 8 mm. Returns polylines + the
/// bbox for the SVG viewBox.
pub fn font_preview(font: &str, text: &str) -> Result<serde_json::Value, String> {
    let mut warn = vec![];
    let (data, idx) = load_font(font, &mut warn)?;
    let face = ttf_parser::Face::parse(&data, idx).map_err(|e| e.to_string())?;
    let upm = face.units_per_em() as f64;
    let (sz, base) = (10.0_f64, 8.0_f64);
    let mut polys: Polys = vec![]; let mut x = 0.0_f64;
    for ch in text.chars().take(12) {
        if let Some(g) = face.glyph_index(ch) {
            let mut ob = Outline::new(x, base, sz / upm);
            face.outline_glyph(g, &mut ob); ob.close();
            polys.extend(ob.polys);
            x += face.glyph_hor_advance(g).unwrap_or(0) as f64 / upm * sz;
        }
    }
    let b = bbox(&polys);
    Ok(serde_json::json!({
        "paths": polys,
        "w": if b[2].is_finite() { b[2].max(1.0) } else { 1.0 },
        "h": if b[3].is_finite() { b[3].max(1.0) } else { 1.0 },
        "warn": warn
    }))
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

/// Join the strokes of one glyph into pen paths (nearest-endpoint chaining): the pen runs
/// along the middle of every stroke like handwriting. Strokes merge only across tiny gaps
/// (<= `join` mm); a bigger gap stays a separate path with a pen-up travel move — drawing
/// the connector pen-down produced a visible bar that closed "n" into a square ring.
fn chain(mut s: Polys, join: f64) -> Polys {
    if s.len() < 2 { return s; }
    let mut out: Polys = vec![];
    let mut cur = dedupe(s.remove(0));
    while !s.is_empty() {
        let e = *cur.last().unwrap();
        let (bi, rev, gap) = s.iter().enumerate().map(|(i, p)| {
            let d0 = (e[0] - p[0][0]).hypot(e[1] - p[0][1]);
            let d1 = (e[0] - p[p.len() - 1][0]).hypot(e[1] - p[p.len() - 1][1]);
            if d1 < d0 { (i, true, d1) } else { (i, false, d0) }
        }).min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
        let mut next = s.remove(bi);
        if rev { next.reverse(); }
        if gap <= join { cur.extend(next); cur = dedupe(cur); }
        else { out.push(std::mem::take(&mut cur)); cur = next; }
    }
    out.push(cur);
    out
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
    let (data, face_idx) = load_font(&o.font, &mut warn)?;
    let face = ttf_parser::Face::parse(&data, face_idx).map_err(|e| e.to_string())?;
    let (upm, sz) = (face.units_per_em() as f64, o.size);
    let text = if o.markdown { strip_md(text) } else { text.to_string() };
    let (pw, ph) = paper(o, &mut warn);
    let mut miss: Vec<char> = text.chars().filter(|&c| !c.is_whitespace() && face.glyph_index(c).is_none()).collect(); miss.sort(); miss.dedup();
    if !miss.is_empty() { warn.push(format!("Font has no glyphs for: {}", miss.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" "))); }
    let adv = |c: char| face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)).unwrap_or(0) as f64 / upm * sz;
    let width = |s: &str| s.chars().map(adv).sum::<f64>();

    // 0.5.22: per-side margins reserve space: right bounds the line width, bottom the page
    // height (legacy single margin behaved exactly the same way)
    let (_, mr, _, mb) = crate::gcode::margins(o);
    let maxw = pw - o.start_x - mr;
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
    let max_lines = (((ph - o.start_y - mb - sz) / step).floor() as i64 + 1).max(1) as usize;
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
                    polys.extend(chain(st, 0.1 * sz));
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
    let mut o2 = o.clone(); o2.keep_origin = true; o2.margin = 0.0; o2.margin_l = 0.0; o2.margin_r = 0.0; o2.margin_t = 0.0; o2.margin_b = 0.0; o2.center = false; o2.scale = 100.0; o2.offset_x = o.start_x; o2.offset_y = o.start_y;
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
    fn system_fonts_found_sorted_and_usable() {
        let sys = system_fonts();
        assert!(!sys.is_empty(), "the sandbox has system fonts");
        assert!(sys.iter().all(|f| f.value.starts_with("sys:")), "values carry the sys: prefix");
        assert!(sys.iter().all(|f| !f.styles.is_empty()), "every family has at least one style");
        assert!(sys.windows(2).all(|w| w[0].family.to_lowercase() <= w[1].family.to_lowercase()), "sorted case-insensitively");
        let fams: Vec<String> = sys.iter().map(|f| f.family.to_lowercase()).collect();
        assert!(fams.contains(&"dejavu sans".into()), "DejaVu Sans must be in the list: {fams:?}");
        let mut u = fams.clone(); u.sort(); u.dedup();
        assert_eq!(u.len(), fams.len(), "families are unique");
        let dj = sys.iter().find(|f| f.family.eq_ignore_ascii_case("dejavu sans")).unwrap();
        let reg = dj.styles.iter().find(|s| s.key == "regular").expect("regular style");
        assert_eq!(reg.value, dj.value, "value is the Regular face");
        let bold = dj.styles.iter().find(|s| s.key == "bold").expect("DejaVu Sans ships a Bold face");
        assert_ne!(reg.value, bold.value, "styles point to different face files");
        let c1 = text_to_conv("Привет, мир!", &Opt { font: reg.value.clone(), ..Default::default() }).unwrap();
        let c2 = text_to_conv("Привет, мир!", &Opt { font: bold.value.clone(), ..Default::default() }).unwrap();
        assert!(!c1.gcode.is_empty() && !c2.gcode.is_empty());
        assert_ne!(c1.gcode, c2.gcode, "Bold must draw differently from Regular");
        assert!(c1.warnings.is_empty(), "no missing glyphs: {:?}", c1.warnings);
    }

    #[test]
    fn google_bold_variant_uses_a_separate_cached_face() {
        let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".cache/plotter-controller/fonts");
        fs::create_dir_all(&dir).unwrap();
        for (src, dst) in [("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "DejaVu_Sans.ttf"), ("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "DejaVu_Sans_bold.ttf")] {
            let d = dir.join(dst);
            if !d.exists() { fs::copy(src, d).unwrap(); }
        }
        let reg = text_to_conv("ШШ", &Opt { font: "DejaVu Sans".into(), size: 10.0, ..Default::default() }).unwrap();
        let bold = text_to_conv("ШШ", &Opt { font: "DejaVu Sans:bold".into(), size: 10.0, ..Default::default() }).unwrap();
        assert!(!bold.gcode.is_empty());
        assert_ne!(reg.gcode, bold.gcode, "the bold variant must load a different face");
    }

    #[test]
    fn font_preview_draws_contours() {
        ensure_test_font();
        let v = font_preview("DejaVu Sans", "Шрифт").unwrap();
        let paths = v["paths"].as_array().unwrap();
        assert!(!paths.is_empty(), "preview must contain contours");
        assert!(v["w"].as_f64().unwrap() > 5.0 && v["h"].as_f64().unwrap() > 5.0);
        let empty = font_preview("DejaVu Sans", "  ").unwrap();
        assert_eq!(empty["paths"].as_array().unwrap().len(), 0, "whitespace has no contours");
        assert!(empty["w"].as_f64().unwrap() >= 1.0, "sane bbox fallback for empty output");
    }

    #[test]
    fn sys_prefix_missing_file_is_a_clean_error() {
        let o = Opt { font: "sys:/nonexistent/font.ttf".into(), ..Default::default() };
        match text_to_conv("x", &o) {
            Ok(_) => panic!("a missing system font file must fail"),
            Err(e) => assert!(e.contains("/nonexistent"), "{e}"),
        }
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
    fn chain_merges_only_tiny_gaps() {
        // pen mode chains glyph strokes like handwriting, but only across tiny gaps:
        // a long pen-down connector drew a visible bar and closed "n" into a ring
        let near = vec![vec![[0.0, 0.0], [1.0, 0.0]], vec![[1.05, 0.0], [2.0, 0.5]]];
        assert_eq!(chain(near, 0.1).len(), 1, "tiny gap merges into one pen path");
        let far = vec![vec![[0.0, 0.0], [1.0, 0.0]], vec![[3.0, 0.0], [4.0, 0.5]]];
        assert_eq!(chain(far, 0.1).len(), 2, "big gap stays a separate path (pen-up travel)");
        let rev = vec![vec![[0.0, 0.0], [1.0, 0.0]], vec![[2.0, 0.5], [1.05, 0.0]]];
        assert_eq!(chain(rev, 0.1).len(), 1, "stroke direction does not matter");
    }

    #[test]
    fn pen_mode_needs_no_more_pen_downs_than_single() {
        ensure_test_font();
        // 0.5.19 defaults emit M3 S255 as pen UP — counting its occurrences counts the lifts,
        // which is 1 + the number of pen-downs and keeps the same ordering.
        let lifts = |fm: &str| {
            let o = Opt { font: "DejaVu Sans".into(), fill_mode: fm.into(), ..Default::default() };
            text_to_conv("шш", &o).unwrap().gcode.matches("M3 S255").count()
        };
        let (pen, single) = (lifts("pen"), lifts("single"));
        assert!(pen > 0 && pen <= single, "pen must not be worse than single: pen={pen} single={single}");
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
    fn pen_mode_n_is_not_a_closed_ring() {
        // regression: "n"/"п" used to draw as a square ring — the skeleton split into
        // stem + arch and pen-down chaining added a baseline bar between them
        ensure_test_font();
        let o = Opt { font: "DejaVu Sans".into(), size: 10.0, ..Default::default() };
        let c = text_to_conv("nn", &o).unwrap();
        assert!(!c.paths.is_empty());
        for p in &c.paths {
            let d = (p[0][0] - p[p.len() - 1][0]).hypot(p[0][1] - p[p.len() - 1][1]);
            assert!(d > 1.5, "pen path must not close into a ring (gap {d:.2} mm)");
        }
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
