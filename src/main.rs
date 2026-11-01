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
// Includes a G-code editor round-trip: click Edit, type via a native value setter, click Done, verify the preview.
// Includes an unlock-button round-trip through the REAL transport: click the actual $X button,
// then count '> $X' lines in the console log. 0.5.9 shipped a GET/POST route mismatch that made
// the button silently do nothing while the Rust-side POST selftest stayed green — this UI half
// exists so that class of bug can never pass again.
const SELFTEST_INIT_JS: &str = "setTimeout(function () {
  var rep = function (body) { try { window.ipc.postMessage(JSON.stringify({ id: 888888, path: 'selftest_report', method: 'POST', body: body })); } catch (e) {} };
  try {
    var s = []; document.querySelectorAll('select').forEach(function (e) { s.push(e.options.length); });
    var b = document.querySelector('.connstat');
    var body = { pushed: window.__pushed || 0, sel: s, banner: b ? { cls: b.className, txt: b.textContent } : null };
    var ux = function (done) { // click the real $X button, verify a NEW '> $X' line reached the log
      try {
        var xb = null;
        document.querySelectorAll('button[title]').forEach(function (x) { if (!xb && x.title.indexOf('$X') >= 0) xb = x; });
        body.unlock_btn = { found: !!xb };
        body.stop_btn = !!document.querySelector('.runrow .stop');
        if (!xb) return done();
        var cnt = function (t) { var n = 0, i = 0; while ((i = t.indexOf('> $X', i)) >= 0) { n++; i += 4; } return n; };
        var lg = document.getElementById('log');
        var before = cnt(lg ? lg.textContent : '');
        xb.click();
        setTimeout(function () {
          var lg2 = document.getElementById('log');
          body.unlock_btn.sent = cnt(lg2 ? lg2.textContent : '') > before;
          done();
        }, 400);
      } catch (e3) { body.unlock_btn = { exc: String(e3) }; done(); }
    };
    try {
      var gc = {};
      gc.load_input = !!document.querySelector('input[accept*=gcode]');
      var pre = document.getElementById('gcode');
      var sec = pre ? pre.closest('section') : null;
      var ed = null;
      if (sec) sec.querySelectorAll('button').forEach(function (x) { if (!ed && /edit|редактир|muuda/i.test(x.textContent)) ed = x; });
      gc.edit_btn = !!ed;
      body.gcheck = gc;
      if (ed) {
        ed.click();
        setTimeout(function () {
          var ta = sec.querySelector('textarea.gcodeedit');
          gc.textarea = !!ta;
          if (!ta) return ux(function () { rep(body); });
          var d = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value');
          d.set.call(ta, 'G21\\nG90\\nG0 X10 Y10\\nG1 X50 Y30 F3000\\nG1 X10 Y10\\nM5');
          ta.dispatchEvent(new Event('input', { bubbles: true }));
          gc.typed = ta.value.indexOf('G0 X10') >= 0;
          setTimeout(function () {
            ed.click();
            setTimeout(function () {
              var p2 = document.getElementById('gcode');
              gc.pre_after = p2 ? p2.textContent.indexOf('G0 X10') >= 0 : false;
              ux(function () { rep(body); });
            }, 150);
          }, 150);
        }, 150);
        return;
      }
    } catch (e2) { body.gcheck = { exc: String(e2) }; }
    ux(function () { rep(body); });
  } catch (e) { rep({ exc: String(e) }); }
}, 6000);";

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
                        if let Some(d) = ui.get("gcheck") { v["ui_gcheck"] = d.clone(); }
                        if let Some(d) = ui.get("unlock_btn") { v["ui_unlock_btn"] = d.clone(); }
                        if let Some(d) = ui.get("stop_btn") { v["ui_stop_btn"] = d.clone(); }
                        if let Some(d) = ui.get("dbg") { v["ipc_dbg"] = d.clone(); }
                        if let Some(e) = ui.get("exc") { v["ui_exc"] = e.clone(); }
                        println!("SELFTEST {v}");
                        g.disconnect();
                        // PLOTTER_SELFTEST_HOLD=<secs>: keep the window on screen for screenshots
                        if let Ok(h) = std::env::var("PLOTTER_SELFTEST_HOLD") {
                            if let Ok(n) = h.parse::<u64>() { std::thread::sleep(Duration::from_secs(n)); }
                        }
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

    // Unlock round-trip: raise an alarm on the mock board (a real GRBL leaves the in-flight
    // line unanswered, wedging the queue). $X must go straight to the wire and the board
    // must return to Idle. Runs early on purpose: it must be finished before the UI half of
    // the selftest clicks the real $X button at t=6s (see SELFTEST_INIT_JS).
    let mut unlock = false;
    if conn {
        let _ = call(&g, "POST", "send", json!({"line": "$ALARM"}));
        let t0 = std::time::Instant::now();
        let mut alarm_seen = false;
        while t0.elapsed() < Duration::from_secs(3) {
            if g.status().state == "Alarm" { alarm_seen = true; break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        if alarm_seen {
            let _ = call(&g, "POST", "send", json!({"line": "G0 X1 Y1"})); // locked board: never answered
            let _ = call(&g, "POST", "unlock", json!({}));
            let t0 = std::time::Instant::now();
            while t0.elapsed() < Duration::from_secs(3) {
                if g.status().state == "Idle" { unlock = true; break; }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    r.insert("unlock".into(), json!(unlock));

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

    // Stop print round-trip: stream a short program (the mock drains it, progress freezes at 100%),
    // then POST stop -> 0x18 -> the board reboots, the welcome banner clears queue and counters.
    // progress back to 0 while staying connected proves the whole reset path actually ran.
    let mut stop = false;
    if conn {
        let _ = call(&g, "POST", "stream", json!({"gcode": "G0 X1 Y1\nG0 X2 Y2\nG0 X3 Y3"}));
        let t0 = std::time::Instant::now();
        let mut progressed = false;
        while t0.elapsed() < Duration::from_secs(3) {
            if g.status().progress > 0.0 { progressed = true; break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = call(&g, "POST", "stop", json!({}));
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            let s = g.status();
            if s.connected && s.state == "Idle" && s.progress == 0.0 { stop = true; break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        stop = stop && progressed;
    }
    r.insert("stop".into(), json!(stop));

    // WCS round-trip: select G55 -> the $G modal report must reflect it
    let mut wcs = false;
    if conn {
        let _ = call(&g, "POST", "send", json!({"line": "G55"}));
        let _ = call(&g, "POST", "send", json!({"line": "$G"}));
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            if g.status().wcs == "G55" { wcs = true; break; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    r.insert("wcs".into(), json!(wcs));

    let _ = proxy.send_event(format!("SELFTEST {}", Value::Object(r)));
    // watchdog: if the page never reports, finish anyway
    std::thread::sleep(Duration::from_secs(15));
    let _ = proxy.send_event("SELFTEST_TIMEOUT".into());
}
