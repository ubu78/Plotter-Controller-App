//! Single dispatcher for all UI requests: handle(grbl, method, path, body) -> Result<Value, String>.
//! The transport lives in ui/src/api.js; nothing here knows about the window or sockets.
use crate::gcode::Opt;
use crate::grbl::Grbl;
use crate::{doc, pdf, svg, text};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Deserialize)] struct Connect { port: String, #[serde(default = "baud")] baud: u32 }
fn baud() -> u32 { 115200 }
#[derive(Deserialize)] struct Line { line: String }
#[derive(Deserialize)] struct Stream { gcode: String }
/// 0.5.33: stop/disconnect carry the pen panel's «перо вверх» command — a soft reset or a
/// closed port leaves the servo at rest (pen down) and the lift must run again right after.
#[derive(Deserialize, Default)] struct PenupReq { #[serde(default)] penup: String }
#[derive(Deserialize)] struct Conv { #[serde(default)] svg: String, #[serde(default)] text: String, #[serde(default)] options: Opt }
/// 0.5.27: SVG + Text as one program. `options` = sheet/pen settings; `text_options` = the
/// text panel's own fields (font, size, style, alignment, Start X/Y, page).
#[derive(Deserialize)] struct CombineReq { #[serde(default)] svg: String, #[serde(default)] text: String, #[serde(default)] options: Opt, #[serde(default)] text_options: Opt }
/// 0.5.28 marker colors: `colors` maps part ids ("svg"/"text"/"infill") to marker names;
/// `penup` is the pen-up command from the pen panel (servo M3/M5 or `G0 Z…`), `dwell` the
/// pen pause — the inserted M0 block lifts the pen, waits and holds the job.
#[derive(Deserialize)] struct MarkersReq { gcode: String, #[serde(default)] colors: std::collections::BTreeMap<String, String>, #[serde(default)] penup: String, #[serde(default)] dwell: f64 }
#[derive(Deserialize)] struct FontPrev { #[serde(default)] font: String, #[serde(default)] text: String }
#[derive(Deserialize)] struct PdfReq { data: String, #[serde(default)] options: Value }
#[derive(Deserialize)] struct DocFile { name: String, data: String }
#[derive(Deserialize)] struct SaveFile { name: String, content: String }

fn b64(s: &str) -> Result<Vec<u8>, String> { // tiny base64 decoder (no extra crate)
    let t = |c: u8| match c { b'A'..=b'Z' => Some(c - b'A'), b'a'..=b'z' => Some(c - b'a' + 26), b'0'..=b'9' => Some(c - b'0' + 52), b'+' => Some(62), b'/' => Some(63), _ => None };
    let (mut out, mut acc, mut n) = (vec![], 0u32, 0);
    for c in s.bytes() { if let Some(v) = t(c) { acc = acc << 6 | v as u32; n += 6; if n >= 8 { n -= 8; out.push((acc >> n) as u8); acc &= (1 << n) - 1; } } }
    Ok(out)
}

fn save_dir() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    let dl = home.join("Downloads");
    if dl.is_dir() { dl } else { home }
}

/// Keep only the file name: no path traversal, no directories.
fn sanitize_name(name: &str) -> String {
    let n = name.rsplit(['/', '\\']).next().unwrap_or("drawing.gcode").to_string();
    if n.is_empty() || n == "." || n == ".." { "drawing.gcode".into() } else { n }
}

fn conv_json(c: impl serde::Serialize) -> Result<Value, String> { serde_json::to_value(c).map_err(|e| e.to_string()) }

