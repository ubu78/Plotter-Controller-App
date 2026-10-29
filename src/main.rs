//! Desktop shell: tao window + wry WebView (WebKitGTK), custom scheme plotter://app/, IPC.
//! No browser, no HTTP server, no open ports. Assets are embedded into the binary (rust-embed).
mod api; mod doc; mod gcode; mod grbl; mod pdf; mod skeleton; mod svg; mod text;

use grbl::Grbl;
use rust_embed::RustEmbed;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::unix::WindowExtUnix;
use tao::window::WindowBuilder;
use wry::{WebContext, WebViewBuilder, WebViewBuilderExtUnix};

#[derive(RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

const INDEX: &str = "plotter://app/index.html";
// Placeholder queue: events that arrive before the UI bundle defines its own window.__push.
const INIT_JS: &str = "window.__pushq = window.__pushq || []; window.__push = function (m) { window.__pushq.push(m); };";
// Self-test: report DOM diagnostics back through the normal IPC channel (callbacks are unreliable here).
const SELFTEST_INIT_JS: &str = "setTimeout(function () { try { var s = []; document.querySelectorAll('select').forEach(function (e) { s.push(e.options.length); }); var b = document.querySelector('.connstat'); window.ipc.postMessage(JSON.stringify({ id: 888888, path: 'selftest_report', method: 'POST', body: { pushed: window.__pushed || 0, sel: s, banner: b ? { cls: b.className, txt: b.textContent } : null } })); } catch (e) { if (window.ipc) window.ipc.postMessage(JSON.stringify({ id: 888888, path: 'selftest_report', method: 'POST', body: { exc: String(e) } })); } }, 6000);";

fn data_dir() -> PathBuf { // keeps localStorage (lang, pen settings)
    let base = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/share")
    });
    base.join("plotter-controller")
}

