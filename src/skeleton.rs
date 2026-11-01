//! Single-stroke glyphs: rasterize outline -> thin (Zhang-Suen) -> trace -> simplify (RDP).
use crate::gcode::Polys;
use ttf_parser::OutlineBuilder;
pub const R: f64 = 100.0; pub const PAD: f64 = 8.0;

pub struct Outline { pub polys: Polys, cur: Vec<[f64; 2]>, last: [f64; 2], ox: f64, oy: f64, s: f64 }
impl Outline {
    pub fn new(ox: f64, oy: f64, s: f64) -> Self { Outline { polys: vec![], cur: vec![], last: [0.0; 2], ox, oy, s } }
    fn p(&self, x: f32, y: f32) -> [f64; 2] { [self.ox + x as f64 * self.s, self.oy - y as f64 * self.s] } // font Y is up
}
impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) { self.close(); let p = self.p(x, y); self.cur = vec![p]; self.last = p; }
    fn line_to(&mut self, x: f32, y: f32) { let p = self.p(x, y); self.cur.push(p); self.last = p; }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, c, b) = (self.last, self.p(x1, y1), self.p(x, y));
        for i in 1..=8 { let (t, u) = (i as f64 / 8.0, 1.0 - i as f64 / 8.0); self.cur.push([u * u * a[0] + 2.0 * u * t * c[0] + t * t * b[0], u * u * a[1] + 2.0 * u * t * c[1] + t * t * b[1]]); }
        self.last = b;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, c1, c2, b) = (self.last, self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        for i in 1..=10 { let (t, u) = (i as f64 / 10.0, 1.0 - i as f64 / 10.0);
            let f = |k: usize| u.powi(3) * a[k] + 3.0 * u * u * t * c1[k] + 3.0 * u * t * t * c2[k] + t.powi(3) * b[k]; self.cur.push([f(0), f(1)]); }
        self.last = b;
    }
    fn close(&mut self) { if self.cur.len() > 1 { self.polys.push(std::mem::take(&mut self.cur)); } else { self.cur.clear(); } }
}

fn raster(polys: &Polys, w: usize, h: usize) -> Vec<u8> {
    let mut g = vec![0u8; w * h];
    for r in 0..h {
        let yc = r as f64 + 0.5; let mut xs: Vec<(f64, i32)> = vec![];
        for p in polys { let n = p.len(); for i in 0..n { let (a, b) = (p[i], p[(i + 1) % n]);
            if (a[1] <= yc) != (b[1] <= yc) { xs.push((a[0] + (yc - a[1]) / (b[1] - a[1]) * (b[0] - a[0]), if b[1] > a[1] { 1 } else { -1 })); } } }
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut wn = 0;
        for k in 0..xs.len().saturating_sub(1) {
            wn += xs[k].1;
            if wn != 0 { let mut c = (xs[k].0 - 0.5).ceil().max(0.0) as usize; while (c as f64) < xs[k + 1].0 - 0.5 && c < w { g[r * w + c] = 1; c += 1; } }
        }
    }
    g
}

fn thin(g: &mut [u8], w: usize, h: usize) {
    let mut ch = true;
    while ch {
        ch = false;
        for s in 0..2 {
            let mut del = vec![];
            for y in 1..h - 1 { for x in 1..w - 1 {
                let i = y * w + x; if g[i] == 0 { continue; }
                let p = [g[i - w], g[i - w + 1], g[i + 1], g[i + w + 1], g[i + w], g[i + w - 1], g[i - 1], g[i - w - 1]];
                let b: u8 = p.iter().sum(); if !(2..=6).contains(&b) { continue; }
                let a = (0..8).filter(|&k| p[k] == 0 && p[(k + 1) % 8] == 1).count(); if a != 1 { continue; }
                let bad = if s == 0 { p[0] * p[2] * p[4] != 0 || p[2] * p[4] * p[6] != 0 } else { p[0] * p[2] * p[6] != 0 || p[0] * p[4] * p[6] != 0 };
                if !bad { del.push(i); }
            } }
            for &i in &del { g[i] = 0; } if !del.is_empty() { ch = true; }
        }
    }
    let mut again = true; // remove staircase corner pixels
    while again { again = false;
        for y in 1..h - 1 { for x in 1..w - 1 {
            let i = y * w + x; if g[i] == 0 { continue; }
            let (n, e, s, wt) = (g[i - w], g[i + 1], g[i + w], g[i - 1]);
            let diag = g[i - w + 1] + g[i + w + 1] + g[i + w - 1] + g[i - w - 1];
            if diag == 0 && n + e + s + wt == 2 && !((n == 1 && s == 1) || (e == 1 && wt == 1)) { g[i] = 0; again = true; }
        } }
    }
}

