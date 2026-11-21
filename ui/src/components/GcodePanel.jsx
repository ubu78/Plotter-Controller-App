import { useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../api';
const MAX = 1500; // rendering thousands of lines makes the page sluggish; the full text stays in memory
// 0.5.28 marker colors: the converter writes `; [PART <id>]` before every part of the
// program (svg drawing / text outline / text fill-hatch infill). The block below scans
// them, lets the user assign a marker name per part and asks the backend to insert an
// M0 pause (pen up, dwell, hold) at every boundary where the marker must change.
const PART_KEYS = ['svg', 'text', 'infill'];
const CHIPS = [['mkblack', '#141414'], ['mkblue', '#2456c4'], ['mkred', '#c62b2b'], ['mkgreen', '#1e8c3c'], ['mkorange', '#e07818'], ['mkbrown', '#7a4a21'], ['mkviolet', '#7a2fa0'], ['mkpink', '#d84a9c']];
export default function GcodePanel({ t, gcode, setGcode, setLog, pen }) {
  const [done, setDone] = useState(false);
  const [edit, setEdit] = useState(false);
  const taRef = useRef(null);
  const lines = gcode ? gcode.split('\n') : [];
  const log = m => setLog && setLog(l => [...l, m]);
  const copy = async () => {
    try { await navigator.clipboard.writeText(gcode); }
    catch (e) { // clipboard API may be unavailable: fall back to a hidden textarea
      const ta = document.createElement('textarea'); ta.value = gcode; ta.style.position = 'fixed'; ta.style.opacity = '0';
      document.body.appendChild(ta); ta.select(); try { document.execCommand('copy'); } catch (e2) { /* give up silently */ }
      ta.remove();
    }
    setDone(true); setTimeout(() => setDone(false), 1500);
  };
  const paste = async () => {
    let txt = '';
    try { txt = await navigator.clipboard.readText(); } catch (e) { /* WebKitGTK may deny the read */ }
    if (!txt) { // fallback: hidden textarea + execCommand('paste') (works in some WebKit builds)
      const ta = document.createElement('textarea'); ta.style.position = 'fixed'; ta.style.opacity = '0';
      document.body.appendChild(ta); ta.focus();
      try { document.execCommand('paste'); } catch (e2) { /* blocked */ }
      txt = ta.value; ta.remove();
    }
    if (!txt) return log(t('noclip'));
    if (edit && taRef.current) { // while editing: insert at the cursor
      const ta = taRef.current, s = ta.selectionStart ?? gcode.length, epos = ta.selectionEnd ?? s;
      setGcode(gcode.slice(0, s) + txt + gcode.slice(epos));
      const pos = s + txt.length;
      requestAnimationFrame(() => { ta.focus(); ta.setSelectionRange(pos, pos); });
    } else setGcode(txt); // otherwise: replace the whole program
  };
  const save = () => {
    if (!gcode.trim()) return;
    api('savefile', { name: 'drawing.gcode', content: gcode })
      .then(r => log(t('saved') + (r?.path || '')), e => log('ERR ' + e));
  };
  // --- markers (0.5.28) ---
  const [colors, setColors] = useState(() => { const d = { svg: '', text: '', infill: '' };
    try { return { ...d, ...JSON.parse(localStorage.markers || '{}') }; } catch (e) { return d; } });
  useEffect(() => { try { localStorage.markers = JSON.stringify(colors); } catch (e) { /* storage blocked */ } }, [colors]);
  const [pauses, setPauses] = useState(null);
  const parts = useMemo(() => { const seen = [];
    for (const l of lines) { const m = /^;\s*\[PART (\w+)\]/.exec(l); if (m && !seen.includes(m[1]) && PART_KEYS.includes(m[1])) seen.push(m[1]); }
    return seen; }, [gcode]);
  const penup = pen => (pen?.penMode === 'z' ? `G0 Z${pen.zUp}` : (pen?.servoUp || 'M3 S255'));
  const apply = (cols = colors) => {
    api('markers', { gcode, colors: cols, penup: penup(pen), dwell: pen?.dwell ?? 0.15 })
      .then(r => { if (r && typeof r.gcode === 'string') { setGcode(r.gcode); setPauses(r.pauses || 0); log(`${t('mkapplied')} ${r.pauses || 0}`); } })
      .catch(e => log('ERR ' + e));
  };
  const clear = () => { const d = { svg: '', text: '', infill: '' }; setColors(d); apply(d); };
  return (<section className="p-gcode"><h2>{t('gcode')} {lines.length ? `· ${lines.length} ${t('glines')}` : ''}</h2>
    <div className="row">
      <button className={edit ? 'pri' : ''} onClick={() => setEdit(v => !v)}>{edit ? t('gdone') : t('gedit')}</button>
      <button onClick={paste}>{t('gpaste')}</button>
      <button className="pri" onClick={save} disabled={!gcode.trim()}>{t('gsaveb')}</button>
      <button onClick={copy} disabled={!gcode}>{done ? t('copied') : t('copy')}</button>
    </div>
    {edit
      ? <textarea ref={taRef} className="gcode gcodeedit" value={gcode} spellCheck={false} placeholder={t('gempty')}
          onChange={e => setGcode(e.target.value)} />
      : <pre id="gcode" className="gcode">{lines.length ? lines.slice(0, MAX).map((l, i) => `${String(i + 1).padStart(4, ' ')}  ${l}`).join('\n') : t('gempty')}</pre>}
    {!edit && lines.length > MAX && <div className="warn">{t('gtrunc')}</div>}
    {/* 0.5.29: marker colors — ALWAYS visible (the 0.5.28 block only appeared after a
        conversion, so a fresh window looked like the feature was missing); an empty
        program shows what to do first, a 1-part program explains there is nothing to swap */}
    <div className="mkbox">
      <b className="mktitle">{t('markers')}</b>
      {!gcode.trim() ? <div className="hint">{t('mkempty')}</div>
        : parts.length >= 2 ? <>
        {parts.map(id => <div className="mkrow" key={id}>
          <label><span>{t('part' + id)}</span>
            <input style={{ flex: 1, minWidth: 60 }} value={colors[id] || ''} placeholder={t('mksame')}
              onChange={e => setColors(c => ({ ...c, [id]: e.target.value }))} /></label>
          <div className="chips">{CHIPS.map(([k, col]) => <button key={k} className="chip" style={{ background: col }}
            title={t(k)} aria-label={t(k)} onClick={() => setColors(c => ({ ...c, [id]: t(k) }))} />)}</div>
        </div>)}
        <div className="row"><button className="pri" onClick={() => apply()}>{t('mkapply')}</button>
          <button onClick={clear}>{t('mkclear')}</button>
          {pauses !== null && <b className="mkcnt">{pauses} {t('mkpauses')}</b>}</div>
        <div className="hint">{t('mkhint')}</div>
      </> : <div className="hint">{t('mknone')}</div>}
    </div></section>);
}