fn main() {
    let event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let (g, rx) = Grbl::new();

    // Bus -> window: every GRBL event becomes a user event and reaches JS via window.__push.
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || for m in rx { if proxy.send_event(m).is_err() { break; } });
    }

    let win_w = std::env::var("PLOTTER_WIN_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1440.0);
    let win_h = std::env::var("PLOTTER_WIN_H").ok().and_then(|v| v.parse().ok()).unwrap_or(950.0);
    let window = WindowBuilder::new()
        .with_title("Plotter Controller")
        .with_inner_size(LogicalSize::new(win_w, win_h))
        .build(&event_loop)
        .expect("cannot create window");

    let mut ctx = WebContext::new(Some(data_dir()));
    let ipc_proxy = proxy.clone();
    let ipc_g = g.clone();
    let selftest_mode = std::env::var("PLOTTER_SELFTEST").as_deref() == Ok("1");
    let mut builder = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(INDEX)
        .with_initialization_script(INIT_JS);
    if selftest_mode { builder = builder.with_initialization_script(SELFTEST_INIT_JS); }
    let webview = builder
        .with_custom_protocol("plotter".into(), move |_, req| {
            let p = req.uri().path().trim_start_matches('/');
            let p = if p.is_empty() { "index.html" } else { p };
            match Assets::get(p).or_else(|| Assets::get("index.html")) {
                Some(f) => wry::http::Response::builder()
                    .header("Content-Type", f.metadata.mimetype().to_string())
                    .header("Cache-Control", "no-cache")
                    .body(f.data)
                    .unwrap(),
                None => wry::http::Response::builder().status(404).body(Vec::new().into()).unwrap(),
            }
        })
        .with_ipc_handler(move |req| {
            let msg = req.body().clone();
            if std::env::var("PLOTTER_IPC_DEBUG").as_deref() == Ok("1") { eprintln!("IPC-RECV: {}", &msg[..msg.len().min(120)]); }
            let proxy = ipc_proxy.clone();
            let g = ipc_g.clone();
            // conversions and font downloads are slow: never block the UI thread
            std::thread::spawn(move || {
                let v: Value = match serde_json::from_str(&msg) { Ok(v) => v, Err(_) => return };
                let id = v["id"].as_u64().unwrap_or(0);
                let path = v["path"].as_str().unwrap_or("").to_string();
                let method = v["method"].as_str().unwrap_or("GET").to_string();
                if path == "quit" { // special request: disconnect the port and exit
                    g.disconnect();
                    std::thread::sleep(Duration::from_millis(150));
                    std::process::exit(0);
                }
                if path == "selftest_report" { // diagnostics from the page -> final report
                    let body = v.get("body").cloned().unwrap_or(json!({}));
                    let _ = proxy.send_event(format!("SELFTEST_UI {body}"));
                    return;
                }
                let body = v.get("body").cloned().unwrap_or(Value::Null);
                let resp = match api::handle(&g, &method, &path, body) {
                    Ok(d) => json!({"t": "rpc", "id": id, "data": d}),
                    Err(e) => json!({"t": "rpc", "id": id, "error": e}),
                };
                let _ = proxy.send_event(resp.to_string());
            });
        })
        .build_gtk(window.default_vbox().expect("no gtk vbox"))
        .expect("cannot create webview");

    if std::env::var("PLOTTER_SELFTEST").as_deref() == Ok("1") {
        let p = proxy.clone();
        let g = g.clone();
        std::thread::spawn(move || selftest(g, p));
    }

    let (mut rep_rust, mut rep_ui) = (None::<String>, None::<Value>);
    event_loop.run(move |ev, _, cf| {
        *cf = ControlFlow::Wait;
        match ev {
            Event::UserEvent(s) => {
                if let Some(rest) = s.strip_prefix("SELFTEST ") {
                    rep_rust = Some(rest.to_string());
                } else if let Some(rest) = s.strip_prefix("SELFTEST_UI ") {
                    rep_ui = serde_json::from_str(rest.trim()).ok();
                } else if s == "SELFTEST_TIMEOUT" {
                    if rep_ui.is_none() { rep_ui = Some(json!({"pushed": -1})); }
                    if rep_rust.is_none() { rep_rust = Some("{}".into()); }
                } else {
                    let _ = webview.evaluate_script(&format!("window.__push({s});"));
                }
                // print the report as soon as both halves have arrived
                if let (Some(rr), Some(ui)) = (&rep_rust, &rep_ui) {
                    if rep_rust.is_some() && rep_ui.is_some() {
                        let mut v: Value = serde_json::from_str(rr).unwrap_or(json!({}));
                        v["pushes"] = ui.get("pushed").cloned().unwrap_or(json!(-1));
                        v["ui_selects"] = ui.get("sel").cloned().unwrap_or(json!(null));
                        if let Some(b) = ui.get("banner") { v["ui_banner"] = b.clone(); }
                        if let Some(d) = ui.get("dbg") { v["ipc_dbg"] = d.clone(); }
                        if let Some(e) = ui.get("exc") { v["ui_exc"] = e.clone(); }
                        println!("SELFTEST {v}");
                        g.disconnect();
                        *cf = ControlFlow::Exit;
                        rep_rust = None; // prevent double print
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                g.disconnect();
                *cf = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

/// Self-test in the real window: converters, storage, connection to a mock board (pty), event pushes.
/// Run: PLOTTER_SELFTEST=1 PLOTTER_SELFTEST_PORT=/dev/pts/N xvfb-run ./plotter-controller
fn selftest(g: Arc<Grbl>, proxy: EventLoopProxy<String>) {
    std::thread::sleep(Duration::from_millis(2500)); // let the page load and subscribe first
    let mut r = serde_json::Map::new();
    let call = |g: &Arc<Grbl>, method: &str, path: &str, body: Value| api::handle(g, method, path, body);

    r.insert("status".into(), json!(call(&g, "GET", "status", json!({})).map(|v| v["connected"] == json!(false)).unwrap_or(false)));

    let src = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100mm\" viewBox=\"0 0 100 100\"><rect x=\"10\" y=\"10\" width=\"50\" height=\"30\"/><path d=\"M10 80 A20 20 0 0 1 50 80Z\"/></svg>";
    let svg_r = call(&g, "POST", "svg", json!({"svg": src, "options": {}}));
    r.insert("svg".into(), json!(svg_r.as_ref().map(|v| !v["gcode"].as_str().unwrap_or("").is_empty() && v["bbox"].is_array()).unwrap_or(false)));

    let text_r = call(&g, "POST", "text", json!({"text": "Привет, selftest", "options": {"size": 6}}));
    r.insert("text".into(), json!(text_r.as_ref().map(|v| !v["gcode"].as_str().unwrap_or("").is_empty()).unwrap_or(false)));

    let servo = text_r.as_ref().map(|v| { let s = v["gcode"].as_str().unwrap_or("");
        s.contains("M5") && s.contains("M3 S255") && s.contains("G4 P") && s.contains("$32=0") }).unwrap_or(false);
    r.insert("servo".into(), json!(servo));

    let storage = call(&g, "POST", "savefile", json!({"name": "plotter-selftest.txt", "content": "ok"})).ok()
        .and_then(|v| v["path"].as_str().map(String::from))
        .map(|p| { let ok = PathBuf::from(&p).exists(); let _ = std::fs::remove_file(&p); ok })
        .unwrap_or(false);
    r.insert("storage".into(), json!(storage));

    let port = std::env::var("PLOTTER_SELFTEST_PORT").unwrap_or_default();
    let conn = !port.is_empty() && call(&g, "POST", "connect", json!({"port": port, "baud": 115200})).is_ok();
    r.insert("connect".into(), json!(conn));

    let mut after = false;
    if conn {
        let _ = call(&g, "POST", "send", json!({"line": "G0 X5 Y5 F1000"}));
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            if g.sent_count() > 0 { after = true; break; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    r.insert("afterConnect".into(), json!(after));

    // feed override round-trip: two F+ clicks -> the mock board reports Ov:120 in the status line
    let mut feedov = false;
    if conn {
        let _ = call(&g, "POST", "feedov", json!({"kind": "plus"}));
        let _ = call(&g, "POST", "feedov", json!({"kind": "plus"}));
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            if g.status().ov == 120.0 { feedov = true; break; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    r.insert("feedov".into(), json!(feedov));

    let _ = proxy.send_event(format!("SELFTEST {}", Value::Object(r)));
    // watchdog: if the page never reports, finish anyway
    std::thread::sleep(Duration::from_secs(15));
    let _ = proxy.send_event("SELFTEST_TIMEOUT".into());
}
