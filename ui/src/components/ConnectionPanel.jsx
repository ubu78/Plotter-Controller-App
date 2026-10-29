import { useEffect, useState } from 'react';
import { api } from '../api';
// Connection info banner: connected / disconnected / error (Alarm) at a glance.
// Print session controls (plot / hold / resume / feed override) also live here.
export default function ConnectionPanel({ t, status, send, gcode, setLog }) {
  const [ports, setPorts] = useState([]); const [port, setPort] = useState(''); const [baud, setBaud] = useState(115200);
  const refresh = () => api('ports').then(p => { if (Array.isArray(p)) { setPorts(p); setPort(x => x || p[0]?.path || ''); } });
  useEffect(() => { refresh(); }, []);
  const [x, y, z] = status.wpos;
  const s = status.state || '';
  const connecting = /^connect/i.test(s), alarm = /^alarm/i.test(s), door = /^door/i.test(s);
  const mode = !status.connected ? 'off' : alarm ? 'err' : (connecting || door) ? 'mid' : 'ok';
  const msg = mode === 'err' ? t('sterr') : !status.connected ? t('stoff') : connecting ? t('stmid') : t('stok');
  const ov = Math.round(status.ov ?? 100);
  const run = () => gcode ? api('stream', { gcode }) : setLog?.(l => [...l, t('noconv')]);
  const runOff = !status.connected || !gcode, runTitle = !status.connected ? t('stoff') : !gcode ? t('noconv') : '';
  return (<section><h2>{t('conn')}</h2>
    <div className={`connstat ${mode}`} role="status" aria-live="polite" title={s}>
      <i /><b>{msg}</b>{status.connected && s && !connecting && <small>{s}</small>}
      {mode === 'err' && <span className="csfix">{t('sterrfix')}</span>}
    </div>
    <label><span>{t('port')}</span><select value={port} onChange={e => setPort(e.target.value)}>{ports.map(p => <option key={p.path} value={p.path}>{p.path} {p.name}</option>)}</select><button onClick={refresh}>↻</button></label>
    <label><span>{t('baud')}</span><input type="number" value={baud} onChange={e => setBaud(+e.target.value)} /></label>
    <div className="row"><button className="pri" onClick={() => api('connect', { port, baud })}>{t('connect')}</button>
      <button onClick={() => api('disconnect', {})}>{t('disconnect')}</button></div>
    <div className="safety">
      <button title="$X — GRBL unlock" onClick={() => send('$X')}>$X<small>{t('unlock')}</small></button>
      <button title="$H — homing cycle" onClick={() => send('$H')}>$H<small>{t('home')}</small></button>
    </div>
    <div className="row runrow">
      <button className="pri" disabled={runOff} title={runTitle} onClick={run}>{t('plot')}</button>
      <button disabled={!status.connected} title="!" onClick={() => api('hold', {})}>❚❚ {t('pause')}</button>
      <button className="res" disabled={!status.connected} title="~" onClick={() => api('resume', {})}>▶ {t('resume')}</button>
    </div>
    <div className="row">
      <button disabled={!status.connected} title="F −10%" onClick={() => api('feedov', { kind: 'minus' })}>F−</button>
      <button disabled={!status.connected} title="F 100%" onClick={() => api('feedov', { kind: 'reset' })}>F%</button>
      <button disabled={!status.connected} title="F +10%" onClick={() => api('feedov', { kind: 'plus' })}>F+</button>
      <b className="fovval" title="Feed override">F {ov}%</b>
    </div>
    <div className="hint">{t('safetyhint')}</div>
    <div id="pos">X{x.toFixed(2)} Y{y.toFixed(2)} Z{z.toFixed(2)}</div>
    <div id="bar"><i style={{ width: status.progress * 100 + '%' }} /></div></section>);
}
