export const DEFAULT_PEN = { penMode: 'servo', servoUp: 'M5', servoDown: 'M3 S255', zUp: 5, zDown: 0, dwell: 0.15 };
export default function PenPanel({ t, pen, setPen }) {
  const set = (k, v) => setPen({ ...pen, [k]: v });
  const num = (k, w = 70) => <input type="number" style={{ width: w }} value={pen[k]} step="any" onChange={e => set(k, +e.target.value)} />;
  return (<section><h2>{t('penset')}</h2>
    <label><span>{t('penmode')}</span><select value={pen.penMode} onChange={e => set('penMode', e.target.value)}><option value="servo">{t('msvo')}</option><option value="z">{t('mz')}</option></select></label>
    {pen.penMode === 'servo'
      ? <><label><span style={{ minWidth: 150 }}>{t('cup')}</span><input style={{ width: 130 }} value={pen.servoUp} onChange={e => set('servoUp', e.target.value)} /></label>
          <label><span style={{ minWidth: 150 }}>{t('cdown')}</span><input style={{ width: 130 }} value={pen.servoDown} onChange={e => set('servoDown', e.target.value)} /></label></>
      : <label><span style={{ minWidth: 150 }}>{t('zud')}</span>{num('zUp')}{num('zDown')}</label>}
    <label><span style={{ minWidth: 150 }}>{t('dwell')}</span>{num('dwell')}</label>
    <div className="warn">{t('phint')}</div></section>);
}
