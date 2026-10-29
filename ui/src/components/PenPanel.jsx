export const DEFAULT_PEN = { penMode: 'servo', servoUp: 'M5', servoDown: 'M3 S255', zUp: 5, zDown: 0, dwell: 0.15, servStep: 5 };
const sval = s => { const m = /S(-?\d*\.?\d+)/.exec(s || ''); return m ? +m[1] : null; };
const setS = (cmd, v) => (cmd || '').replace(/S(-?\d*\.?\d+)/, `S${v}`);
export default function PenPanel({ t, pen, setPen }) {
  const set = (k, v) => setPen({ ...pen, [k]: v });
  const num = (k, w = 70) => <input type="number" style={{ width: w }} value={pen[k]} step="any" onChange={e => { const v = +e.target.value; if (!isNaN(v)) set(k, v); }} />;
  // Servo position helper: edits the S number inside the up/down command (if present),
  // with a slider for quick moves and a numeric field for precise values. Step is selectable.
  const servoRow = (cmdKey, label) => {
    const v = sval(pen[cmdKey]); if (v === null) return null;
    const max = Math.max(255, Math.ceil(v / 50) * 50);
    const apply = x => { if (!isNaN(x)) set(cmdKey, setS(pen[cmdKey], x)); };
    return (<div className="servorow">
      <label><span style={{ minWidth: 150 }}>{label}</span>
        <input type="number" style={{ width: 70 }} value={v} step="any" onChange={e => apply(+e.target.value)} />
        <select value={pen.servStep} title={t('step')} onChange={e => set('servStep', +e.target.value)}>
          {[1, 5, 10, 15].map(s => <option key={s} value={s}>{t('step')} {s}</option>)}
        </select></label>
      <input type="range" min="0" max={max} step={pen.servStep || 1} value={Math.min(max, Math.max(0, v))} onChange={e => apply(+e.target.value)} />
    </div>);
  };
  return (<section><h2>{t('penset')}</h2>
    <label><span>{t('penmode')}</span><select value={pen.penMode} onChange={e => set('penMode', e.target.value)}><option value="servo">{t('msvo')}</option><option value="z">{t('mz')}</option></select></label>
    {pen.penMode === 'servo'
      ? <><label><span style={{ minWidth: 150 }}>{t('cup')}</span><input style={{ width: 130 }} value={pen.servoUp} onChange={e => set('servoUp', e.target.value)} /></label>
          {servoRow('servoUp', t('svup'))}
          <label><span style={{ minWidth: 150 }}>{t('cdown')}</span><input style={{ width: 130 }} value={pen.servoDown} onChange={e => set('servoDown', e.target.value)} /></label>
          {servoRow('servoDown', t('svdown'))}
          <div className="hint">{t('svnote')}</div></>
      : <label><span style={{ minWidth: 150 }}>{t('zud')}</span>{num('zUp')}{num('zDown')}</label>}
    <label><span style={{ minWidth: 150 }}>{t('dwell')}</span>{num('dwell')}</label>
    <div className="warn">{t('phint')}</div></section>);
}
