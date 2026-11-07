import { useRef, useState } from 'react';
import { api } from '../api';
const MAX = 1500; // rendering thousands of lines makes the page sluggish; the full text stays in memory
export default function GcodePanel({ t, gcode, setGcode, setLog }) {
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
    {!edit && lines.length > MAX && <div className="warn">{t('gtrunc')}</div>}</section>);
}
