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
// Includes a ready-G-code paste round-trip: fill the paste block via a native value setter, click Apply,
// verify the G-code panel received the program (0.5.13).
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
    var px = function (done) { // ready-G-code block: fill the paste field, click Apply, verify the G-code panel, then open a file
      try {
        var pta = document.getElementById('pasteta');
        body.paste = { ta: !!pta };
        if (!pta) return done();
        var psec = pta.closest('section');
        var pab = null;
        if (psec) psec.querySelectorAll('button').forEach(function (x) { if (!pab && /apply|применить|rakenda/i.test(x.textContent)) pab = x; });
        body.paste.apply_btn = !!pab;
        body.paste.file_input = !!(psec && psec.querySelector('input[type=file]'));
        if (!pab) return done();
        var pd = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value');
        pd.set.call(pta, 'G21\\nG90\\nG1 X7 Y7 F1500\\nM5');
        pta.dispatchEvent(new Event('input', { bubbles: true }));
        body.paste.filled = pta.value.indexOf('G1 X7 Y7') >= 0;
        setTimeout(function () {
          pab.click();
          setTimeout(function () {
            var pp = document.getElementById('gcode');
            body.paste.applied = pp ? pp.textContent.indexOf('G1 X7 Y7 F1500') >= 0 : false;
            // file path: synthesize a File into the picker (DataTransfer), fire change -> content must
            // land in the paste field AND in the G-code panel in one step
            try {
              var pf = psec ? psec.querySelector('input[type=file]') : null;
              if (pf && typeof DataTransfer === 'function' && typeof File === 'function') {
                var dt = new DataTransfer();
                dt.items.add(new File(['G21\\nG90\\nG1 X3 Y3 F1200\\nM5'], 'selftest.gcode', { type: 'text/plain' }));
                pf.files = dt.files;
                pf.dispatchEvent(new Event('change', { bubbles: true }));
                body.paste.file_pick = 'dispatched';
                setTimeout(function () {
                  var pp2 = document.getElementById('gcode');
                  body.paste.file_applied = pp2 ? pp2.textContent.indexOf('G1 X3 Y3 F1200') >= 0 : false;
                  var pt2 = document.getElementById('pasteta');
                  body.paste.file_in_field = pt2 ? pt2.value.indexOf('G1 X3 Y3') >= 0 : false;
                  done();
                }, 250);
                return;
              }
              body.paste.file_pick = 'skip';
            } catch (e5) { body.paste.file_pick = 'exc:' + String(e5); }
            done();
          }, 150);
        }, 100);
      } catch (e4) { body.paste = { exc: String(e4) }; done(); }
    };
    var iy = function (done) { // 0.5.15 invert-Y round-trip: load an SVG, convert with the
    // mirror OFF, then ON. Idempotent: the checkbox is first forced to a known state, because
    // localStorage may restore invertY=true from a previous run (that exact state once made
    // the first conversion already mirrored and the test flapped).
      try {
        var fi = document.querySelector('input[accept*=svg]');
        body.invy = { input: !!fi };
        if (!fi || typeof DataTransfer !== 'function' || typeof File !== 'function') { body.invy.skip = 'no-input'; return done(); }
        var sec2 = fi.closest('section');
        var cb = null, cv = null;
        if (sec2) {
          sec2.querySelectorAll('label').forEach(function (l) { var c = l.querySelector('input[type=checkbox]'); if (c && /инвертир|invert|peegeld/i.test(l.textContent)) cb = c; });
          sec2.querySelectorAll('button').forEach(function (x) { if (!cv && /конверт|convert|konverteeri/i.test(x.textContent)) cv = x; });
        }
        body.invy.checkbox = !!cb; body.invy.conv_btn = !!cv;
        if (!cb || !cv) return done();
        var dt = new DataTransfer();
        dt.items.add(new File([\"<svg xmlns='http://www.w3.org/2000/svg' width='40mm' viewBox='0 0 40 20'><path d='M5 5 L35 15'/></svg>\"], 'st.svg', { type: 'image/svg+xml' }));
        fi.files = dt.files;
        fi.dispatchEvent(new Event('change', { bubbles: true }));
        var snap = function () { return (document.getElementById('gcode') || {}).textContent || ''; };
        setTimeout(function () { // the picked SVG auto-converts in whatever state was restored
          body.invy.pre = cb.checked ? 'on' : 'off';
          if (cb.checked) cb.click(); // force the known OFF state
          setTimeout(function () {
            cv.click(); // convert with the mirror OFF -> baseline diagonal (0,0)->(30,10)
            setTimeout(function () {
              var g1 = snap();
              body.invy.base = g1.indexOf('G1 X0.000 Y0.000') >= 0 && g1.indexOf('G1 X30.000 Y10.000') >= 0 && g1.indexOf('G1 X0.000 Y10.000') < 0;
              cb.click(); // mirror ON
              setTimeout(function () {
                cv.click();
                setTimeout(function () {
                  var g2 = snap();
                  body.invy.mirrored = g2.indexOf('G1 X0.000 Y10.000') >= 0 && g2.indexOf('G1 X30.000 Y0.000') >= 0 && g2.indexOf('G1 X30.000 Y10.000') < 0;
                  body.invy.checked = !!cb.checked;
                  done();
                }, 450);
              }, 150);
            }, 450);
          }, 150);
        }, 500);
      } catch (e6) { body.invy = { exc: String(e6) }; done(); }
    };
    var yo = function (done) { // 0.5.18 Y origin: switching the SVG card's select to 'bottom'
    // (machine zero at the sheet bottom, standard CNC) must move the same loaded SVG to the top
    // of the A4 sheet (Y 287..297); then 'top' is restored. Idempotent: the select is forced to
    // 'top' first and the invert-Y checkbox is turned OFF (the iy chain leaves it ON)
      try {
        var fi3 = document.querySelector('input[accept*=svg]');
        body.yorigin = { input: !!fi3 };
        if (!fi3) { body.yorigin.skip = 'no-input'; return done(); }
        var sec4 = fi3.closest('section');
        var ys = null, cv3 = null, cb3 = null;
        if (sec4) {
          sec4.querySelectorAll('select').forEach(function (s) { if (ys) return; var has = false; s.querySelectorAll('option').forEach(function (o) { if (o.value === 'bottom') has = true; }); if (has) ys = s; });
          sec4.querySelectorAll('button').forEach(function (x) { if (!cv3 && /конверт|convert|konverteeri/i.test(x.textContent)) cv3 = x; });
          sec4.querySelectorAll('label').forEach(function (l) { var c = l.querySelector('input[type=checkbox]'); if (c && /инвертир|invert|peegeld/i.test(l.textContent)) cb3 = c; });
        }
        body.yorigin.select = !!ys; body.yorigin.conv_btn = !!cv3;
        if (!ys || !cv3) return done();
        if (cb3 && cb3.checked) cb3.click(); // iy left the mirror ON — turn it off for absolute coords
        var sd4 = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, 'value');
        sd4.set.call(ys, 'top');
        ys.dispatchEvent(new Event('change', { bubbles: true }));
        setTimeout(function () {
          sd4.set.call(ys, 'bottom');
          ys.dispatchEvent(new Event('change', { bubbles: true }));
          setTimeout(function () {
            cv3.click();
            setTimeout(function () {
              var g5 = (document.getElementById('gcode') || {}).textContent || '';
              body.yorigin.bottom = g5.indexOf('Y297.000') >= 0 && g5.indexOf('Y287.000') >= 0 && g5.indexOf('Y10.000') < 0;
              sd4.set.call(ys, 'top');
              ys.dispatchEvent(new Event('change', { bubbles: true }));
              setTimeout(function () {
                cv3.click();
                setTimeout(function () {
                  var g6 = (document.getElementById('gcode') || {}).textContent || '';
                  body.yorigin.restored = g6.indexOf('G1 X0.000 Y0.000') >= 0 && g6.indexOf('Y287.000') < 0;
                  done();
                }, 450);
              }, 150);
            }, 450);
          }, 150);
        }, 150);
      } catch (e9) { body.yorigin = { exc: String(e9) }; done(); }
    };
    var dg = function (done) { // 0.5.21 drag: a synthetic pointer drag on the sheet preview
    // must shift the drawing live and the reconversion must bake the offset into the G-code
    // (the first G1 X grows by ~40px * mm-per-screen-pixel). Verifies the whole loop the user
    // reported broken: canvas pointer events -> onDrop -> offsetX/offsetY -> new gcode.
      try {
        var fi5 = document.querySelector('input[accept*=svg]');
        body.drag = { input: !!fi5 };
        if (!fi5) { body.drag.skip = 'no-input'; return done(); }
        var cv5 = null;
        document.querySelectorAll('canvas').forEach(function (c) { if (!cv5 && c.clientWidth > 50) cv5 = c; });
        body.drag.canvas = !!cv5;
        if (!cv5 || typeof PointerEvent !== 'function') { body.drag.skip = 'no-canvas-or-pe'; return done(); }
        var dt5 = new DataTransfer();
        dt5.items.add(new File([\"<svg xmlns='http://www.w3.org/2000/svg' width='40mm' viewBox='0 0 40 20'><path d='M5 5 L35 15'/></svg>\"], 'dg.svg', { type: 'image/svg+xml' }));
        fi5.files = dt5.files;
        fi5.dispatchEvent(new Event('change', { bubbles: true }));
        setTimeout(function () {
          var gs5 = function () { return (document.getElementById('gcode') || {}).textContent || ''; };
          var g7 = gs5();
          body.drag.baseline = g7.indexOf('G1 X0.000 Y0.000') >= 0;
          if (!body.drag.baseline) { body.drag.no_base = g7.slice(0, 60); return done(); }
          var r5 = cv5.getBoundingClientRect();
          var mk5 = function (t, x, y) { try { cv5.dispatchEvent(new PointerEvent(t, { bubbles: true, cancelable: true, clientX: x, clientY: y, pointerId: 7, pointerType: 'mouse', isPrimary: true, buttons: t === 'pointerup' ? 0 : 1 })); } catch (eM) { body.drag.pe_exc = String(eM); } };
          mk5('pointerdown', r5.left + 60, r5.top + 60);
          // the move and the release are spread over real time: while the board is connected the
          // pen-position pushes keep redrawing the canvas between the pointer events — the old
          // code erased the pending shift on each of those redraws — the user's complaint
          setTimeout(function () {
            mk5('pointermove', r5.left + 100, r5.top + 90);
            body.drag.dispatched = true;
            setTimeout(function () {
              mk5('pointerup', r5.left + 100, r5.top + 90);
              setTimeout(function () {
                var g8 = gs5();
                var mmv = 210 / cv5.clientWidth;
                var fx5 = function (s) { var m = /G1 X([\\d.-]+)/.exec(s); return m ? +m[1] : null; };
                var a5 = fx5(g7), b5 = fx5(g8);
                body.drag.changed = g8 !== g7;
                body.drag.mm = +mmv.toFixed(3);
                body.drag.x_before = a5; body.drag.x_after = b5;
                body.drag.shift_ok = a5 !== null && b5 !== null && Math.abs((b5 - a5) - 40 * mmv) < 2.0;
                done();
              }, 700);
            }, 350);
          }, 350);
        }, 600);
      } catch (eD) { body.drag = { exc: String(eD) }; done(); }
    };
    var sf = function (done) { // 0.5.16 system fonts: the font dropdown must list installed families
    // (sys: options) and picking one must really convert text — no network, the file is read from disk
      try {
        var sel = null;
        document.querySelectorAll('select').forEach(function (s) {
          if (sel) return;
          var has = false;
          s.querySelectorAll('option').forEach(function (o) { if ((o.value || '').indexOf('sys:') === 0) has = true; });
          if (has) sel = s;
        });
        body.sysfont = { select: !!sel };
        if (!sel) return done();
        var opt = null;
        sel.querySelectorAll('option').forEach(function (o) { if (!opt && (o.value || '').indexOf('sys:') === 0 && /dejavu/i.test(o.textContent)) opt = o; });
        if (!opt) sel.querySelectorAll('option').forEach(function (o) { if (!opt && (o.value || '').indexOf('sys:') === 0) opt = o; });
        if (!opt) { body.sysfont.no_sys_option = true; return done(); }
        body.sysfont.family = opt.textContent;
        var sd = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, 'value');
        sd.set.call(sel, opt.value);
        sel.dispatchEvent(new Event('change', { bubbles: true }));
        var ta = null;
        document.querySelectorAll('textarea').forEach(function (x) { if (!ta && x.id !== 'pasteta' && !x.classList.contains('gcodeedit')) ta = x; });
        body.sysfont.textarea = !!ta;
        if (!ta) return done();
        var td = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value');
        td.set.call(ta, 'Тест шрифт 123');
        ta.dispatchEvent(new Event('input', { bubbles: true }));
        var btn = null;
        document.querySelectorAll('button').forEach(function (b) { if (!btn && /Текст → G-code|Text → G-code|Tekst → G-code/i.test(b.textContent)) btn = b; });
        body.sysfont.convert_btn = !!btn;
        if (!btn) return done();
        setTimeout(function () {
          btn.click();
          setTimeout(function () {
            var g1 = (document.getElementById('gcode') || {}).textContent || '';
            body.sysfont.converted = /G1 X[\\d.]+ Y[\\d.]+/.test(g1);
            // 0.5.17: the style select (Regular/Bold/...) switches the exact face file and
            // must change the plotted output; the preview line must show real contours
            var ss = null, bo = null;
            document.querySelectorAll('select').forEach(function (s) {
              if (ss) return;
              var b = null;
              s.querySelectorAll('option').forEach(function (o) { if (!b && /^(Bold|Жирный|Rasvane)$/.test(o.textContent)) b = o; });
              if (b) { ss = s; bo = b; }
            });
            body.sysfont.style_select = !!ss;
            body.sysfont.preview = !!document.querySelector('.fprev path');
            if (ss && bo) {
              var sd2 = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, 'value');
              sd2.set.call(ss, bo.value);
              ss.dispatchEvent(new Event('change', { bubbles: true }));
              setTimeout(function () {
                btn.click();
                setTimeout(function () {
                  var g2 = (document.getElementById('gcode') || {}).textContent || '';
                  body.sysfont.bold_diff = g2.length !== g1.length && /G1 X[\\d.]+ Y[\\d.]+/.test(g2);
                  done();
                }, 1200);
              }, 250);
              return;
            }
            done();
          }, 1200);
        }, 200);
      } catch (e7) { body.sysfont = { exc: String(e7) }; done(); }
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
          if (!ta) return px(function () { iy(function () { yo(function () { dg(function () { sf(function () { ux(function () { rep(body); }); }); }); }); }); });
          var d = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value');
          d.set.call(ta, 'G21\\nG90\\nG0 X10 Y10\\nG1 X50 Y30 F3000\\nG1 X10 Y10\\nM5');
          ta.dispatchEvent(new Event('input', { bubbles: true }));
          gc.typed = ta.value.indexOf('G0 X10') >= 0;
          setTimeout(function () {
            ed.click();
            setTimeout(function () {
              var p2 = document.getElementById('gcode');
              gc.pre_after = p2 ? p2.textContent.indexOf('G0 X10') >= 0 : false;
              px(function () { iy(function () { yo(function () { dg(function () { sf(function () { ux(function () { rep(body); }); }); }); }); }); });
            }, 150);
          }, 150);
        }, 150);
        return;
      }
    } catch (e2) { body.gcheck = { exc: String(e2) }; }
    px(function () { iy(function () { yo(function () { dg(function () { sf(function () { ux(function () { rep(body); }); }); }); }); }); });
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
                        if let Some(d) = ui.get("paste") { v["ui_paste"] = d.clone(); }
                        if let Some(d) = ui.get("invy") { v["ui_invy"] = d.clone(); }
                        if let Some(d) = ui.get("sysfont") { v["ui_sysfont"] = d.clone(); }
                        if let Some(d) = ui.get("yorigin") { v["ui_yorigin"] = d.clone(); }
                        if let Some(d) = ui.get("drag") { v["ui_drag"] = d.clone(); }
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

    // 0.5.16: system font scan — the list must be non-empty and a listed font must convert
    let sysfonts = call(&g, "GET", "fonts", json!({})).ok().and_then(|v| {
        let sys = v["system"].as_array()?.clone();
        let val = (sys.first()?)["value"].as_str()?.to_string();
        call(&g, "POST", "text", json!({"text": "Шрифт системы", "options": {"font": val, "size": 6}}))
            .map(|r| !r["gcode"].as_str().unwrap_or("").is_empty()).ok()
    }).unwrap_or(false);
    r.insert("sysfonts".into(), json!(sysfonts));

    // 0.5.17: style variants + preview — DejaVu Sans must expose Bold (and it must draw
    // differently), fontpreview must return the contours of the requested text
    let (fstyle, fontprev) = (|| -> Option<(bool, bool)> {
        let v = call(&g, "GET", "fonts", json!({})).ok()?;
        let dj = v["system"].as_array()?.iter().find(|f| f["family"] == json!("DejaVu Sans"))?.clone();
        let reg = dj["value"].as_str()?.to_string();
        let bold = dj["styles"].as_array()?.iter().find(|s| s["key"] == json!("bold"))?["value"].as_str()?.to_string();
        let g1 = call(&g, "POST", "text", json!({"text": "ШШ", "options": {"font": reg.clone(), "size": 10}})).ok()?;
        let g2 = call(&g, "POST", "text", json!({"text": "ШШ", "options": {"font": bold, "size": 10}})).ok()?;
        let styled = !g1["gcode"].as_str()?.is_empty() && g1["gcode"] != g2["gcode"];
        let prev = call(&g, "POST", "fontpreview", json!({"font": reg, "text": "Шрифт"})).ok()
            .map(|p| !p["paths"].as_array().map_or(true, |a| a.is_empty())).unwrap_or(false);
        Some((styled, prev))
    })().unwrap_or((false, false));
    r.insert("fstyle".into(), json!(fstyle));
    r.insert("fontpreview".into(), json!(fontprev));

    // 0.5.15: invert-Y mirror — the same diagonal with invertY must travel to the mirrored
    // corner and draw to the flipped end, with the footprint unchanged
    let inv = call(&g, "POST", "svg", json!({"svg": "<svg xmlns='http://www.w3.org/2000/svg' width='40mm' viewBox='0 0 40 20'><path d='M5 5 L35 15'/></svg>", "options": {"invertY": true}}));
    r.insert("invy".into(), json!(inv.as_ref().map(|v| {
        let s = v["gcode"].as_str().unwrap_or("");
        s.contains("G1 X0.000 Y10.000") && s.contains("G1 X30.000 Y0.000") && !s.contains("G1 X30.000 Y10.000")
    }).unwrap_or(false)));

    // 0.5.18: Y origin — the machine zero at the BOTTOM edge of the sheet (standard CNC) must
    // move the whole drawing to the top of the A4 sheet (Y ~287..297); the reverse mode negates
    // Y for feed machines that scroll the sheet backwards
    let ybot = call(&g, "POST", "svg", json!({"svg": "<svg xmlns='http://www.w3.org/2000/svg' width='40mm' viewBox='0 0 40 20'><path d='M5 5 L35 15'/></svg>", "options": {"yOrigin": "bottom"}}));
    let yrev = call(&g, "POST", "svg", json!({"svg": "<svg xmlns='http://www.w3.org/2000/svg' width='40mm' viewBox='0 0 40 20'><path d='M5 5 L35 15'/></svg>", "options": {"yOrigin": "reverse"}}));
    let b = ybot.as_ref().map(|v| { let s = v["gcode"].as_str().unwrap_or("");
        s.contains("G1 X0.000 Y297.000") && s.contains("G1 X30.000 Y287.000") && !s.contains("G1 X30.000 Y10.000") }).unwrap_or(false);
    let rv = yrev.as_ref().map(|v| { let s = v["gcode"].as_str().unwrap_or("");
        s.contains("G1 X0.000 Y0.000") && s.contains("G1 X30.000 Y-10.000") }).unwrap_or(false);
    r.insert("yorigin".into(), json!(b && rv));

    let servo = text_r.as_ref().map(|v| { let s = v["gcode"].as_str().unwrap_or("");
        // 0.5.19 convention: M3 S255 lifts (first), M5 lowers before drawing
        s.contains("$32=0") && s.contains("G4 P")
            && s.find("M3 S255").zip(s.find("\nM5")).map_or(false, |(u, d)| u < d) }).unwrap_or(false);
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
