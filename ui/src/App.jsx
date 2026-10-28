import { useEffect, useState } from 'react';
import { T, detectLang, decorate } from './i18n';
import { api, onEvent } from './api';
import ConnectionPanel from './components/ConnectionPanel.jsx';
import ManualPanel from './components/ManualPanel.jsx';
import SourcePanel from './components/SourcePanel.jsx';
import SettingsPanel from './components/SettingsPanel.jsx';
import ConsolePanel from './components/ConsolePanel.jsx';
import GcodePanel from './components/GcodePanel.jsx';
import PenPanel, { DEFAULT_PEN } from './components/PenPanel.jsx';

export default function App() {
  const [lang, setLang] = useState(detectLang());
  const [status, setStatus] = useState({ state: '—', wpos: [0, 0, 0], progress: 0, connected: false });
  const [log, setLog] = useState([]);
  const [values, setValues] = useState({});
  const [stopped, setStopped] = useState(false);
  const [gcode, setGcode] = useState('');
  const [pen, setPen] = useState(() => { try { return { ...DEFAULT_PEN, ...JSON.parse(localStorage.pen || '{}') }; } catch (e) { return DEFAULT_PEN; } });
  useEffect(() => { try { localStorage.pen = JSON.stringify(pen); } catch (e) { /* storage blocked */ } }, [pen]);
  const t = k => T[lang][k] || T.en[k] || k;
  const send = line => api('send', { line });
  useEffect(() => {
    document.documentElement.lang = lang; localStorage.lang = lang;
  }, [lang]);
  useEffect(() => { api('settings').then(j => setValues(j.values)); }, []);
  useEffect(() => onEvent(m => {
    if (m.type === 'status') setStatus(m.status);
    if (m.type === 'log') setLog(l => [...l.slice(-400), m.line]);
    if (m.type === 'settings') setValues(m.values);
  }), []);
  const quit = async () => { if (confirm(t('quitq'))) { await api('quit', {}); setStopped(true); } };
  if (stopped) return <main><section className="wide"><h2>Plotter Controller</h2>{t('stopped')}</section></main>;
  return (<>
    <header><h1>Plotter Controller</h1>
      <a className="credit" href="https://claude.ai" target="_blank" rel="noreferrer">✦ {t('credit')}</a>
      <a className="credit rust" href="https://www.rust-lang.org" target="_blank" rel="noreferrer">🦀 Built in Rust</a><span>{status.state}</span>
      <select value={lang} onChange={e => setLang(e.target.value)}><option value="ru">Русский</option><option value="en">English</option><option value="et">Eesti</option></select>
      <button className="stop" onClick={() => api('stop', {})}>{t('estop')}</button>
      <button onClick={quit}>{t('quit')}</button></header>
    <main>
      <ConnectionPanel t={t} status={status} send={send} />
      <ManualPanel t={t} send={send} pen={pen} />
      <PenPanel t={t} pen={pen} setPen={setPen} />
      <SourcePanel t={t} pen={pen} status={status} setLog={setLog} onConv={r => setGcode(r.gcode || '')} />
      <SettingsPanel t={t} lang={lang} values={values} send={send} />
      <GcodePanel t={t} gcode={gcode} />
      <ConsolePanel t={t} log={log.map(l => decorate(lang, l))} send={send} />
    </main></>);
}
