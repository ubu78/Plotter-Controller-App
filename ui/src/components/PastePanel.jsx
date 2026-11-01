import { useRef, useState } from 'react';
// Ready-made G-code block: paste an existing program (Ctrl+V / clipboard button) or OPEN a file
// (.gcode/.nc/.tap/.ngc/.txt) — a picked file lands in the field AND goes straight to the G-code
// panel. A real focused textarea is the reliable path in WebKitGTK — native Ctrl+V works there
// without the async clipboard API, which the embedded view may deny.
export default function PastePanel({ t, setGcode, setLog }) {
  const [txt, setTxt] = useState('');
  const taRef = useRef(null);
  const log = m => setLog && setLog(l => [...l, m]);
  const into = (s, name) => {
    setTxt(s);
    const clean = s.replace(/\s+$/, '');
    setGcode(clean);
    log(`${t('gloaded')}: ${name} · ${clean ? clean.split('\n').length : 0} ${t('glines')}`);
  };
  const openFile = async e => {
    const f = e.target.files[0]; e.target.value = '';
    if (!f) return;
    into(await f.text(), f.name);
  };
  const fromClip = async () => {
    let s = '';
    try { s = await navigator.clipboard.readText(); } catch (e) { /* WebKitGTK may deny the read */ }
    if (!s) { // fallback: hidden textarea + execCommand('paste') (works in some WebKit builds)
      const h = document.createElement('textarea'); h.style.position = 'fixed'; h.style.opacity = '0';
      document.body.appendChild(h); h.focus();
      try { document.execCommand('paste'); } catch (e2) { /* blocked */ }
      s = h.value; h.remove();
    }
    if (!s) { // nothing came: focus the field so plain Ctrl+V lands there natively
      if (taRef.current) taRef.current.focus();
      return log(t('noclip'));
    }
    setTxt(s);
  };
  return (<section><h2>{t('readyg')}</h2>
    <label><span>{t('openf')}</span><input type="file" accept=".gcode,.ngc,.nc,.tap,.txt" onChange={openFile} /></label>
    <textarea ref={taRef} id="pasteta" className="gcode pasteta" value={txt} spellCheck={false}
      placeholder={t('pastehint')} onChange={e => setTxt(e.target.value)} />
    <div className="row">
      <button onClick={fromClip}>{t('gpasteclip')}</button>
      <button onClick={() => setTxt('')} disabled={!txt}>{t('gclear')}</button>
      <button className="pri" onClick={() => { const s = txt.replace(/\s+$/, ''); if (!s) return; setGcode(s); log(`${t('gloaded')}: ${s.split('\n').length} ${t('glines')}`); }} disabled={!txt.trim()}>{t('gapply')}</button>
    </div>
    <div className="hint">{t('gapplyhint')}</div></section>);
}