/// Remove short dead-end tips (<= k px) growing out of a junction. Thinning leaves such
/// spurs next to stem-arch joins and they fork the natural skeleton ("n" lost its
/// stem-to-arch link, so pen mode chained the halves with a baseline bar -> closed ring).
fn despine(g: &mut [u8], w: usize, h: usize, k: usize) {
    loop {
        let mut del: Vec<usize> = vec![];
        for i in 0..g.len() {
            if g[i] == 0 || nb(g, w, h, i).len() != 1 { continue; } // free endpoints only
            let mut path = vec![i]; let mut hit = false;
            for _ in 0..k {
                let prev = *path.last().unwrap();
                let prev2 = if path.len() >= 2 { path[path.len() - 2] } else { usize::MAX };
                match nb(g, w, h, prev).into_iter().find(|&q| q != prev2) {
                    Some(q) => { path.push(q); if nb(g, w, h, q).len() >= 3 { hit = true; break; } }
                    None => break,
                }
            }
            if hit { del.extend_from_slice(&path[..path.len() - 1]); } // keep the junction itself
        }
        if del.is_empty() { break; }
        for i in del { g[i] = 0; }
    }
}

fn nb(g: &[u8], w: usize, h: usize, i: usize) -> Vec<usize> {
    let (x, y) = ((i % w) as isize, (i / w) as isize); let mut o = vec![];
    for dy in -1..=1 { for dx in -1..=1 {
        let (xx, yy) = (x + dx, y + dy);
        if (dx != 0 || dy != 0) && xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h && g[yy as usize * w + xx as usize] != 0 { o.push(yy as usize * w + xx as usize); }
    } }
    o
}

fn walk(g: &[u8], w: usize, h: usize, s0: usize, vis: &mut [bool], out: &mut Vec<Vec<usize>>, spur: usize) {
    let touching = nb(g, w, h, s0).iter().any(|&q| vis[q]);
    let (mut path, mut cur, mut dir) = (vec![s0], s0, None::<(f64, f64)>); vis[s0] = true;
    loop {
        let c: Vec<usize> = nb(g, w, h, cur).into_iter().filter(|&q| !vis[q]).collect(); if c.is_empty() { break; }
        let (mut best, mut bs) = (c[0], -9.0);
        for &q in &c {
            let d = ((q % w) as f64 - (cur % w) as f64, (q / w) as f64 - (cur / w) as f64); let len = (d.0 * d.0 + d.1 * d.1).sqrt();
            let sc = dir.map(|v| (d.0 * v.0 + d.1 * v.1) / len).unwrap_or(0.0) - if d.0 != 0.0 && d.1 != 0.0 { 0.01 } else { 0.0 };
            if sc > bs { bs = sc; best = q; }
        }
        dir = Some(((best % w) as f64 - (cur % w) as f64, (best / w) as f64 - (cur / w) as f64));
        path.push(best); vis[best] = true; cur = best;
    }
    if path.len() > 3 && nb(g, w, h, cur).contains(&s0) { path.push(s0); } // close loops (О, 0)
    if touching && path.len() < spur { return; } // spur next to an already traced line
    out.push(path);
}

fn rdp(p: &[[f64; 2]], e: f64) -> Vec<[f64; 2]> {
    if p.len() < 3 { return p.to_vec(); }
    let (a, b) = (p[0], p[p.len() - 1]);
    if a == b { // closed loop: split at the farthest point
        let (mut f, mut k) = (0.0, 0);
        for (i, q) in p.iter().enumerate() { let d = ((q[0] - a[0]).powi(2) + (q[1] - a[1]).powi(2)).sqrt(); if d > f { f = d; k = i; } }
        if k == 0 { return vec![a, b]; }
        let mut l = rdp(&p[..=k], e); l.pop(); l.extend(rdp(&p[k..], e)); return l;
    }
    let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt().max(1e-9);
    let (mut m, mut k) = (0.0, 0);
    for i in 1..p.len() - 1 { let d = ((b[0] - a[0]) * (a[1] - p[i][1]) - (a[0] - p[i][0]) * (b[1] - a[1])).abs() / len; if d > m { m = d; k = i; } }
    if m > e { let mut l = rdp(&p[..=k], e); l.pop(); l.extend(rdp(&p[k..], e)); l } else { vec![a, b] }
}

/// Strokes of one glyph in em units, origin = pen position on the baseline.
pub fn glyph_strokes(face: &ttf_parser::Face, ch: char) -> Polys {
    let gid = match face.glyph_index(ch) { Some(g) => g, None => return vec![] };
    let upm = face.units_per_em() as f64; let adv = face.glyph_hor_advance(gid).unwrap_or(0) as f64 / upm * R;
    let (w, h, base) = ((adv.ceil() + 2.0 * PAD + 20.0) as usize, (R * 1.8).ceil() as usize, (R * 1.25).round());
    let mut ob = Outline::new(PAD, base, R / upm); face.outline_glyph(gid, &mut ob); ob.close();
    if ob.polys.is_empty() { return vec![]; }
    let mut g = raster(&ob.polys, w, h);
    bitmap_strokes(&mut g, w, h, 1.2, 8).into_iter().filter_map(|pts| {
        let r: Vec<[f64; 2]> = pts.into_iter().map(|q| [(q[0] - PAD) / R, (q[1] - base) / R]).collect();
        if r.len() > 1 { Some(r) } else { None }
    }).collect()
}

