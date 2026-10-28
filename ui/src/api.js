// The only transport point. In the desktop window — IPC via window.ipc (no network at all);
// in a regular browser — fallback to fetch + WebSocket (kept for `npm run dev` and future use).
const pend = new Map();
let idc = 1;
const listeners = new Set();

const dispatch = m => {
  if (!m || typeof m !== 'object') return;
  window.__pushed = (window.__pushed || 0) + 1;
  if (m.t === 'rpc') {
    const p = pend.get(m.id);
    if (p) { pend.delete(m.id); m.error ? p.err(m.error) : p.ok(m.data); }
  }
  else if (m.t === 'ev') listeners.forEach(f => { try { f(m.data); } catch (e) { /* listener error must not break the bus */ } });
};
window.__push = dispatch; // replaces the placeholder queue installed by the shell
(window.__pushq || []).forEach(dispatch);
window.__pushq = [];

export const api = (p, b) => {
  if (window.ipc) return new Promise((ok, err) => {
    const id = idc++;
    pend.set(id, { ok, err });
    window.ipc.postMessage(JSON.stringify({ id, path: p, method: b ? 'POST' : 'GET', body: b ?? {} }));
  });
  return fetch('/api/' + p, { method: b ? 'POST' : 'GET', headers: { 'Content-Type': 'application/json' }, body: b ? JSON.stringify(b) : undefined }).then(r => r.json());
};

let ws = null;
const connectWs = () => {
  if (ws || typeof WebSocket === 'undefined') return;
  try { ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`); ws.onmessage = e => dispatch(JSON.parse(e.data)); } catch (e) { /* dev server may be absent */ }
};

export const onEvent = f => { listeners.add(f); if (!window.ipc) connectWs(); return () => listeners.delete(f); };
