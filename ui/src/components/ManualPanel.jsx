import { useEffect, useState } from 'react';
export default function ManualPanel({ t, send, pen, status }) {
  const [step, setStep] = useState(10); const [feed, setFeed] = useState(2000); const [inch, setInch] = useState(false);
  const [wcsi, setWcsi] = useState(() => { const v = parseInt(status.wcs?.slice(1), 10); return v >= 54 && v <= 59 ? v - 54 : 0; }); // 0 -> G54
  const wcs = 'G' + (54 + wcsi), p = wcsi + 1; // G10 L20 P index: P1=G54 ... P6=G59
  useEffect(() => { // keep the selector truthful: the board reports its modal state via [GC:...] ($G on connect)
    const v = parseInt(status.wcs?.slice(1), 10);
    if (v >= 54 && v <= 59 && v - 54 !== wcsi) setWcsi(v - 54);
  }, [status.wcs]);
  const pickWcs = e => { const v = +e.target.value; setWcsi(v); send('G' + (54 + v)); };
  const k = inch ? 1 / 25.4 : 1; // mm step -> active unit system
  const jog = (x, y, z = 0) => send(`$J=G91 ${inch ? 'G20' : 'G21'}${x ? ` X${(x * step * k).toFixed(4)}` : ''}${y ? ` Y${(y * step * k).toFixed(4)}` : ''}${z ? ` Z${(z * step * k).toFixed(4)}` : ''} F${inch ? (feed * k).toFixed(2) : feed}`); // Y grows downward
  const penCmd = up => send(pen.penMode === 'z' ? `G0 Z${up ? pen.zUp : pen.zDown}` : up ? pen.servoUp : pen.servoDown);
  const coord = i => (status.connected ? status.wpos[i] || 0 : 0).toFixed(inch ? 3 : 2);
  const ax = (n, i, c) => (<div className="axrow"><button className="zbtn" title={`G10 L20 P${p} ${n}0`} onClick={() => send(`G10 L20 P${p} ${n}0`)}>⟲</button>
    <b style={{ color: c }}>{n}</b><span className="work">{t('work')}</span><code>{coord(i)}</code></div>);
  return (<section><h2>{t('manual')}</h2>
    <div className="row"><select className="wcs" value={wcs} onChange={pickWcs} title={t('wcshint')}>{[54, 55, 56, 57, 58, 59].map(n => <option key={n}>{'G' + n}</option>)}</select>
      <button className={inch ? '' : 'pri'} onClick={() => { setInch(false); send('G21'); }}>mm</button>
      <button className={inch ? 'pri' : ''} onClick={() => { setInch(true); send('G20'); }}>{t('inch')}</button></div>
    {ax('X', 0, '#e53935')}{ax('Y', 1, '#43a047')}{ax('Z', 2, '#1e88e5')}
    <div className="row"><button onClick={() => send(`G10 L20 P${p} X0 Y0 Z0`)}>{t('setzero3')}</button>
      <button title="G0 X0 Y0" onClick={() => { penCmd(true); send('G0 X0 Y0'); }}>{t('gotozero')}</button></div>
    <div className="pad"><span /><button onClick={() => jog(0, -1)}>↑</button><span />
      <button onClick={() => jog(-1, 0)}>←</button><button title={`G10 L20 P${p} X0 Y0`} onClick={() => send(`G10 L20 P${p} X0 Y0`)}>0</button><button onClick={() => jog(1, 0)}>→</button>
      <span /><button onClick={() => jog(0, 1)}>↓</button><span /></div>
    <div className="row"><button onClick={() => jog(0, 0, 1)}>Z+</button><button onClick={() => jog(0, 0, -1)}>Z−</button></div>
    <label><span>{t('step')}</span><select value={step} onChange={e => setStep(+e.target.value)}>{[0.1, 1, 10, 50].map(v => <option key={v}>{v}</option>)}</select> mm</label>
    <label><span>{t('feed')}</span><input type="number" value={feed} onChange={e => setFeed(+e.target.value)} /></label>
    <div className="row"><button onClick={() => penCmd(true)}>{t('penup')}</button><button onClick={() => penCmd(false)}>{t('pendown')}</button></div></section>);
}
