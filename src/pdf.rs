//! PDF page as a drawing -> G-code. Page is rendered (pure-Rust hayro), thinned to single-line strokes.
//! One PDF page = one sheet; landscape pages switch the sheet to landscape automatically.
use crate::gcode::{polys_to_conv, Conv, Opt, Polys};
use crate::skeleton::bitmap_strokes;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};

const PT: f64 = 25.4 / 72.0;

pub fn pdf_to_conv(buf: &[u8], o: &Opt, dpi: f64) -> Result<Conv, String> {
    let pdf = Pdf::new(buf.to_vec()).map_err(|e| format!("Cannot read PDF: {e:?}"))?;
    let pages = pdf.pages();
    let n = pages.iter().count();
    if n == 0 { return Err("PDF has no pages".into()); }
    let idx = o.page.unwrap_or(0).min(n - 1);
    let page = pages.iter().nth(idx).ok_or("page not found")?;
    let (wpt, hpt) = page.render_dimensions();
    let dpi = dpi.clamp(72.0, 300.0); let k = (dpi / 72.0) as f32;

    let pix = render(page, &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(),
        &PixmapSettings { x_scale: k, y_scale: k, bg_color: WHITE });
    let (w, h) = (pix.width() as usize, pix.height() as usize);
    let px = pix.data_as_u8_slice();
    let mut g: Vec<u8> = (0..w * h).map(|i| u8::from(px[i * 4] < 140 && px[i * 4 + 3] > 0)).collect(); // dark pixel = ink
    let mm = 25.4 / dpi;
    let paths: Polys = bitmap_strokes(&mut g, w, h, 1.2, 4).into_iter().map(|s| s.into_iter().map(|p| [p[0] * mm, p[1] * mm]).collect()).collect();

    let (pw_mm, ph_mm) = (wpt as f64 * PT, hpt as f64 * PT);
    let mut o2 = o.clone();
    o2.landscape = o.landscape || pw_mm > ph_mm; // follow the page orientation unless forced
    let mut warn = vec![];
    let (sw, sh) = crate::gcode::paper(&o2, &mut warn);
    // 0.5.22: right/bottom margins reserve space for the page fit (the legacy single margin did)
    let (_, mr, _, mb) = crate::gcode::margins(o);
    let f = ((sw - o.start_x - mr) / pw_mm).min((sh - o.start_y - mb) / ph_mm).min(1.0);
    if f < 0.995 { warn.push(format!("Page scaled to {:.0}% to fit the sheet", f * 100.0)); }
    o2.keep_origin = true; o2.margin = 0.0; o2.margin_l = 0.0; o2.margin_r = 0.0; o2.margin_t = 0.0; o2.margin_b = 0.0; o2.center = false; o2.scale = 100.0 * f;
    o2.offset_x = o.start_x + o.offset_x; o2.offset_y = o.start_y + o.offset_y;
    let mut c = polys_to_conv(paths, 1.0, &o2, warn);
    c.pages = Some(n); c.page = Some(idx);
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcode::Opt;

    /// Minimal single-page PDF with one black line, xref offsets computed properly.
    fn tiny_pdf() -> Vec<u8> {
        let content = "1 w 0 G 10 10 m 180 80 l S\n";
        let objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << >> >>".into(),
            format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offs = vec![];
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for o in &offs { out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes()); }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    #[test]
    fn pdf_page_renders_to_strokes() {
        let o = Opt::default();
        let c = pdf_to_conv(&tiny_pdf(), &o, 150.0).expect("hayro must render the tiny pdf");
        assert_eq!(c.pages, Some(1));
        assert_eq!(c.page, Some(0));
        assert!(!c.gcode.is_empty());
        assert!(c.paths.iter().any(|p| p.len() >= 2), "the drawn line becomes a stroke");
    }

    #[test]
    fn landscape_page_detected() {
        let o = Opt::default();
        let c = pdf_to_conv(&tiny_pdf(), &o, 150.0).unwrap();
        assert!((c.paper[0] - 297.0).abs() < 0.01, "200x100pt page -> landscape A4, got {:?}", c.paper);
    }

    #[test]
    fn page_out_of_range_is_clamped() {
        let o = Opt { page: Some(5), ..Default::default() };
        let c = pdf_to_conv(&tiny_pdf(), &o, 150.0).unwrap();
        assert_eq!(c.page, Some(0));
    }
}
