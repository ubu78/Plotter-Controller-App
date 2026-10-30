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
#[derive(Deserialize)] struct Conv { #[serde(default)] svg: String, #[serde(default)] text: String, #[serde(default)] options: Opt }
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
        ("GET", "fonts") => Ok(json!(text::FONTS)),
        ("GET", "settings") => Ok(json!({"values": g.settings(), "meta": {}, "preset": {"110": 3000, "111": 3000, "120": 100, "121": 100, "32": 0}})),
        ("GET", "selftest_info") => Ok(json!({"version": env!("CARGO_PKG_VERSION"), "fonts": text::FONTS.len(), "platform": std::env::consts::OS})),
        ("POST", "connect") => { let b: Connect = serde_json::from_value(body).map_err(|e| e.to_string())?; g.connect(&b.port, b.baud)?; Ok(json!({"ok": true})) }
        ("POST", "disconnect") => { g.disconnect(); Ok(json!({"ok": true})) }
        ("POST", "send") => { let b: Line = serde_json::from_value(body).map_err(|e| e.to_string())?; g.send(&b.line); Ok(json!({"ok": true})) }
        ("POST", "unlock") => { g.send_now("$X"); Ok(json!({"ok": true})) }
        ("POST", "home") => { g.send_now("$H"); Ok(json!({"ok": true})) }
        ("POST", "stream") => { let b: Stream = serde_json::from_value(body).map_err(|e| e.to_string())?; g.stream(&b.gcode); Ok(json!({"ok": true})) }
        ("POST", "hold") => { g.hold(); Ok(json!({"ok": true})) }
        ("POST", "resume") => { g.resume(); Ok(json!({"ok": true})) }
        ("POST", "feedov") => { match body["kind"].as_str().unwrap_or("") { "plus" => g.feed_ov(0x91), "minus" => g.feed_ov(0x92), _ => g.feed_ov(0x90) }; Ok(json!({"ok": true})) }
        ("POST", "stop") => { g.reset(); Ok(json!({"ok": true})) }
        ("POST", "jogcancel") => { g.jog_cancel(); Ok(json!({"ok": true})) }
        ("POST", "svg") => { let b: Conv = serde_json::from_value(body).map_err(|e| e.to_string())?; svg::svg_to_conv(&b.svg, &b.options).and_then(conv_json) }
        ("POST", "text") => { let b: Conv = serde_json::from_value(body).map_err(|e| e.to_string())?; text::text_to_conv(&b.text, &b.options).and_then(conv_json) }
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
    fn svg_dispatch() {
        let (g, _rx) = Grbl::new();
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='10mm' viewBox='0 0 10 10'><rect width='5' height='5'/></svg>";
        let v = handle(&g, "POST", "svg", json!({"svg": svg, "options": {}})).unwrap();
        assert!(v["gcode"].as_str().unwrap().contains("G1"));
        assert!(v["warnings"].as_array().unwrap().is_empty());
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
        let n = v.as_array().map(|a| a.len());
        assert_eq!(n, Some(14), "fonts response: {v}");
        assert!(v.as_array().unwrap().iter().all(|x| x.is_string()));
    }
}
