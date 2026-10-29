//! GRBL 1.1 link: strict flow control (one line in flight until ok/error), '?' polling, $N=value capture.
//! Events go through a plain channel Bus (no sockets): Bus.send(json) -> proxy -> window.
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::{thread, time::Duration};

#[derive(Serialize, Clone)]
pub struct Status { pub connected: bool, pub state: String, pub wpos: [f64; 3], pub wco: [f64; 3], pub progress: f64, pub ov: f64 }

/// Event channel: status / log / settings as JSON strings, one consumer (the UI pump in main.rs).
pub struct Bus { tx: Sender<String> }
impl Bus {
    pub fn send(&self, s: String) { let _ = self.tx.send(s); }
}

struct Inner {
    port: Option<Box<dyn serialport::SerialPort>>, queue: VecDeque<String>, busy: bool, streaming: bool,
    total: usize, sent: usize, st: Status, settings: BTreeMap<String, String>, gen: u64,
}
pub struct Grbl { inner: Mutex<Inner>, pub bus: Bus }

fn triple(v: &str) -> [f64; 3] { let mut o = [0.0; 3]; for (i, s) in v.split(',').take(3).enumerate() { o[i] = s.parse().unwrap_or(0.0); } o }

impl Grbl {
    pub fn new() -> (Arc<Self>, Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        let st = Status { connected: false, state: "Disconnected".into(), wpos: [0.0; 3], wco: [0.0; 3], progress: 0.0, ov: 100.0 };
        (Arc::new(Grbl { inner: Mutex::new(Inner { port: None, queue: VecDeque::new(), busy: false, streaming: false, total: 0, sent: 0, st, settings: BTreeMap::new(), gen: 0 }), bus: Bus { tx } }), rx)
    }
    fn log(&self, l: String) { self.bus.send(json!({"type": "log", "line": l}).to_string()); }
    fn emit(&self, i: &Inner) { self.bus.send(json!({"type": "status", "status": i.st}).to_string()); }
    pub fn status(&self) -> Status { self.inner.lock().unwrap().st.clone() }
    pub fn settings(&self) -> BTreeMap<String, String> { self.inner.lock().unwrap().settings.clone() }
    /// Number of lines finished with ok/error — used by the self-test and tests.
    pub fn sent_count(&self) -> usize { self.inner.lock().unwrap().sent }

    pub fn connect(self: &Arc<Self>, path: &str, baud: u32) -> Result<(), String> {
        let mut i = self.inner.lock().unwrap();
        if i.port.is_some() { return Err("Already connected".into()); }
        let port = serialport::new(path, baud).timeout(Duration::from_millis(100)).open().map_err(|e| e.to_string())?;
        let mut rd = port.try_clone().map_err(|e| e.to_string())?;
        i.port = Some(port); i.gen += 1; let g = i.gen; i.st.connected = true; i.st.state = "Connecting".into(); self.emit(&i); drop(i);
        let me = self.clone();
        thread::spawn(move || { // reader
            let (mut buf, mut b) = (Vec::new(), [0u8; 256]);
            loop {
                if me.inner.lock().unwrap().gen != g { break; }
                match rd.read(&mut b) {
                    Ok(n) => for &c in &b[..n] { if c == b'\n' { let l = String::from_utf8_lossy(&buf).trim().to_string(); buf.clear(); me.line(l); } else if c != b'\r' { buf.push(c); } },
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => { me.closed(g); break; }
                }
            }
        });
        let me = self.clone();
        thread::spawn(move || loop { // status poll
            thread::sleep(Duration::from_millis(250));
            let mut i = me.inner.lock().unwrap();
            if i.gen != g { break; }
            match i.port.as_mut() { Some(p) => { let _ = p.write_all(b"?"); } None => break }
        });
        Ok(())
    }
    fn reset_state(i: &mut Inner) { i.queue.clear(); i.busy = false; i.streaming = false; i.total = 0; i.sent = 0; i.st.progress = 0.0; }
    fn closed(&self, g: u64) {
        let mut i = self.inner.lock().unwrap();
        if i.gen == g { i.port = None; Self::reset_state(&mut i); i.st.connected = false; i.st.state = "Disconnected".into(); self.emit(&i); }
    }
    pub fn disconnect(&self) {
        let mut i = self.inner.lock().unwrap(); i.port = None; i.gen += 1; Self::reset_state(&mut i);
        i.st.connected = false; i.st.state = "Disconnected".into(); self.emit(&i);
    }
    fn line(&self, l: String) {
        if l.is_empty() { return; }
        let mut i = self.inner.lock().unwrap();
        if l.starts_with('<') {
            let f: Vec<&str> = l.trim_matches(|c| c == '<' || c == '>').split('|').collect();
            i.st.state = f[0].split(':').next().unwrap_or("").to_string();
            for x in &f[1..] {
                if let Some((k, v)) = x.split_once(':') {
                    match k { "WCO" => i.st.wco = triple(v), "WPos" => i.st.wpos = triple(v),
                        "MPos" => { let m = triple(v); for n in 0..3 { i.st.wpos[n] = m[n] - i.st.wco[n]; } }
                        "Ov" => i.st.ov = triple(v)[0], _ => {} }
                }
            }
            i.st.progress = if i.total > 0 { i.sent as f64 / i.total as f64 } else { 0.0 };
            self.emit(&i); return;
        }
        if let Some((k, v)) = l.split_once('=') {
            if k.starts_with('$') && k[1..].chars().all(|c| c.is_ascii_digit()) {
                i.settings.insert(k[1..].to_string(), v.to_string());
                self.bus.send(json!({"type": "settings", "values": i.settings}).to_string());
            }
        }
        self.log(format!("< {l}"));
        if l == "ok" || l.starts_with("error:") { i.busy = false; i.sent += 1; self.next(&mut i); }
    }
    fn next(&self, i: &mut Inner) {
        if i.busy || i.port.is_none() { return; }
        match i.queue.pop_front() {
            None => if i.streaming { i.streaming = false; self.log("-- done --".into()); },
            Some(l) => { i.busy = true; self.log(format!("> {l}")); if let Some(p) = i.port.as_mut() { let _ = p.write_all(format!("{l}\n").as_bytes()); } }
        }
    }
    pub fn send(&self, line: &str) { let mut i = self.inner.lock().unwrap(); i.queue.push_back(line.to_string()); i.total += 1; self.next(&mut i); }
    pub fn stream(&self, gcode: &str) {
        let mut i = self.inner.lock().unwrap();
        let lines: Vec<String> = gcode.lines().map(|l| l.split(';').next().unwrap_or("").trim().to_string()).filter(|l| !l.is_empty()).collect();
        i.total = lines.len(); i.sent = 0; i.streaming = true; i.queue.extend(lines); self.next(&mut i);
    }
    fn rt(&self, b: &[u8]) { if let Some(p) = self.inner.lock().unwrap().port.as_mut() { let _ = p.write_all(b); } }
    pub fn hold(&self) { self.rt(b"!"); }
    pub fn resume(&self) { self.rt(b"~"); }
    /// Real-time feed override (GRBL 1.1): 0x91 = +10%, 0x92 = -10%, 0x90 = set 100%.
    pub fn feed_ov(&self, b: u8) { self.rt(&[b]); }
    pub fn reset(&self) { { let mut i = self.inner.lock().unwrap(); Self::reset_state(&mut i); } self.rt(&[0x18]); }
    pub fn jog_cancel(&self) { self.inner.lock().unwrap().queue.clear(); self.rt(&[0x85]); }
}
