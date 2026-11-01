//! GRBL 1.1 link: windowed flow control (character-counting protocol), '?' polling, $N=value capture.
//!
//! 0.5.12: the old strict one-line-in-flight scheme made GRBL plan a single block at a time,
//! so the machine braked at every junction and each line paid a full USB round-trip. Now up to
//! RX_WINDOW bytes of complete lines sit in the board's RX buffer at once — the planner sees
//! 10-20 blocks ahead and chains short segments at full speed (the "program №1 feels faster" fix).
//! Events go through a plain channel Bus (no sockets): Bus.send(json) -> proxy -> window.
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::{thread, time::Duration};

/// Bytes of complete lines allowed in flight. GRBL 1.1 on Arduino has a 128-byte RX buffer;
/// 120 leaves margin for forks with the same budget and for the trailing newline bookkeeping.
pub const RX_WINDOW: usize = 120;

/// Byte-credit window for complete lines (the character-counting protocol from the GRBL wiki).
/// Every sent line deposits `len+1` credits; every ok/error refunds the oldest one (FIFO —
/// GRBL answers lines in order). A line larger than the window is only sent alone on a quiet wire.
#[derive(Default)]
pub struct Window { used: usize, inflight: VecDeque<usize> }
impl Window {
    pub fn can_send(&self, len: usize) -> bool {
        let sz = len + 1;
        if self.used == 0 && self.inflight.is_empty() { return true; } // quiet wire: even oversized lines go
        self.used + sz <= RX_WINDOW
    }
    pub fn push(&mut self, len: usize) { let sz = len + 1; self.used += sz; self.inflight.push_back(sz); }
    /// Refund the oldest in-flight line; false when nothing was in flight (stray ok after a reset).
    pub fn ack(&mut self) -> bool { match self.inflight.pop_front() { Some(sz) => { self.used = self.used.saturating_sub(sz); true } None => false } }
    #[cfg(test)] // test-only diagnostics
    pub fn used(&self) -> usize { self.used }
    pub fn depth(&self) -> usize { self.inflight.len() }
    pub fn clear(&mut self) { self.used = 0; self.inflight.clear(); }
}

#[derive(Serialize, Clone)]
pub struct Status { pub connected: bool, pub state: String, pub wpos: [f64; 3], pub wco: [f64; 3], pub progress: f64, pub ov: f64, pub wcs: String }

/// Event channel: status / log / settings as JSON strings, one consumer (the UI pump in main.rs).
pub struct Bus { tx: Sender<String> }
impl Bus {
    pub fn send(&self, s: String) { let _ = self.tx.send(s); }
}

struct Inner {
    port: Option<Box<dyn serialport::SerialPort>>, queue: VecDeque<String>, win: Window, streaming: bool,
    total: usize, sent: usize, st: Status, settings: BTreeMap<String, String>, gen: u64,
}
pub struct Grbl { inner: Mutex<Inner>, pub bus: Bus }

fn triple(v: &str) -> [f64; 3] { let mut o = [0.0; 3]; for (i, s) in v.split(',').take(3).enumerate() { o[i] = s.parse().unwrap_or(0.0); } o }

