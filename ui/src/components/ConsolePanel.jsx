import { useEffect, useRef, useState } from 'react';
export default function ConsolePanel({ t, log, send }) {
  const [cmd, setCmd] = useState(''); const box = useRef(); const hist = useRef({ a: [], i: 0 });
  useEffect(() => { box.current.scrollTop = box.current.scrollHeight; }, [log]);
  const go = () => { if (!cmd) return; send(cmd); hist.current.a.push(cmd); hist.current.i = hist.current.a.length; setCmd(''); };
  const key = e => { const h = hist.current;
    if (e.key === 'Enter') go();
    if (e.key === 'ArrowUp' && h.i > 0) setCmd(h.a[--h.i]);
    if (e.key === 'ArrowDown' && h.i < h.a.length - 1) setCmd(h.a[++h.i]); };
  return (<section className="wide p-console"><h2>{t('console')}</h2><div id="log" ref={box}>{log.join('\n')}</div>
    <div className="row"><input style={{ flex: 1 }} value={cmd} placeholder="G-code / $$ / $X" onChange={e => setCmd(e.target.value)} onKeyDown={key} /><button onClick={go}>{t('send')}</button></div></section>);
}