/// Friendly USB-serial name + "is Arduino" flag, from USB IDs and strings.
/// Arduino LLC/Genuino 0x2341, Arduino.org 0x2a03; CH340/CH9102 clones (most Uno/Nano
/// compatibles) 0x1a86:7523/55d3/55d4. CP210x (0x10c4:ea60) and FTDI (0x0403) get chip names.
fn usb_name(vid: u16, pid: u16, man: &str, prod: &str) -> (String, bool) {
    let (man_l, prod_l) = (man.trim().to_lowercase(), prod.trim().to_lowercase());
    let arduino = man_l.contains("arduino") || prod_l.contains("arduino")
        || matches!(vid, 0x2341 | 0x2a03)
        || matches!((vid, pid), (0x1a86, 0x7523) | (0x1a86, 0x55d3) | (0x1a86, 0x55d4));
    let mut name = prod.trim().to_string(); // vendor strings already have sane casing
    if arduino {
        if name.is_empty() { name = "Arduino".into(); }
        if vid == 0x1a86 && !name.to_lowercase().contains("ch340") { name.push_str(" (CH340)"); }
    } else if name.is_empty() {
        name = if vid == 0x1a86 { "CH340".into() }
            else if (vid, pid) == (0x10c4, 0xea60) { "CP210x".into() }
            else if vid == 0x0403 { "FTDI".into() } else { String::new() };
    }
    (name, arduino)
}

/// Dropdown order: Arduino (0) -> other USB-serial ttyUSB*/ttyACM* (1) -> system ports like ttyS* (2).
fn port_group(path: &str, arduino: bool) -> u8 {
    if arduino { 0 } else if path.contains("/ttyUSB") || path.contains("/ttyACM") { 1 } else { 2 }
}

