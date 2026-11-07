import { useEffect, useState } from 'react';
import { api } from '../api';
import { settingName } from '../i18n';
const UNITS = { 0: 'µs', 1: 'ms', 11: 'mm', 12: 'mm', 24: 'mm/min', 25: 'mm/min', 26: 'ms', 27: 'mm', 30: 'RPM', 31: 'RPM' };
const unit = id => UNITS[id] || (id >= 130 ? 'mm' : id >= 120 ? 'mm/s²' : id >= 110 ? 'mm/min' : id >= 100 ? 'steps/mm' : '');
export default function SettingsPanel({ t, lang, values, send }) {
  const [edit, setEdit] = useState({}); const [preset, setPreset] = useState({});
  useEffect(() => { api('settings').then(j => setPreset(j.preset)); }, []);
  useEffect(() => setEdit({}), [values]);
  const ids = Object.keys(values).sort((a, b) => a - b);
  const write = () => Object.entries(edit).forEach(([id, v]) => v !== values[id] && send(`$${id}=${v}`));
  return (<section className="wide p-set"><h2>{t('sets')}</h2>
    <div className="row"><button onClick={() => send('$$')}>{t('rd')}</button><button className="pri" onClick={write}>{t('wall')}</button>
      <button onClick={() => setEdit(e => ({ ...e, ...Object.fromEntries(Object.entries(preset).filter(([k]) => k in values)) }))}>{t('preset')}</button></div>
    <div className="sets">{ids.map(id => <label key={id}><span style={{ minWidth: 150 }}>${id} {settingName(lang, id)}</span>
      <input style={{ width: 80 }} value={edit[id] ?? values[id]} onChange={e => setEdit({ ...edit, [id]: e.target.value })} /> <small>{unit(id)}</small></label>)}</div></section>);
}
