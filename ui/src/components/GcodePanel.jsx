import { useState } from 'react';
const MAX = 1500; // rendering thousands of lines makes the page sluggish; the full text stays in memory
export default function GcodePanel({ t, gcode }) {
  const [done, setDone] = useState(false);
  const lines = gcode ? gcode.split('\n') : [];
  const copy = async () => {
    try { await navigator.clipboard.writeText(gcode); }
    catch (e) { // clipboard API may be unavailable: fall back to a hidden textarea
      const ta = document.createElement('textarea'); ta.value = gcode; ta.style.position = 'fixed'; ta.style.opacity = '0';
      document.body.appendChild(ta); ta.select(); try { document.execCommand('copy'); } catch (e2) { /* give up silently */ }
      ta.remove();
    }
    setDone(true); setTimeout(() => setDone(false), 1500);
  };
  return (<section className="wide"><h2>{t('gcode')} {lines.length ? `· ${lines.length} ${t('glines')}` : ''}</h2>
    <div className="row"><button onClick={copy} disabled={!gcode}>{done ? t('copied') : t('copy')}</button></div>
    <pre id="gcode" className="gcode">{lines.length ? lines.slice(0, MAX).map((l, i) => `${String(i + 1).padStart(4, ' ')}  ${l}`).join('\n') : t('gempty')}</pre>
    {lines.length > MAX && <div className="warn">{t('gtrunc')}</div>}</section>);
}
