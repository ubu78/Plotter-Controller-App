import { useState } from 'react';
export default function ManualPanel({ t, send, pen }) {
  const [step, setStep] = useState(10); const [feed, setFeed] = useState(2000);
  const jog = (x, y) => send(`$J=G91 G21 ${x ? 'X' + x * step : ''} ${y ? 'Y' + y * step : ''} F${feed}`); // Y grows downward
  const penCmd = up => send(pen.penMode === 'z' ? `G0 Z${up ? pen.zUp : pen.zDown}` : up ? pen.servoUp : pen.servoDown);
  return (<section><h2>{t('manual')}</h2>
    <div className="pad"><span /><button onClick={() => jog(0, -1)}>↑</button><span />
      <button onClick={() => jog(-1, 0)}>←</button><button title="G10 L20 P1 X0 Y0" onClick={() => send('G10 L20 P1 X0 Y0')}>0</button><button onClick={() => jog(1, 0)}>→</button>
      <span /><button onClick={() => jog(0, 1)}>↓</button><span /></div>
    <label><span>{t('step')}</span><select value={step} onChange={e => setStep(+e.target.value)}>{[0.1, 1, 10, 50].map(v => <option key={v}>{v}</option>)}</select> mm</label>
    <label><span>{t('feed')}</span><input type="number" value={feed} onChange={e => setFeed(+e.target.value)} /></label>
    <div className="row"><button onClick={() => penCmd(true)}>{t('penup')}</button><button onClick={() => penCmd(false)}>{t('pendown')}</button></div></section>);
}
