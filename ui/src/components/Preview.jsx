import { useEffect, useRef } from 'react';
// yOrigin: which machine Y convention the emitted G-code uses. The preview shows the SHEET as
// it will look on paper, so machine coordinates are mapped back: 'bottom' (zero at the bottom
// edge, Y up) mirrors around the sheet height, 'reverse' (Y = -Y) around the top edge. The live
// pen position arrives in the same machine coordinates and is mapped identically.
// 0.5.21: while the board is connected the pen position arrives several times a second and every
// redraw used to paint the UNshifted picture — a drag visually snapped back while the button was
// still held, so the sheet felt immovable ("не переставить мышкой"). The pending shift now lives
// in a ref: pen-position redraws keep it, and a new conversion (whose offset is already baked
// into the coordinates) or a yOrigin switch clears it.
// 0.5.22: the preview draws the SHEET itself — a 10 mm grid (heavier every 50 mm), the sheet
// frame for the selected paper format (the `sheet` prop is used until a conversion provides its
// own paper) and a dashed rectangle of the printable area inside the four margins
// (`margins` = [left, right, top, bottom] mm). Grid, frame and margins live in sheet space
// (origin top-left); only the paths and the live pen dot follow the yOrigin machine mapping.
// `sheet` and `margins` must be memoized by the caller: a fresh array each render would reset
// the pending drag shift through the [conv, yOrigin, sheet, margins] effect below.
export default function Preview({ conv, pos, onDrop, pick, onPick, yOrigin = 'top', sheet = [210, 297], margins = [0, 0, 0, 0] }) {
  const cv = useRef(); const drag = useRef(null); const sh = useRef([0, 0]); const drawRef = useRef(null);
  const draw = (s = sh.current) => {
    const c = cv.current; if (!c) return;
    const x = c.getContext('2d'), [pw, ph] = conv ? conv.paper : sheet;
    // 0.5.24: the preview card is a large 2-column grid area — the canvas buffer follows the
    // real card width (crisp at any size) instead of the old fixed 420 px buffer.
    const cw = Math.max(320, Math.round(c.clientWidth || 420)), k = cw / pw;
    const fy = yOrigin === 'bottom' ? (b => ph - b) : yOrigin === 'reverse' ? (b => -b) : (b => b);
    c.width = cw; c.height = ph * k; x.fillStyle = '#fff'; x.fillRect(0, 0, c.width, c.height);
    x.lineWidth = 1;
    x.strokeStyle = '#e7ebf0'; // 10 mm grid
    for (let g = 10; g < pw; g += 10) { x.beginPath(); x.moveTo(g * k, 0); x.lineTo(g * k, ph * k); x.stroke(); }
    for (let g = 10; g < ph; g += 10) { x.beginPath(); x.moveTo(0, g * k); x.lineTo(pw * k, g * k); x.stroke(); }
    x.strokeStyle = '#cdd5df'; // every 50 mm stands out
    for (let g = 50; g < pw; g += 50) { x.beginPath(); x.moveTo(g * k, 0); x.lineTo(g * k, ph * k); x.stroke(); }
    for (let g = 50; g < ph; g += 50) { x.beginPath(); x.moveTo(0, g * k); x.lineTo(pw * k, g * k); x.stroke(); }
    const [ml, mr, mt, mb] = margins; // printable area inside the four margins
    if (ml + mr + mt + mb > 0.01) {
      x.setLineDash([5, 4]); x.strokeStyle = '#e08a00';
      x.strokeRect(ml * k, mt * k, Math.max(1, (pw - ml - mr) * k), Math.max(1, (ph - mt - mb) * k));
      x.setLineDash([]);
    }
    x.strokeStyle = '#1f6feb';
    (conv?.paths || []).forEach(p => { x.beginPath(); p.forEach(([a, b], i) => x[i ? 'lineTo' : 'moveTo']((a + s[0]) * k, fy(b + s[1]) * k)); x.stroke(); });
    if (pos) { x.fillStyle = '#c62828'; x.beginPath(); x.arc(pos[0] * k, fy(pos[1]) * k, 4, 0, 7); x.fill(); }
    x.strokeStyle = '#7d8791'; x.lineWidth = 2; x.strokeRect(1, 1, pw * k - 2, ph * k - 2); // the sheet edge
  };
  drawRef.current = draw; // latest draw for the resize observer below
  useEffect(() => { sh.current = [0, 0]; draw(); }, [conv, yOrigin, sheet, margins]);
  useEffect(() => { if (!drag.current) draw(); }, [pos]); // the live pen dot must not erase a running drag
  useEffect(() => { // 0.5.24: the big preview card resizes with the window — repaint to stay crisp
    if (typeof ResizeObserver !== 'function') return;
    const ro = new ResizeObserver(() => drawRef.current && drawRef.current());
    if (cv.current) ro.observe(cv.current);
    return () => ro.disconnect();
  }, []);
  const mm = () => (conv ? conv.paper[0] : sheet[0]) / cv.current.clientWidth;
  return <canvas ref={cv} style={{ cursor: pick ? 'crosshair' : 'move', touchAction: 'none' }}
    onPointerDown={e => { if (pick) { const r = cv.current.getBoundingClientRect(); onPick((e.clientX - r.left) * mm(), (e.clientY - r.top) * mm()); return; } if (conv?.paths.length) { drag.current = { x: e.clientX, y: e.clientY }; try { e.target.setPointerCapture(e.pointerId); } catch (err) { /* inactive/synthetic pointer: drag still works while it stays on the canvas */ } } }}
    onPointerMove={e => { const d = drag.current; if (!d) return; sh.current = [d.dx = (e.clientX - d.x) * mm(), d.dy = (e.clientY - d.y) * mm()]; draw(); }}
    onPointerUp={() => { const d = drag.current; drag.current = null; if (d && d.dx !== undefined) onDrop(d.dx, d.dy); }} />;
}