pub fn handle(g: &Arc<Grbl>, method: &str, path: &str, body: Value) -> Result<Value, String> {
    match (method, path) {
        ("GET", "status") => serde_json::to_value(g.status()).map_err(|e| e.to_string()),
        ("GET", "ports") => {
            let mut ps: Vec<(u8, String, Value)> = serialport::available_ports().unwrap_or_default().into_iter().map(|p| {
                let path = p.port_name.clone();
                let (name, ard) = match &p.port_type {
                    serialport::SerialPortType::UsbPort(u) => usb_name(u.vid, u.pid, u.manufacturer.as_deref().unwrap_or(""), u.product.as_deref().unwrap_or("")),
                    _ => (String::new(), false),
                };
                let g = port_group(&path, ard);
                (g, path.clone(), json!({"path": path, "name": name}))
            }).collect();
            ps.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            Ok(json!(ps.into_iter().map(|x| x.2).collect::<Vec<_>>()))
        }
        // Two groups for the font dropdown: Google fonts (downloaded on first use) and
        // every family installed on the system — with an exact face per available style.
        ("GET", "fonts") => Ok(json!({
            "google": text::FONTS,
            "system": text::system_fonts()
        })),
        ("GET", "settings") => Ok(json!({"values": g.settings(), "meta": {}, "preset": {"110": 3000, "111": 3000, "120": 100, "121": 100, "32": 0}})),
        ("GET", "selftest_info") => Ok(json!({"version": env!("CARGO_PKG_VERSION"), "fonts": text::FONTS.len(), "platform": std::env::consts::OS})),
        ("GET", "version") => Ok(json!({"version": env!("CARGO_PKG_VERSION")})),
        ("POST", "connect") => { let b: Connect = serde_json::from_value(body).map_err(|e| e.to_string())?; g.connect(&b.port, b.baud)?; Ok(json!({"ok": true})) }
        ("POST", "disconnect") => { let b: PenupReq = serde_json::from_value(body).unwrap_or_default(); g.disconnect(&b.penup); Ok(json!({"ok": true})) }
        ("POST", "send") => { let b: Line = serde_json::from_value(body).map_err(|e| e.to_string())?; g.send(&b.line); Ok(json!({"ok": true})) }
        // Safety actions: accept ANY method. A transport-level slip (GET vs POST) must never
        // silently kill the unlock button again — 0.5.9 shipped exactly that bug.
        (_, "unlock") => { g.send_now("$X"); Ok(json!({"ok": true})) }
        (_, "home") => { g.send_now("$H"); Ok(json!({"ok": true})) }
        ("POST", "stream") => { let b: Stream = serde_json::from_value(body).map_err(|e| e.to_string())?; g.stream(&b.gcode); Ok(json!({"ok": true})) }
        ("POST", "hold") => { g.hold(); Ok(json!({"ok": true})) }
        ("POST", "resume") => { g.resume(); Ok(json!({"ok": true})) }
        ("POST", "feedov") => { match body["kind"].as_str().unwrap_or("") { "plus" => g.feed_ov(0x91), "minus" => g.feed_ov(0x92), _ => g.feed_ov(0x90) }; Ok(json!({"ok": true})) }
        ("POST", "stop") => { let b: PenupReq = serde_json::from_value(body).unwrap_or_default(); g.reset(&b.penup); Ok(json!({"ok": true})) }
        // 0.5.34 «Заправить лист» stops its jog through this route; like unlock/home it accepts
        // ANY method — a transport-level slip must never silently keep the sheet moving.
        (_, "jogcancel") => { g.jog_cancel(); Ok(json!({"ok": true})) }
        ("POST", "svg") => { let b: Conv = serde_json::from_value(body).map_err(|e| e.to_string())?; svg::svg_to_conv(&b.svg, &b.options).and_then(conv_json) }
        ("POST", "text") => { let b: Conv = serde_json::from_value(body).map_err(|e| e.to_string())?; text::text_to_conv(&b.text, &b.options).and_then(conv_json) }
        // 0.5.27: both blocks in ONE G-code program — one header, one return home, one print job
        ("POST", "combine") => { let b: CombineReq = serde_json::from_value(body).map_err(|e| e.to_string())?; crate::gcode::combine_to_conv(&b.svg, &b.text, &b.options, &b.text_options).and_then(conv_json) }
        // 0.5.28 marker colors: insert/remove the M0 change-marker pauses in an existing program
        ("POST", "markers") => {
            let b: MarkersReq = serde_json::from_value(body).map_err(|e| e.to_string())?;
            let (gcode, pauses) = crate::gcode::apply_marker_colors(&b.gcode, &b.colors, &b.penup, b.dwell);
            Ok(json!({"gcode": gcode, "pauses": pauses}))
        }
        // live font preview: glyph contours of a sample text for the dropdown preview line
        ("POST", "fontpreview") => {
            let b: FontPrev = serde_json::from_value(body).map_err(|e| e.to_string())?;
            let font = if b.font.is_empty() { "Roboto".to_string() } else { b.font };
            let text = if b.text.trim().is_empty() { "Aa Яя 123".to_string() } else { b.text };
            text::font_preview(&font, &text).and_then(conv_json)
        }
        ("POST", "doc") => {
            let b: DocFile = serde_json::from_value(body).map_err(|e| e.to_string())?;
            let d = b64(&b.data)?; doc::doc_to_text(&b.name, &d).map(|t| json!({"text": t}))
        }
        ("POST", "pdf") => {
            let b: PdfReq = serde_json::from_value(body).map_err(|e| e.to_string())?;
            if b.options["mode"] == "vector" { return Err("Vector mode is not available in the Rust build — use the default trace mode".into()); }
            let dpi = b.options["dpi"].as_f64().unwrap_or(150.0);
            let d = b64(&b.data)?;
            let o: Opt = serde_json::from_value(b.options).map_err(|e| e.to_string())?;
            pdf::pdf_to_conv(&d, &o, dpi).and_then(conv_json)
        }
        ("POST", "savefile") => {
            let b: SaveFile = serde_json::from_value(body).map_err(|e| e.to_string())?;
            let p = save_dir().join(sanitize_name(&b.name));
            std::fs::write(&p, b.content).map_err(|e| e.to_string())?;
            Ok(json!({"path": p.to_string_lossy()}))
        }
        _ => Err(format!("Unknown request: {method} {path}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grbl::Grbl;

    #[test]
    fn status_and_unknown_paths() {
        let (g, _rx) = Grbl::new();
        let v = handle(&g, "GET", "status", json!({})).unwrap();
        assert_eq!(v["connected"], json!(false));
        assert!(handle(&g, "GET", "nope", json!({})).unwrap_err().contains("Unknown"));
        assert!(handle(&g, "POST", "connect", json!({"port": ""})).is_err(), "empty port fails cleanly");
    }

    #[test]
    fn safety_routes_accept_any_method() {
        // 0.5.9 regression: the UI once sent GET /unlock, the route only matched POST and the
        // request died with "Unknown request" — the safety button silently did nothing.
        let (g, _rx) = Grbl::new();
        assert!(handle(&g, "GET", "unlock", json!({})).is_ok(), "GET unlock must route");
        assert!(handle(&g, "POST", "unlock", json!({})).is_ok());
        assert!(handle(&g, "GET", "home", json!({})).is_ok());
        assert!(handle(&g, "POST", "home", json!({})).is_ok());
        assert!(handle(&g, "DELETE", "unlock", json!({})).is_ok(), "any method must route");
    }

    #[test]
    fn jogcancel_routes_for_the_sheet_feed() {
        // 0.5.34: «Заправить лист» stops its jog chain through this route — the realtime
        // 0x85 must be reachable from the UI by ANY method (same safety rule as unlock/home)
        let (g, _rx) = Grbl::new();
        assert!(handle(&g, "POST", "jogcancel", json!({})).is_ok());
        assert!(handle(&g, "GET", "jogcancel", json!({})).is_ok(), "any method must route");
    }

    #[test]
    fn svg_dispatch() {
        let (g, _rx) = Grbl::new();
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='10mm' viewBox='0 0 10 10'><rect width='5' height='5'/></svg>";
        let v = handle(&g, "POST", "svg", json!({"svg": svg, "options": {}})).unwrap();
        assert!(v["gcode"].as_str().unwrap().contains("G1"));
        assert!(v["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn svg_single_mode_routes_through_api() {
        // 0.5.30: svgMode=single — an open stroke collapses to ONE centerline pen-down
        let (g, _rx) = Grbl::new();
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='40mm' viewBox='0 0 40 20'><path d='M5 5 L35 15' fill='none' stroke='#000' stroke-width='2'/></svg>";
        let v = handle(&g, "POST", "svg", json!({"svg": svg, "options": {"svgMode": "single"}})).unwrap();
        let s = v["gcode"].as_str().unwrap();
        assert!(!s.is_empty());
        assert!(!s.contains("G2 ") && !s.contains("G3 "), "skeleton output: {s}");
        assert_eq!(s.matches("M5").count(), 1, "one pen down: {s}");
    }

    #[test]
    fn settings_shape() {
        let (g, _rx) = Grbl::new();
        let v = handle(&g, "GET", "settings", json!({})).unwrap();
        assert!(v["values"].is_object());
        assert_eq!(v["preset"]["110"], json!(3000));
    }

    #[test]
    fn b64_roundtrip() {
        assert_eq!(b64("SGVsbG8=").unwrap(), b"Hello");
        assert_eq!(b64("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn markers_route_patches_the_program() {
        let (g, _rx) = Grbl::new();
        let prog = "G21 G90\n; [PART svg]\nG1 X1 Y1\nM3 S255\n; [PART text]\nG1 X5 Y5";
        let v = handle(&g, "POST", "markers", json!({
            "gcode": prog, "colors": {"svg": "black", "text": "red"}, "penup": "M3 S255", "dwell": 0.15
        })).unwrap();
        assert_eq!(v["pauses"], json!(1));
        let out = v["gcode"].as_str().unwrap();
        assert!(out.contains("M0") && out.contains("[MARKER: red]"));
        // idempotent re-apply keeps one pause; empty palette removes it
        let v2 = handle(&g, "POST", "markers", json!({ "gcode": out, "colors": {}, "penup": "M3 S255", "dwell": 0.15 })).unwrap();
        assert_eq!(v2["pauses"], json!(0));
        assert!(!v2["gcode"].as_str().unwrap().contains("M0"));
    }

    #[test]
    fn usb_names_and_port_order() {
        // Arduino by VID (Uno), by product string, by CH340 clone id
        assert_eq!(usb_name(0x2341, 0x0043, "Arduino (www.arduino.cc)", "Uno"), ("Uno".into(), true));
        assert_eq!(usb_name(0x2a03, 0x0001, "", ""), ("Arduino".into(), true));
        assert_eq!(usb_name(0x1a86, 0x7523, "wch.cn", ""), ("Arduino (CH340)".into(), true));
        assert_eq!(usb_name(0x1a86, 0x7523, "wch.cn", "USB-SERIAL CH340"), ("USB-SERIAL CH340".into(), true));
        assert_eq!(usb_name(0x10c4, 0xea60, "Silicon Labs", "CP2102"), ("CP2102".into(), false));
        assert_eq!(usb_name(0x10c4, 0xea60, "Silicon Labs", ""), ("CP210x".into(), false));
        assert_eq!(usb_name(0x0403, 0x6001, "FTDI", ""), ("FTDI".into(), false));
        assert_eq!(usb_name(0x1234, 0x5678, "", ""), ("".into(), false));
        // order: Arduino first, then ttyUSB/ttyACM, then system ports
        assert_eq!(port_group("/dev/ttyUSB0", true), 0);
        assert_eq!(port_group("/dev/ttyACM1", true), 0);
        assert_eq!(port_group("/dev/ttyUSB0", false), 1);
        assert_eq!(port_group("/dev/ttyACM0", false), 1);
        assert_eq!(port_group("/dev/ttyS0", false), 2);
        assert_eq!(port_group("/dev/pts/3", false), 2);
    }

    #[test]
    fn ports_endpoint_sorted_and_shaped() {
        let (g, _rx) = Grbl::new();
        let v = handle(&g, "GET", "ports", json!({})).unwrap();
        let a = v.as_array().expect("ports array");
        println!("real ports in sandbox: {a:?}");
        let mut prev = (0u8, String::new());
        for p in a {
            assert!(p["path"].is_string(), "each port has a path");
            let g = if p["name"].as_str().unwrap_or("").contains("Arduino") { 0 }
                else { let path = p["path"].as_str().unwrap_or("");
                    if path.contains("/ttyUSB") || path.contains("/ttyACM") { 1 } else { 2 } };
            let path = p["path"].as_str().unwrap_or("").to_string();
            assert!(g > prev.0 || (g == prev.0 && path >= prev.1), "ports must be sorted: {a:?}");
            prev = (g, path);
        }
    }

    #[test]
    fn savefile_name_is_sanitized() {
        assert_eq!(sanitize_name("../../evil.txt"), "evil.txt");
        assert_eq!(sanitize_name("/etc/passwd"), "passwd");
        assert_eq!(sanitize_name(".."), "drawing.gcode");
        assert_eq!(sanitize_name(""), "drawing.gcode");
        assert_eq!(sanitize_name("drawing.gcode"), "drawing.gcode");
    }
}

#[cfg(test)]
mod fonts_test {
    use super::*;
    use crate::grbl::Grbl;

    #[test]
    fn fonts_list_is_complete() {
        let (g, _rx) = Grbl::new();
        let v = handle(&g, "GET", "fonts", json!({})).unwrap();
        let n = v["google"].as_array().map(|a| a.len());
        assert_eq!(n, Some(14), "fonts response: {v}");
        assert!(v["google"].as_array().unwrap().iter().all(|x| x.is_string()));
        let sys = v["system"].as_array().expect("system font array");
        assert!(!sys.is_empty(), "the sandbox has system fonts");
        assert!(sys.iter().all(|f| f["value"].as_str().map_or(false, |s| s.starts_with("sys:"))), "every entry carries a sys: value");
        assert!(sys.iter().all(|f| f["styles"].as_array().map_or(false, |a| !a.is_empty())), "every family lists its styles");
        // DejaVu Sans: regular + bold faces, and the bold face really draws differently
        let dj = sys.iter().find(|f| f["family"] == json!("DejaVu Sans")).expect("DejaVu Sans in the list").clone();
        let reg = dj["value"].as_str().unwrap().to_string();
        let bold = dj["styles"].as_array().unwrap().iter().find(|s| s["key"] == json!("bold"))
            .map(|s| s["value"].as_str().unwrap().to_string());
        let bold = bold.expect("DejaVu Sans must expose a bold face");
        let g1 = handle(&g, "POST", "text", json!({"text": "ШШ", "options": {"font": reg, "size": 10}})).unwrap();
        let g2 = handle(&g, "POST", "text", json!({"text": "ШШ", "options": {"font": bold, "size": 10}})).unwrap();
        assert_ne!(g1["gcode"], g2["gcode"], "the bold face must draw differently");
        // preview route returns the contours of the requested text
        let p = handle(&g, "POST", "fontpreview", json!({"font": reg, "text": "Шрифт"})).unwrap();
        assert!(!p["paths"].as_array().expect("paths array").is_empty(), "preview returns contours: {p}");
        assert!(p["w"].as_f64().unwrap() > 5.0);
    }
}