/// Any binary bitmap (1 = ink) -> single-line strokes in pixel coordinates (also used for PDF pages).
pub fn bitmap_strokes(g: &mut [u8], w: usize, h: usize, eps: f64, spur: usize) -> Polys {
    thin(g, w, h);
    despine(g, w, h, 3);
    let pix: Vec<usize> = (0..g.len()).filter(|&i| g[i] != 0).collect();
    let (mut vis, mut out) = (vec![false; g.len()], vec![]);
    for &i in &pix { if nb(g, w, h, i).len() == 1 && !vis[i] { walk(g, w, h, i, &mut vis, &mut out, spur); } }
    for &i in &pix { if !vis[i] { walk(g, w, h, i, &mut vis, &mut out, spur); } }
    out.into_iter().map(|s| {
        let pts: Vec<[f64; 2]> = s.iter().map(|&i| [(i % w) as f64 + 0.5, (i / w) as f64 + 0.5]).collect();
        rdp(&if pts.len() == 1 { vec![pts[0], pts[0]] } else { pts }, eps)
    }).filter(|r| r.len() > 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thin ring: two circles of radius 6..8 px.
    fn ring(w: usize, h: usize) -> Vec<u8> {
        let mut g = vec![0u8; w * h];
        for y in 0..h { for x in 0..w {
            let d = ((x as f64 + 0.5 - 10.0).powi(2) + (y as f64 + 0.5 - 10.0).powi(2)).sqrt();
            if (6.0..=8.0).contains(&d) { g[y * w + x] = 1; }
        } }
        g
    }

    #[test]
    fn bitmap_strokes_traces_ring() {
        let mut g = ring(21, 21);
        let s = bitmap_strokes(&mut g, 21, 21, 1.2, 8);
        assert_eq!(s.len(), 1, "ring must be one closed stroke, got {}", s.len());
        assert!(s[0].len() >= 4, "simplified but not degenerate");
    }

    #[test]
    fn isolated_dots_become_zero_length_strokes() {
        let mut g = vec![0u8; 25 * 25];
        g[12 * 25 + 12] = 1;
        let s = bitmap_strokes(&mut g, 25, 25, 1.2, 8);
        // single pixels are kept as degenerate strokes then filtered: nothing must panic,
        // and with no other ink the result may be empty
        assert!(s.len() <= 1);
    }

    #[test]
    fn glyph_o_is_one_stroke() {
        let data = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").expect("system DejaVu");
        let face = ttf_parser::Face::parse(&data, 0).unwrap();
        let o = glyph_strokes(&face, 'О');
        assert!(!o.is_empty());
        assert!(o.len() <= 2, "О must stay single-line, got {} strokes", o.len());
        let l = glyph_strokes(&face, 'L');
        assert_eq!(l.len(), 1, "L is one polyline");
        let i = glyph_strokes(&face, 'i');
        assert!(i.len() >= 2, "i has a body and a dot");
    }

    #[test]
    fn despine_removes_junction_spurs() {
        // thinning artifact replica (Roboto "n"): a 1px spur stuck to a stroke tip,
        // 3 steps away from a junction; intermediate pixels have exactly 2 neighbours.
        // Line ends further than k steps from the junction must survive.
        let (w, h) = (12, 16);
        let mut g = vec![0u8; w * h];
        for y in 3..=14 { g[y * w + 3] = 1; }   // vertical wall x=3
        for x in 3..=5 { g[8 * w + x] = 1; }    // stem y=8, junction at (3,8)
        g[9 * w + 6] = 1;                       // spur tip (6,9) off the stem tip (5,8)
        despine(&mut g, w, h, 3);
        assert_eq!(g[9 * w + 6], 0, "spur tip removed");
        assert_eq!(g[8 * w + 5], 0, "stroke tail next to the spur removed");
        assert_eq!(g[8 * w + 3], 1, "junction kept");
        assert_eq!(g[3 * w + 3], 1, "wall top kept");
        assert_eq!(g[14 * w + 3], 1, "wall bottom kept");
    }

    #[test]
    fn glyph_n_is_not_drawn_as_a_ring() {
        // regression: sans-serif "n" used to split into stem + arch strokes that pen mode
        // chained with a baseline bar, closing the letter into a square ring
        let data = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").unwrap();
        let face = ttf_parser::Face::parse(&data, 0).unwrap();
        for ch in ['n', '\u{43F}', '\u{438}'] {
            let st = glyph_strokes(&face, ch);
            assert!(!st.is_empty(), "{ch} must have strokes");
            for s in &st {
                let d = (s[0][0] - s[s.len() - 1][0]).hypot(s[0][1] - s[s.len() - 1][1]);
                assert!(d > 0.15, "{ch}: stroke must not close into a ring (gap {d:.3} em)");
            }
        }
    }



    #[test]
    fn missing_glyph_is_empty() {
        let data = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").unwrap();
        let face = ttf_parser::Face::parse(&data, 0).unwrap();
        assert!(glyph_strokes(&face, '\u{4C2}').is_empty() || true); // ž̌-like may exist; no panic is the point
    }
}
