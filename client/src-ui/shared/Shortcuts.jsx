// 快捷键说明（⌘/ / Ctrl+/，用户菜单里也能打开）：按 shortcuts.js 的注册表列出当前环境能用的全部快捷键。别处发 `ll-shortcuts` 事件打开。
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { isMod, keyLabel } from './hotkeys.js';
import { visibleGroups } from './shortcuts.js';
import { keyHintsOff, setKeyHintsOff } from './KeyHint.jsx';
import { Modal, Switch } from './ui.jsx';


const label = (combo) => combo.split(' / ').map((c) => keyLabel(c)).join(' / ');

export default function ShortcutsHelp() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [hintsOff, setHintsOff] = useState(keyHintsOff);
  useEffect(() => {
    const show = () => setOpen(true);
    const key = (e) => {
      if (isMod(e, '/')) {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    window.addEventListener('keydown', key);
    window.addEventListener('ll-shortcuts', show);
    return () => {
      window.removeEventListener('keydown', key);
      window.removeEventListener('ll-shortcuts', show);
    };
  }, []);
  return (
    <Modal open={open} onClose={() => setOpen(false)} title={t('keys.title')} wide>
      <div className="keys-help">
        {visibleGroups().map(([g, rows]) => (
          <section key={g}>
            <h3>{t(`keys.group.${g}`)}</h3>
            <dl>
              {rows.map(([k, what]) => (
                <div key={what} className="keys-row">
                  <dt>{t(`keys.${what}`)}</dt>
                  <dd>
                    <kbd>{label(k)}</kbd>
                  </dd>
                </div>
              ))}
            </dl>
          </section>
        ))}
      </div>
      <label className="keys-hints-toggle">
        <Switch checked={!hintsOff} onChange={(on) => {
          setKeyHintsOff(!on);
          setHintsOff(!on);
        }} />
        <span>
          <b>{t('keys.hints_toggle')}</b>
          <span className="small muted">{t('keys.hints_toggle_sub')}</span>
        </span>
      </label>
      <div className="modal-actions">
        <button type="button" className="btn" onClick={() => setOpen(false)}>
          {t('action.close')}
        </button>
      </div>
    </Modal>
  );
}
