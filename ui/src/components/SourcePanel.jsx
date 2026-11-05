import { useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../api';
import Preview from './Preview.jsx';
const b64 = u8 => { let s = ''; for (let i = 0; i < u8.length; i += 32768) s += String.fromCharCode.apply(null, u8.subarray(i, i + 32768)); return btoa(s); };
// Mini previews of the four letter styles: the same "O" drawn in each style.
const RING = 'M12 4.5 A7.5 7.5 0 1 0 12 19.5 A7.5 7.5 0 1 0 12 4.5 Z M12 8 A4 4 0 1 1 12 16 A4 4 0 1 1 12 8 Z';
const STYLE_ICONS = {
  pen: <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round">
    <path d="M5 14 C7 7.5 15 5.5 17 9.5 C19 13.5 13 17 10 15 C7.2 13 9 9.5 12 9.5" /></svg>,
  single: <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" strokeWidth="1.2">
    <circle cx="12" cy="12" r="7.5" /><circle cx="12" cy="12" r="4" /></svg>,
  fill: <svg viewBox="0 0 24 24" width="24" height="24">
    <defs><pattern id="pv-fill" width="1.8" height="1.8" patternUnits="userSpaceOnUse">
      <path d="M0 0.9 H1.8" stroke="currentColor" strokeWidth="0.7" /></pattern></defs>
    <path d={RING} fillRule="evenodd" fill="url(#pv-fill)" stroke="currentColor" strokeWidth="0.8" /></svg>,
  hatch: <svg viewBox="0 0 24 24" width="24" height="24">
    <defs><pattern id="pv-hatch" width="2.6" height="2.6" patternUnits="userSpaceOnUse">
      <path d="M0 2.6 L2.6 0 M-0.65 0.65 L0.65 -0.65 M1.95 3.25 L3.25 1.95" stroke="currentColor" strokeWidth="0.7" /></pattern></defs>
    <path d={RING} fillRule="evenodd" fill="url(#pv-hatch)" stroke="currentColor" strokeWidth="0.8" /></svg>,
};
const STYLES = [['pen', 'fmpen'], ['single', 'fmsingle'], ['fill', 'fmfill'], ['hatch', 'fmhatch']];
const GOOGLE_STYLES = ['regular', 'bold', 'italic', 'bolditalic']; // resolved on the Rust side (download or cached variant)
export default function SourcePanel({ t, pen, status, setLog, onConv }) {
  // Conversion options persist in localStorage: the machine's Y direction does not change
  // between sessions, so «Invert Y» must not reset on restart either.
  const [o, setO] = useState(() => { const d = { paper: 'A4', margin: 0, marginL: 0, marginR: 0, marginT: 0, marginB: 0, scale: 100, center: false, feed: 3000, travelFeed: 0, landscape: false, laserOff: true, invertY: false, yOrigin: 'top' };
    try { const s = { ...d, ...JSON.parse(localStorage.svgopts || '{}') };
      // 0.5.22 migration: the single «Поле» became four per-side margins (left/right/top/bottom)
      if (s.marginL === undefined && +(s.margin || 0) > 0) { s.marginL = s.marginR = s.marginT = s.marginB = +s.margin; s.margin = 0; }
      return s; } catch (e) { return d; } });
  useEffect(() => { try { localStorage.svgopts = JSON.stringify(o); } catch (e) { /* storage blocked */ } }, [o]);
  const [tx, setTx] = useState({ text: '', font: 'Roboto', fontStyle: 'regular', size: 6, lineHeight: 1.4, markdown: false, fillMode: 'pen', fillSpacing: 0, hatchAngle: 45, startX: 0, startY: 0 });
  const [fonts, setFonts] = useState({ google: [], system: [] }); const [svg, setSvg] = useState('');
  const [fp, setFp] = useState(null); const fpSeq = useRef(0);
  const [mode, setMode] = useState('svg'); const [off, setOff] = useState([0, 0]); const [page, setPage] = useState(0); const [conv, setConv] = useState(null); const [pick, setPick] = useState(false); const [pdf, setPdf] = useState(''); const [pdfDraw, setPdfDraw] = useState(true);
  const set = (k, v) => setO(p => ({ ...p, [k]: v })); const st = (k, v) => setTx(p => ({ ...p, [k]: v }));
  // 0.5.22: sheet size of the SELECTED format — the preview frame/grid follow it even before
  // the first conversion. Mirrors the backend cap (Letter is limited to A4) and landscape swap.
  // Memoized: a fresh array each render would reset the preview's pending drag shift.
  const sheet = useMemo(() => { let w = o.paper === 'A5' ? 148 : o.paper === 'Letter' ? 215.9 : 210,
    h = o.paper === 'A5' ? 210 : o.paper === 'Letter' ? 279.4 : 297;
    if (w > 210 || h > 297) { w = Math.min(w, 210); h = Math.min(h, 297); }
    return o.landscape ? [h, w] : [w, h]; }, [o.paper, o.landscape]);
  // effective preview margins: the four per-side values win over the legacy single field
  const mg = useMemo(() => ((o.marginL || 0) > 0 || (o.marginR || 0) > 0 || (o.marginT || 0) > 0 || (o.marginB || 0) > 0)
    ? [o.marginL || 0, o.marginR || 0, o.marginT || 0, o.marginB || 0]
    : [o.margin || 0, o.margin || 0, o.margin || 0, o.margin || 0], [o.marginL, o.marginR, o.marginT, o.marginB, o.margin]);
  const SNAME = { regular: t('streg'), bold: t('stbold'), italic: t('stital'), bolditalic: t('stbi') };
  const fam = fonts.system.find(f => f.value === tx.font); // selected family is a system font?
  // Google fonts + every family installed on the system; ↻ rescans after installing new fonts
  const loadFonts = () => api('fonts').then(f => setFonts(Array.isArray(f) ? { google: f, system: [] } : { google: f.google || [], system: f.system || [] }));
  useEffect(() => { loadFonts(); }, []);
  // The font value sent to the converter: a system style maps to the exact face file from
  // the scan (sys:<path>), a Google style becomes 'Family:variant' and is resolved on the
  // Rust side (downloaded once, cached next to the regular face, Regular on 400 fallback).
  const fontVal = txo => {
    const stl = txo.fontStyle || 'regular';
    if (stl === 'regular') return txo.font;
    const fam = fonts.system.find(f => f.value === txo.font);
    const s = fam && (fam.styles || []).find(s => s.key === stl);
    return s ? s.value : txo.font + ':' + stl;
  };
  // Live preview: the real glyph contours of the sample text (or of the typed text),
  // rendered from the same outlines the plotter draws. Debounced, stale replies dropped.
  useEffect(() => {
    const id = ++fpSeq.current;
    const tm = setTimeout(() => api('fontpreview', { font: fontVal(tx), text: (tx.text.trim() || 'Aa Яя 123').slice(0, 12) })
      .then(r => { if (fpSeq.current === id) setFp(r && r.paths ? r : null); }).catch(() => {}), 250);
    return () => clearTimeout(tm);
  }, [tx.font, tx.fontStyle, tx.text, fonts.system]);
  const run = async (m = mode, of = off, pg = page, source = svg, txo = tx, pdfData = pdf) => {
    if (m === 'svg' && !source) return;
    const options = { ...o, ...pen, offsetX: of[0], offsetY: of[1], page: pg };
    const r = m === 'pdf' ? await api('pdf', { data: pdfData, options: { ...options, startX: 0, startY: 0 } }) : m === 'text' ? await api('text', { text: txo.text, options: { ...options, ...txo, text: undefined, font: fontVal(txo) } }) : await api('svg', { svg: source, options });
    if (r.error) return setLog(l => [...l, 'ERR ' + r.error]);
    setConv(r); onConv?.(r); setPage(r.page || 0);
  };
  const num = (k, v, f = set) => <input type="number" value={v} onChange={e => f(k, +e.target.value)} />;
  return (<>
    <section><h2>{t('svg')}</h2>
      <input type="file" accept=".svg,image/svg+xml" onChange={async e => { const s = await e.target.files[0].text(); setSvg(s); setMode('svg'); setOff([0, 0]); run('svg', [0, 0], 0, s); }} />
      <label><span>{t('paper')}</span><select value={o.paper} onChange={e => set('paper', e.target.value)}><option>A4</option><option>A5</option><option>Letter</option></select></label>
      <div className="margins">
        <label><span>{t('mgl')}</span>{num('marginL', o.marginL)} мм</label>
        <label><span>{t('mgr')}</span>{num('marginR', o.marginR)} мм</label>
        <label><span>{t('mgt')}</span>{num('marginT', o.marginT)} мм</label>
        <label><span>{t('mgb')}</span>{num('marginB', o.marginB)} мм</label>
      </div>
      <div className="hint">{t('marginhint')}</div>
      <label><span>{t('scale')}</span>{num('scale', o.scale)} %</label>
      <label><span>{t('center')}</span><input type="checkbox" checked={o.center} onChange={e => set('center', e.target.checked)} /></label>
      <label><span>{t('feed')}</span>{num('feed', o.feed)}</label>
      <label title={t('travelhint')}><span>{t('travelspeed')}</span>{num('travelFeed', o.travelFeed)}</label>
      <div className="hint">{t('travelhint')}</div>
      <label><span>{t('land')}</span><input type="checkbox" checked={o.landscape} onChange={e => set('landscape', e.target.checked)} /></label>
      <label><span>{t('laser')}</span><input type="checkbox" checked={o.laserOff} onChange={e => set('laserOff', e.target.checked)} /></label>
      <label title={t('invyhint')}><span>{t('invy')}</span><input type="checkbox" checked={o.invertY} onChange={e => set('invertY', e.target.checked)} /></label>
      <div className="hint">{t('invyhint')}</div>
      <label title={t('yorhint')}><span>{t('yorigin')}</span>
        <select value={o.yOrigin || 'top'} onChange={e => set('yOrigin', e.target.value)}>
          <option value="top">{t('yotop')}</option>
          <option value="bottom">{t('yobottom')}</option>
          <option value="reverse">{t('yoreverse')}</option>
        </select></label>
      <div className="hint">{t('yorhint')}</div>
      <div className="row"><button className="pri" onClick={() => run()}>{t('convert')}</button>
        <button onClick={() => { if (!conv?.gcode) return setLog(l => [...l, t('noconv')]);
          api('savefile', { name: 'drawing.gcode', content: conv.gcode }).then(r => setLog(l => [...l, '→ ' + (r?.path || '')]), e => setLog(l => [...l, 'ERR ' + e])); }}>{t('save')}</button></div>
      <div className="hint">{t('dragsvg')}</div>
      <div className="warn">{(conv?.warnings || []).join(' • ')}</div></section>
    <section><h2>{t('text')}</h2>
      <label><span>{t('doc')}</span><input type="file" accept=".pdf,.docx,.txt,.md" onChange={async e => { const f = e.target.files[0]; if (!f) return;
        const data = b64(new Uint8Array(await f.arrayBuffer()));
        if (/\.pdf$/i.test(f.name) && pdfDraw) { setPdf(data); setMode('pdf'); setOff([0, 0]); setPage(0); run('pdf', [0, 0], 0, svg, tx, data); return; }
        const r = await api('doc', { name: f.name, data }); if (r.error) return setLog(l => [...l, 'ERR ' + r.error]);
        setTx(p => ({ ...p, text: r.text, markdown: /\.md$/i.test(f.name) })); }} /></label>
      <label><span>{t('pdfdraw')}</span><input type="checkbox" checked={pdfDraw} onChange={e => setPdfDraw(e.target.checked)} /></label>
      <textarea rows={6} style={{ width: '100%' }} value={tx.text} placeholder="Привет, мир! Tere, õäöü" onChange={e => st('text', e.target.value)} />
      <label><span>{t('font')}</span><select value={tx.font} onChange={e => { const nf = e.target.value; const nfam = fonts.system.find(f => f.value === nf);
        const keys = nfam ? (nfam.styles || []).map(s => s.key) : GOOGLE_STYLES;
        if (!keys.includes(tx.fontStyle || 'regular')) st('fontStyle', 'regular'); // new family may lack the current style
        st('font', nf); }}>
        <optgroup label={t('fontgoogle')}>{fonts.google.map(f => <option key={f}>{f}</option>)}</optgroup>
        {fonts.system.length > 0 && <optgroup label={t('fontsys')}>{fonts.system.map(f => <option key={f.value} value={f.value}>{f.family}</option>)}</optgroup>}
      </select><button title={t('frefresh')} onClick={e => { e.preventDefault(); loadFonts(); }}>↻</button></label>
      <label><span>{t('fstyle')}</span><select value={tx.fontStyle || 'regular'} onChange={e => st('fontStyle', e.target.value)}>
        {(fam ? (fam.styles || []).map(s => [s.key, s.name]) : GOOGLE_STYLES.map(k => [k, k]))
          .map(([k, n]) => <option key={k} value={k}>{SNAME[k] || n || k}</option>)}
      </select></label>
      {fp && <div className="fprev"><svg viewBox={`0 0 ${fp.w} ${fp.h}`} preserveAspectRatio="xMinYMid meet" style={{ width: '100%', height: 44, display: 'block' }}>
        {fp.paths.map((p, i) => <path key={i} d={'M' + p.map(v => v[0].toFixed(2) + ' ' + v[1].toFixed(2)).join('L')} fill="none" stroke="currentColor" strokeWidth="0.18" strokeLinejoin="round" />)}
      </svg></div>}
      <label><span>{t('fsize')}</span>{num('size', tx.size, st)}</label><label><span>{t('lh')}</span>{num('lineHeight', tx.lineHeight, st)}</label>
      <label><span>{t('speed')}</span>{num('feed', o.feed)} мм/мин</label>
      <label title={t('travelhint')}><span>{t('travelspeed')}</span>{num('travelFeed', o.travelFeed)} мм/мин</label>
      <div className="stylerow" role="radiogroup" aria-label={t('style')}>
        {STYLES.map(([v, k]) => (<button key={v} className={tx.fillMode === v ? 'stybtn pri' : 'stybtn'} title={t('d' + v)} onClick={() => st('fillMode', v)}>
          {STYLE_ICONS[v]}<span>{t(k)}</span></button>))}
      </div>
      <div className="hint">{t('d' + tx.fillMode)}</div>
      {(tx.fillMode === 'fill' || tx.fillMode === 'hatch') && <>
        <label><span>{t('fillstep')}</span>{num('fillSpacing', tx.fillSpacing, st)} mm</label>
        <div className="hint">{t('fillstephint')}</div></>}
      {tx.fillMode === 'hatch' && <label><span>{t('hatchang')}</span>
        <input type="number" min="0" max="180" step="any" style={{ width: 64 }} value={tx.hatchAngle}
          onChange={e => { const v = +e.target.value; if (!isNaN(v)) st('hatchAngle', v); }} />
        <input type="range" min="0" max="180" step="1" value={Math.min(180, Math.max(0, tx.hatchAngle))}
          onChange={e => st('hatchAngle', +e.target.value)} />
        <b style={{ minWidth: 30, textAlign: 'right' }}>{tx.hatchAngle}°</b></label>}
      <label><span>{t('start')}</span>{num('startX', tx.startX, st)}{num('startY', tx.startY, st)}
        <button className={pick ? 'pri' : ''} onClick={() => setPick(!pick)}>{pick ? t('picking') : t('pick')}</button></label>
      <label><span>{t('md')}</span><input type="checkbox" checked={tx.markdown} onChange={e => st('markdown', e.target.checked)} /></label>
      <div className="row"><button className="pri" onClick={() => { setMode('text'); setOff([0, 0]); setPage(0); run('text', [0, 0], 0); }}>{t('ctext')}</button>
        <button onClick={() => mode !== 'svg' && page > 0 && run(mode, off, page - 1)}>◀</button><b>{conv?.pages ? `${page + 1}/${conv.pages}` : '–'}</b>
        <button onClick={() => mode !== 'svg' && conv && page < conv.pages - 1 && run(mode, off, page + 1)}>▶</button></div>
      <div className="warn">{t('drag')}</div></section>
    <section><h2>{t('preview')}</h2><Preview conv={conv} pos={status.connected ? status.wpos : null} pick={pick} yOrigin={mode === 'svg' || mode === 'pdf' ? (o.yOrigin || 'top') : 'top'} sheet={sheet} margins={mg}
      onPick={(x, y) => { const n = { ...tx, startX: +x.toFixed(1), startY: +y.toFixed(1) }; setTx(n); setPick(false); setMode('text'); setPage(0); run('text', off, 0, svg, n); }}
      onDrop={(dx, dy) => { if (mode === 'text') { const n = { ...tx, startX: +Math.max(0, tx.startX + dx).toFixed(1), startY: +Math.max(0, tx.startY + dy).toFixed(1) }; setTx(n); run('text', off, page, svg, n); }
        else { const n = [off[0] + dx, off[1] + dy]; setOff(n); run(mode, n); } }} /></section></>);
}
