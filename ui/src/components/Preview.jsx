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
export default function Preview({ conv, pos, onDrop, pick, onPick, yOrigin = 'top' }) {
  const cv = useRef(); const drag = useRef(null); const sh = useRef([0, 0]);
  const draw = (s = sh.current) => {
    const c = cv.current, x = c.getContext('2d'), [pw, ph] = conv ? conv.paper : [210, 297], k = 420 / pw;
    const fy = yOrigin === 'bottom' ? (b => ph - b) : yOrigin === 'reverse' ? (b => -b) : (b => b);
    c.width = 420; c.height = ph * k; x.fillStyle = '#fff'; x.fillRect(0, 0, c.width, c.height); x.strokeStyle = '#1f6feb';
    (conv?.paths || []).forEach(p => { x.beginPath(); p.forEach(([a, b], i) => x[i ? 'lineTo' : 'moveTo']((a + s[0]) * k, fy(b + s[1]) * k)); x.stroke(); });
    if (pos) { x.fillStyle = '#c62828'; x.beginPath(); x.arc(pos[0] * k, fy(pos[1]) * k, 4, 0, 7); x.fill(); }
  };
  useEffect(() => { sh.current = [0, 0]; draw(); }, [conv, yOrigin]); // new geometry carries the offset itself
  useEffect(() => { if (!drag.current) draw(); }, [pos]); // the live pen dot must not erase a running drag
  const mm = () => (conv ? conv.paper[0] : 210) / cv.current.clientWidth;
  return <canvas ref={cv} style={{ cursor: pick ? 'crosshair' : 'move', touchAction: 'none' }}
    onPointerDown={e => { if (pick) { const r = cv.current.getBoundingClientRect(); onPick((e.clientX - r.left) * mm(), (e.clientY - r.top) * mm()); return; } if (conv?.paths.length) { drag.current = { x: e.clientX, y: e.clientY }; try { e.target.setPointerCapture(e.pointerId); } catch (err) { /* inactive/synthetic pointer: drag still works while it stays on the canvas */ } } }}
    onPointerMove={e => { const d = drag.current; if (!d) return; sh.current = [d.dx = (e.clientX - d.x) * mm(), d.dy = (e.clientY - d.y) * mm()]; draw(); }}
    onPointerUp={() => { const d = drag.current; drag.current = null; if (d && d.dx !== undefined) onDrop(d.dx, d.dy); }} />;
}
