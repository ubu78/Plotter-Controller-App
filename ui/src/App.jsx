import { useEffect, useState } from 'react';
import { T, detectLang, decorate } from './i18n';
import { api, onEvent } from './api';
import ConnectionPanel from './components/ConnectionPanel.jsx';
import ManualPanel from './components/ManualPanel.jsx';
import SourcePanel from './components/SourcePanel.jsx';
import SettingsPanel from './components/SettingsPanel.jsx';
import ConsolePanel from './components/ConsolePanel.jsx';
import GcodePanel from './components/GcodePanel.jsx';
import PastePanel from './components/PastePanel.jsx';
import PenPanel, { DEFAULT_PEN } from './components/PenPanel.jsx';
import SheetPanel from './components/SheetPanel.jsx';

export default function App() {
  const [lang, setLang] = useState(detectLang());
  const [status, setStatus] = useState({ state: '—', wpos: [0, 0, 0], progress: 0, connected: false, ov: 100, wcs: 'G54' });
  const [log, setLog] = useState([]);
  const [values, setValues] = useState({});
  const [stopped, setStopped] = useState(false);
  const [gcode, setGcode] = useState('');
  const [ver, setVer] = useState(''); const [about, setAbout] = useState(false);
  // 0.5.32: tabs — «Плоттер» (machine: connection, manual, pen, GRBL, console, the
  // G-code result and the sheet preview), «G-code» (the program sources: ready-made
  // G-code, text, SVG) and 0.5.34 «Лист» (sheet loading — «Заправить лист»). The choice
  // persists in localStorage like the language.
  const [tab, setTabS] = useState(() => { try { const v = localStorage.plottab; return v === 'gcode' || v === 'sheet' ? v : 'plotter'; } catch (e) { return 'plotter'; } });
  const setTab = t2 => { setTabS(t2); try { localStorage.plottab = t2; } catch (e) { /* storage blocked */ } };
  useEffect(() => { api('version').then(v => { if (v && v.version) setVer(v.version); }).catch(() => {}); }, []);
  const [pen, setPen] = useState(() => { try { return { ...DEFAULT_PEN, ...JSON.parse(localStorage.pen || '{}') }; } catch (e) { return DEFAULT_PEN; } });
  useEffect(() => { try { localStorage.pen = JSON.stringify(pen); } catch (e) { /* storage blocked */ } }, [pen]);
  const t = k => T[lang][k] || T.en[k] || k;
  const send = line => api('send', { line });
  // 0.5.33: the pen panel's lift command rides on Stop/Disconnect — a soft reset (0x18) kills
  // the PWM and the servo falls to rest (pen down), so the lift re-runs right after the board
  // reboots and greets us (or after $X clears an ALARM).
  const penupCmd = () => (pen.penMode === 'z' ? `G0 Z${pen.zUp}` : (pen.servoUp || 'M3 S255'));
  useEffect(() => {
    document.documentElement.lang = lang; localStorage.lang = lang;
  }, [lang]);
  useEffect(() => { api('settings').then(j => setValues(j.values)); }, []);
  // Sync the banner once on mount: even a missed push event can no longer leave a stale "Disconnected".
  useEffect(() => { api('status').then(s => { if (s && typeof s.connected === 'boolean') setStatus(s); }).catch(() => {}); }, []);
  useEffect(() => onEvent(m => {
    if (m.type === 'status') setStatus(m.status);
    if (m.type === 'log') setLog(l => [...l.slice(-400), m.line]);
    if (m.type === 'settings') setValues(m.values);
  }), []);
  const quit = async () => { if (confirm(t('quitq'))) { await api('quit', {}); setStopped(true); } };
  if (stopped) return <main><section className="wide"><h2>Plotter Controller</h2>{t('stopped')}</section></main>;
  return (<>
    <header><h1>Plotter Controller</h1>
      <a className="credit" href="https://glm.ai" target="_blank" rel="noreferrer">✦ {t('credit')}</a>
      <a className="credit rust" href="https://www.rust-lang.org" target="_blank" rel="noreferrer">🦀 Built in Rust</a>
      <button className="credit ver" title={t('about')} onClick={() => setAbout(a => !a)}>v{ver || '…'} ⓘ</button><span>{status.state}</span>
      <select value={lang} onChange={e => setLang(e.target.value)}><option value="ru">Русский</option><option value="en">English</option><option value="et">Eesti</option></select>
      <button className="stop" onClick={() => api('stop', { penup: penupCmd() })}>{t('estop')}</button>
      <button onClick={quit}>{t('quit')}</button></header>
    <main>
      {about && <div className="aboutpop" role="dialog" aria-label={t('about')}>
        <b>{t('about')}{ver && ` — v${ver}`}<button onClick={() => setAbout(false)}>✕</button></b>
        {t('abouttext').split('\n').map((l, i) => <p key={i}>{l}</p>)}
      </div>}
      {/* 0.5.32: the tab bar. Both panes stay mounted — only the CSS class switches, so the
          hidden cards keep their state and every programmatic interaction keeps working. */}
      <div className="tabbar" role="tablist">
        <button data-tab="plotter" role="tab" aria-selected={tab === 'plotter'} className={'tabbtn' + (tab === 'plotter' ? ' on' : '')} onClick={() => setTab('plotter')}>{t('tabplotter')}</button>
        <button data-tab="gcode" role="tab" aria-selected={tab === 'gcode'} className={'tabbtn' + (tab === 'gcode' ? ' on' : '')} onClick={() => setTab('gcode')}>{t('tabgcode')}</button>
        <button data-tab="sheet" role="tab" aria-selected={tab === 'sheet'} className={'tabbtn' + (tab === 'sheet' ? ' on' : '')} onClick={() => setTab('sheet')}>{t('tabsheet')}</button>
        <span className="tabhint">{t('tabhint')}</span>
      </div>
      <div className={'tabpane' + (tab === 'plotter' ? ' on' : '')} data-pane="plotter">
        <ConnectionPanel t={t} status={status} send={send} gcode={gcode} setLog={setLog} penup={penupCmd} />
        <ManualPanel t={t} send={send} pen={pen} status={status} />
        <PenPanel t={t} pen={pen} setPen={setPen} send={send} />
        <SettingsPanel t={t} lang={lang} values={values} send={send} />
        <GcodePanel t={t} gcode={gcode} setGcode={setGcode} setLog={setLog} pen={pen} />
        {/* the sheet preview card is rendered here by SourcePanel through a portal:
            the empty slot is a display:contents grid item, the section lands in the
            `prev` grid area (or flows between G-code and the console on narrow windows) */}
        <div id="prevslot" />
        <ConsolePanel t={t} log={log.map(l => decorate(lang, l))} send={send} />
      </div>
      <div className={'tabpane' + (tab === 'gcode' ? ' on' : '')} data-pane="gcode">
        <PastePanel t={t} setGcode={setGcode} setLog={setLog} />
        <SourcePanel t={t} pen={pen} status={status} setLog={setLog} onConv={r => setGcode(r.gcode || '')} goTab={setTab} />
      </div>
      <div className={'tabpane' + (tab === 'sheet' ? ' on' : '')} data-pane="sheet">
        <SheetPanel t={t} send={send} pen={pen} status={status} setLog={setLog} />
      </div>
    </main></>);
}