impl Grbl {
    pub fn new() -> (Arc<Self>, Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        let st = Status { connected: false, state: "Disconnected".into(), wpos: [0.0; 3], wco: [0.0; 3], progress: 0.0, ov: 100.0, wcs: "G54".into() };
        (Arc::new(Grbl { inner: Mutex::new(Inner { port: None, queue: VecDeque::new(), win: Window::default(), streaming: false, total: 0, sent: 0, st, settings: BTreeMap::new(), gen: 0 }), bus: Bus { tx } }), rx)
    }
    fn log(&self, l: String) { self.bus.send(json!({"type": "log", "line": l}).to_string()); }
    fn emit(&self, i: &Inner) { self.bus.send(json!({"type": "status", "status": i.st}).to_string()); }
    pub fn status(&self) -> Status { self.inner.lock().unwrap().st.clone() }
    pub fn settings(&self) -> BTreeMap<String, String> { self.inner.lock().unwrap().settings.clone() }
    /// Number of lines finished with ok/error — used by the self-test and tests.
    pub fn sent_count(&self) -> usize { self.inner.lock().unwrap().sent }
    /// Lines currently in flight (sent, not yet acked) — diagnostics and tests.
    #[allow(dead_code)] // diagnostics (selftest report)
    pub fn inflight(&self) -> usize { self.inner.lock().unwrap().win.depth() }

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
        self.send("$G"); // read modal state: the [GC:...] reply syncs the active WCS (G54..G59)
        Ok(())
    }
    fn reset_state(i: &mut Inner) { i.queue.clear(); i.win.clear(); i.streaming = false; i.total = 0; i.sent = 0; i.st.progress = 0.0; }
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
        // Board rebooted (welcome banner) or raised an alarm: whatever was in flight is lost and
        // never answered. Drop the window and the queue, otherwise $X/$H from the UI would
        // sit trapped behind dead lines forever ("unlock button stopped working").
        if l.starts_with("Grbl ") || l.starts_with("ALARM:") {
            Self::reset_state(&mut i);
            self.log(format!("-- {l} — queue cleared --"));
            if l.starts_with("ALARM:") { self.log("-- press $X to unlock --".into()); }
        }
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
            i.st.progress = if i.total > 0 { (i.sent as f64 / i.total as f64).min(1.0) } else { 0.0 };
            self.emit(&i); return;
        }
        if let Some(rest) = l.strip_prefix("[GC:") { // modal state report (reply to $G): sync the active WCS
            for tok in rest.trim_end_matches(']').split(' ') {
                if let Some(n) = tok.strip_prefix('G') {
                    if let Ok(num) = n.parse::<u32>() { if (54..=59).contains(&num) { i.st.wcs = format!("G{num}"); self.emit(&i); } }
                }
            }
        }
        if let Some((k, v)) = l.split_once('=') {
            if k.starts_with('$') && k[1..].chars().all(|c| c.is_ascii_digit()) {
                i.settings.insert(k[1..].to_string(), v.to_string());
                self.bus.send(json!({"type": "settings", "values": i.settings}).to_string());
            }
        }
        self.log(format!("< {l}"));
        if l == "ok" || l.starts_with("error:") {
            i.win.ack(); // a stray ok after a reset just refunds nothing
            i.sent += 1;
            self.pump(&mut i);
        }
    }
    /// Fill the board's RX buffer: send queued lines while the byte window allows.
    fn pump(&self, i: &mut Inner) {
        loop {
            let Some(len) = i.queue.front().map(|l| l.len()) else { break };
            if !i.win.can_send(len) { break; }
            let l = i.queue.pop_front().unwrap();
            i.win.push(l.len());
            self.log(format!("> {l}"));
            if let Some(p) = i.port.as_mut() { let _ = p.write_all(format!("{l}\n").as_bytes()); }
        }
        if i.streaming && i.queue.is_empty() && i.win.depth() == 0 { i.streaming = false; self.log("-- done --".into()); }
    }
    pub fn send(&self, line: &str) { let mut i = self.inner.lock().unwrap(); i.queue.push_back(line.to_string()); i.total += 1; self.pump(&mut i); }
    /// System commands ($X unlock, $H homing) must never wait behind a stuck queue:
    /// write them to the wire immediately and account their bytes like any in-flight line.
    pub fn send_now(&self, line: &str) {
        let mut i = self.inner.lock().unwrap();
        if i.port.is_none() { self.log("-- not connected: connect first --".into()); return; }
        self.log(format!("> {line}"));
        i.win.push(line.len());
        if let Some(p) = i.port.as_mut() { let _ = p.write_all(format!("{line}\n").as_bytes()); }
    }
    pub fn stream(&self, gcode: &str) {
        let mut i = self.inner.lock().unwrap();
        let lines: Vec<String> = gcode.lines().map(|l| l.split(';').next().unwrap_or("").trim().to_string()).filter(|l| !l.is_empty()).collect();
        i.total = lines.len(); i.sent = 0; i.streaming = true; i.queue.extend(lines); self.pump(&mut i);
    }
    fn rt(&self, b: &[u8]) { if let Some(p) = self.inner.lock().unwrap().port.as_mut() { let _ = p.write_all(b); } }
    pub fn hold(&self) { self.rt(b"!"); }
    pub fn resume(&self) { self.rt(b"~"); }
    /// Real-time feed override (GRBL 1.1): 0x91 = +10%, 0x92 = -10%, 0x90 = set 100%.
    pub fn feed_ov(&self, b: u8) { self.rt(&[b]); }
    pub fn reset(&self) { { let mut i = self.inner.lock().unwrap(); Self::reset_state(&mut i); } self.rt(&[0x18]); }
    pub fn jog_cancel(&self) { self.inner.lock().unwrap().queue.clear(); self.rt(&[0x85]); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_fills_to_the_cap_and_acks_in_order() {
        let mut w = Window::default();
        // "G0 X1 Y1" = 8 chars -> 9 credits; "G1 X2" = 5 -> 6 credits; total 15
        assert!(w.can_send(8)); w.push(8);
        assert!(w.can_send(5)); w.push(5);
        assert_eq!((w.used(), w.depth()), (15, 2));
        // credits nearly full: only small lines still fit
        assert!(w.can_send(RX_WINDOW - 15 - 1));
        assert!(!w.can_send(RX_WINDOW - 15));
        // FIFO refunds: the FIRST line acks first
        assert!(w.ack());
        assert_eq!((w.used(), w.depth()), (6, 1));
        assert!(w.ack());
        assert_eq!((w.used(), w.depth()), (0, 0));
    }

    #[test]
    fn oversized_line_goes_only_on_a_quiet_wire() {
        let mut w = Window::default();
        let big = RX_WINDOW + 50; // longer than the whole buffer
        assert!(w.can_send(big), "quiet wire accepts any line");
        w.push(big);
        assert!(!w.can_send(3), "nothing fits behind an oversized line");
        assert!(w.ack());
        assert!(w.can_send(3), "after the ack the wire is quiet again");
    }

    #[test]
    fn stray_ok_after_reset_refunds_nothing() {
        let mut w = Window::default();
        assert!(!w.ack(), "ack on an empty window must be a no-op");
        assert_eq!(w.used(), 0);
        w.push(10); w.clear();
        assert!(!w.ack());
        assert_eq!(w.used(), 0, "clear wipes the credits");
    }

    #[test]
    fn many_short_lines_stack_in_the_window() {
        // the whole point of 0.5.12: short drawing lines pile up instead of one-per-round-trip
        let mut w = Window::default();
        let mut n = 0;
        while w.can_send(10) { w.push(10); n += 1; }
        assert!(n >= 9, "G1 X0.000 Y0.000-size lines must stack, got {n}");
    }
}
