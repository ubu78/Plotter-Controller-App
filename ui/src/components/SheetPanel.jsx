import { useEffect, useRef, useState } from 'react';
import { api } from '../api';

// 0.5.34: «Заправить лист» — the sheet enters from the +Y side, where the guide rails sit.
// Start lifts the pen and rolls Y toward zero with chained relative jogs; Stop cancels the
// jog in real time (0x85) and the stop point becomes the sheet origin — X/Y of the ACTIVE
// work coordinate system are zeroed automatically (same G10 L20 semantics as the pad «0»).
// The segment loop is driven by the pushed status reports: every next jog goes out only
// after the previous one was really seen in the «Jog» state and really moved the axis, so
// soft-limit rejections, alarms, disconnects and resets end the cycle instead of stacking
// up more motion.
const SEGMENT = 200; // mm per jog — long enough to roll freely, short enough to stop precisely
const GRACE = 900;   // ms — a jog that never enters «Jog» within this window was rejected

export default function SheetPanel({ t, send, pen, status, setLog }) {
  const [feed, setFeed] = useState(() => { try { const v = +localStorage.sheetfeed; return v > 0 ? v : 2000; } catch (e) { return 2000; } });
  useEffect(() => { try { localStorage.sheetfeed = String(feed); } catch (e) { /* storage blocked */ } }, [feed]);
  const [phase, setPhase] = useState('idle'); // idle | run | stop (stopping, waiting for the board)
  const run = useRef(null); // { stopping, sawJog, sentAt, lastY } — alive while the feed cycle runs
  const penCmd = up => send(pen.penMode === 'z' ? `G0 Z${up ? pen.zUp : pen.zDown}` : up ? pen.servoUp : pen.servoDown);
  const log = line => setLog(l => [...l.slice(-400), line]);
  const zero = () => { // active WCS index: G54..G59 -> P1..P6 (the board reports it via [GC:...])
    const v = parseInt(status.wcs?.slice(1), 10);
    const p = v >= 54 && v <= 59 ? v - 54 + 1 : 1;
    send(`G10 L20 P${p} X0 Y0`);
  };
  const end = msg => { run.current = null; setPhase('idle'); if (msg) log(`; ${msg}`); };

  useEffect(() => { // the feed cycle: chain the next jog when the previous one really moved
    const r = run.current;
    if (!r) return;
    const st = status.state, y = status.wpos ? (status.wpos[1] || 0) : 0;
    if (!status.connected || st === 'Alarm' || st === 'Hold') return end(t('feedhalt'));
    if (st === 'Jog') { r.sawJog = true; return; }
    if (st !== 'Idle') return; // Connecting/Run — wait
    if (r.stopping) { run.current = null; setPhase('idle'); zero(); log(`; ${t('feedzero')}`); return; }
    if (!r.sawJog) { if (Date.now() - r.sentAt > GRACE) end(t('feedwarn')); return; }
    if (Math.abs(y - r.lastY) < 0.05) return end(t('feedwarn')); // the axis stood still
    r.lastY = y; r.sawJog = false; r.sentAt = Date.now();
    send(`$J=G91 G21 Y-${SEGMENT} F${feed}`);
  }, [status]);

  const start = () => {
    if (phase !== 'idle' || !status.connected || status.state !== 'Idle') return;
    run.current = { stopping: false, sawJog: false, sentAt: Date.now(), lastY: status.wpos ? (status.wpos[1] || 0) : 0 };
    setPhase('run');
    penCmd(true); // the pen must not scratch while the sheet rolls under it
    log(`; ${t('feedrun')}`);
    send(`$J=G91 G21 Y-${SEGMENT} F${feed}`);
  };
  const stop = () => {
    if (!run.current || run.current.stopping) return;
    run.current.stopping = true; setPhase('stop');
    api('jogcancel', {}); // realtime 0x85 — GRBL cancels the jog where it stands
  };
  const canStart = phase === 'idle' && status.connected && status.state === 'Idle';
  return (<section className="p-sheet" data-phase={phase}><h2>{t('sheetcard')}</h2>
    <p className="hint">{t('sheethint')}</p>
    <div className="row"><button className="pri" disabled={!canStart} onClick={start}>{t('feedstart')}</button>
      <button className="stop" disabled={phase === 'idle'} onClick={stop}>{t('feedstop')}</button></div>
    <label><span>{t('feed')}</span><input type="number" min="1" value={feed} onChange={e => setFeed(Math.max(1, +e.target.value || 1))} /></label>
    {phase !== 'idle' && <div className="feedstat">{phase === 'run' ? t('feedrun') : t('feedstopmsg')}</div>}
  </section>);
}
