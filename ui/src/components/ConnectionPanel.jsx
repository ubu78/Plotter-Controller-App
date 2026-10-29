import { useEffect, useState } from 'react';
import { api } from '../api';
// Connection info banner: connected / disconnected / error (Alarm) at a glance.
export default function ConnectionPanel({ t, status, send }) {
  const [ports, setPorts] = useState([]); const [port, setPort] = useState(''); const [baud, setBaud] = useState(115200);
  const refresh = () => api('ports').then(p => { if (Array.isArray(p)) { setPorts(p); setPort(x => x || p[0]?.path || ''); } });
  useEffect(() => { refresh(); }, []);
  const [x, y, z] = status.wpos;
  const s = status.state || '';
  const connecting = /^connect/i.test(s), alarm = /^alarm/i.test(s), door = /^door/i.test(s);
  const mode = !status.connected ? 'off' : alarm ? 'err' : (connecting || door) ? 'mid' : 'ok';
  const msg = mode === 'err' ? t('sterr') : !status.connected ? t('stoff') : connecting ? t('stmid') : t('stok');
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
    <div className="hint">{t('safetyhint')}</div>
    <div id="pos">X{x.toFixed(2)} Y{y.toFixed(2)} Z{z.toFixed(2)}</div>
    <div id="bar"><i style={{ width: status.progress * 100 + '%' }} /></div></section>);
}
