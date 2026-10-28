import { useEffect, useState } from 'react';
import { api } from '../api';
import Preview from './Preview.jsx';
const b64 = u8 => { let s = ''; for (let i = 0; i < u8.length; i += 32768) s += String.fromCharCode.apply(null, u8.subarray(i, i + 32768)); return btoa(s); };
export default function SourcePanel({ t, pen, status, setLog, onConv }) {
  const [o, setO] = useState({ paper: 'A4', margin: 0, scale: 100, center: false, feed: 3000, landscape: false, laserOff: true });
  const [tx, setTx] = useState({ text: '', font: 'Roboto', size: 6, lineHeight: 1.4, markdown: false, singleLine: true, startX: 0, startY: 0 });
  const [fonts, setFonts] = useState([]); const [svg, setSvg] = useState('');
  const [mode, setMode] = useState('svg'); const [off, setOff] = useState([0, 0]); const [page, setPage] = useState(0); const [conv, setConv] = useState(null); const [pick, setPick] = useState(false); const [pdf, setPdf] = useState(''); const [pdfDraw, setPdfDraw] = useState(true);
  const set = (k, v) => setO(p => ({ ...p, [k]: v })); const st = (k, v) => setTx(p => ({ ...p, [k]: v }));
  useEffect(() => { api('fonts').then(setFonts); }, []);
  const run = async (m = mode, of = off, pg = page, source = svg, txo = tx, pdfData = pdf) => {
    if (m === 'svg' && !source) return;
    const options = { ...o, ...pen, offsetX: of[0], offsetY: of[1], page: pg };
    const r = m === 'pdf' ? await api('pdf', { data: pdfData, options: { ...options, startX: 0, startY: 0 } }) : m === 'text' ? await api('text', { text: txo.text, options: { ...options, ...txo, text: undefined } }) : await api('svg', { svg: source, options });
    if (r.error) return setLog(l => [...l, 'ERR ' + r.error]);
    setConv(r); onConv?.(r); setPage(r.page || 0);
  };
  const num = (k, v, f = set) => <input type="number" value={v} onChange={e => f(k, +e.target.value)} />;
  return (<>
    <section><h2>{t('svg')}</h2>
      <input type="file" accept=".svg,image/svg+xml" onChange={async e => { const s = await e.target.files[0].text(); setSvg(s); setMode('svg'); setOff([0, 0]); run('svg', [0, 0], 0, s); }} />
      <label><span>{t('paper')}</span><select value={o.paper} onChange={e => set('paper', e.target.value)}><option>A4</option><option>A5</option><option>Letter</option></select></label>
      <label><span>{t('margin')}</span>{num('margin', o.margin)} mm</label><label><span>{t('scale')}</span>{num('scale', o.scale)} %</label>
      <label><span>{t('center')}</span><input type="checkbox" checked={o.center} onChange={e => set('center', e.target.checked)} /></label>
      <label><span>{t('feed')}</span>{num('feed', o.feed)}</label>
      <label><span>{t('land')}</span><input type="checkbox" checked={o.landscape} onChange={e => set('landscape', e.target.checked)} /></label>
      <label><span>{t('laser')}</span><input type="checkbox" checked={o.laserOff} onChange={e => set('laserOff', e.target.checked)} /></label>
      <div className="row"><button className="pri" onClick={() => run()}>{t('convert')}</button>
        <button onClick={() => conv?.gcode ? api('stream', { gcode: conv.gcode }) : setLog(l => [...l, t('noconv')])}>{t('plot')}</button>
        <button onClick={() => api('hold', {})}>❚❚</button><button onClick={() => api('resume', {})}>▶</button>
        <button onClick={() => { if (!conv?.gcode) return setLog(l => [...l, t('noconv')]);
          api('savefile', { name: 'drawing.gcode', content: conv.gcode }).then(r => setLog(l => [...l, '→ ' + (r?.path || '')]), e => setLog(l => [...l, 'ERR ' + e])); }}>{t('save')}</button></div>
      <div className="warn">{(conv?.warnings || []).join(' • ')}</div></section>
    <section><h2>{t('text')}</h2>
      <label><span>{t('doc')}</span><input type="file" accept=".pdf,.docx,.txt,.md" onChange={async e => { const f = e.target.files[0]; if (!f) return;
        const data = b64(new Uint8Array(await f.arrayBuffer()));
        if (/\.pdf$/i.test(f.name) && pdfDraw) { setPdf(data); setMode('pdf'); setOff([0, 0]); setPage(0); run('pdf', [0, 0], 0, svg, tx, data); return; }
        const r = await api('doc', { name: f.name, data }); if (r.error) return setLog(l => [...l, 'ERR ' + r.error]);
        setTx(p => ({ ...p, text: r.text, markdown: /\.md$/i.test(f.name) })); }} /></label>
      <label><span>{t('pdfdraw')}</span><input type="checkbox" checked={pdfDraw} onChange={e => setPdfDraw(e.target.checked)} /></label>
      <textarea rows={6} style={{ width: '100%' }} value={tx.text} placeholder="Привет, мир! Tere, õäöü" onChange={e => st('text', e.target.value)} />
      <label><span>{t('font')}</span><select value={tx.font} onChange={e => st('font', e.target.value)}>{fonts.map(f => <option key={f}>{f}</option>)}</select></label>
      <label><span>{t('fsize')}</span>{num('size', tx.size, st)}</label><label><span>{t('lh')}</span>{num('lineHeight', tx.lineHeight, st)}</label>
      <label><span>{t('single')}</span><input type="checkbox" checked={tx.singleLine} onChange={e => st('singleLine', e.target.checked)} /></label>
      <label><span>{t('start')}</span>{num('startX', tx.startX, st)}{num('startY', tx.startY, st)}
        <button className={pick ? 'pri' : ''} onClick={() => setPick(!pick)}>{pick ? t('picking') : t('pick')}</button></label>
      <label><span>{t('md')}</span><input type="checkbox" checked={tx.markdown} onChange={e => st('markdown', e.target.checked)} /></label>
      <div className="row"><button className="pri" onClick={() => { setMode('text'); setOff([0, 0]); setPage(0); run('text', [0, 0], 0); }}>{t('ctext')}</button>
        <button onClick={() => mode !== 'svg' && page > 0 && run(mode, off, page - 1)}>◀</button><b>{conv?.pages ? `${page + 1}/${conv.pages}` : '–'}</b>
        <button onClick={() => mode !== 'svg' && conv && page < conv.pages - 1 && run(mode, off, page + 1)}>▶</button></div>
      <div className="warn">{t('drag')}</div></section>
    <section><h2>{t('preview')}</h2><Preview conv={conv} pos={status.connected ? status.wpos : null} pick={pick}
      onPick={(x, y) => { const n = { ...tx, startX: +x.toFixed(1), startY: +y.toFixed(1) }; setTx(n); setPick(false); setMode('text'); setPage(0); run('text', off, 0, svg, n); }}
      onDrop={(dx, dy) => { if (mode === 'text') { const n = { ...tx, startX: +Math.max(0, tx.startX + dx).toFixed(1), startY: +Math.max(0, tx.startY + dy).toFixed(1) }; setTx(n); run('text', off, page, svg, n); }
        else { const n = [off[0] + dx, off[1] + dy]; setOff(n); run(mode, n); } }} /></section></>);
}
