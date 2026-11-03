// 清理后台会话（电脑端 idle_stop.rs）：列出在后台跑着的会话，很久没打开的在前。默认勾上没在忙、现在没开着的，
// 用户看过、确认后停掉。停掉只结束 tmux 会话和里面的程序，会话留着，点开就重新启动（AI 会话接着原来的对话）。
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { TriangleAlert } from 'lucide-react';

import { api } from './api.js';
import { LaunchIcon } from './brands.jsx';
import { errorText } from './i18n.js';
import { Button, ErrorNote, Modal, Note, Spinner, useAction, useToast } from './ui.jsx';

/** `keep`：现在开着的会话（标签里的），默认不勾 */
export function IdleCleanupDialog({ onClose, keep = [], onDone }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [data, setData] = useState(null);
  const [picked, setPicked] = useState(new Set());
  const [sure, setSure] = useState(false);
  const { busy, error, run } = useAction();
  useEffect(() => {
    let alive = true;
    api.get('/instances/idle').then(
      (r) => {
        if (!alive) return;
        const items = Array.isArray(r?.items) ? r.items : [];
        setData({ items, days: r?.days ?? 0 });
        setPicked(new Set(items.filter((i) => !i.busy && !keep.includes(i.id)).map((i) => i.id)));
      },
      (e) => alive && setData({ error: e }),
    );
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 打开时取一次
  }, []);
  const items = data?.items ?? [];
  const busyPicked = useMemo(() => items.filter((i) => i.busy && picked.has(i.id)).length, [items, picked]);
  const toggle = (id) =>
    setPicked((p) => {
      const n = new Set(p);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  const stop = () =>
    run(async () => {
      const r = await api.post('/instances/idle', { ids: [...picked] });
      toast(t('idle.stopped', { n: r?.stopped ?? 0 }));
      onDone?.();
      onClose();
    });
  const ago = (ms) => {
    const d = Math.floor((Date.now() - ms) / 86400e3);
    if (d >= 1) return t('idle.days_ago', { n: d });
    const h = Math.floor((Date.now() - ms) / 3600e3);
    return h >= 1 ? t('idle.hours_ago', { n: h }) : t('idle.just_now');
  };
  return (
    <Modal open onClose={onClose} title={t('idle.title')} className="idle-dlg">
      <p className="muted small">{t('idle.intro')}</p>
      {data === null ? (
        <Spinner />
      ) : data.error ? (
        <p className="error">{errorText(t, data.error)}</p>
      ) : !items.length ? (
        <p className="muted">{t('idle.none')}</p>
      ) : (
        <>
          <div className="idle-head">
            <span className="grow">{t('idle.count', { n: items.length })}</span>
            <button type="button" className="btn btn-plain btn-sm" onClick={() => setPicked(new Set(items.filter((i) => !i.busy).map((i) => i.id)))}>
              {t('idle.pick_idle')}
            </button>
            <button type="button" className="btn btn-plain btn-sm" onClick={() => setPicked(new Set())}>
              {t('idle.pick_none')}
            </button>
          </div>
          <div className="idle-list">
            {items.map((i) => (
              <label key={i.id} className="idle-row">
                <input type="checkbox" checked={picked.has(i.id)} onChange={() => toggle(i.id)} />
                <LaunchIcon launch={i.tool || i.launch} size={16} />
                <span className="idle-text">
                  <span className="ellipsis">{i.title || i.name}</span>
                  <span className="idle-sub ellipsis">
                    {keep.includes(i.id) ? t('idle.open_now') : t('idle.opened', { when: ago(i.opened_at) })}
                    {i.busy && <b className="idle-busy"> · {i.fg ? t('idle.busy_cmd', { cmd: i.fg }) : t('idle.busy')}</b>}
                  </span>
                </span>
              </label>
            ))}
          </div>
          {busyPicked > 0 && (
            <Note kind="warn" icon={TriangleAlert}>
              {t('idle.busy_warn', { n: busyPicked })}
            </Note>
          )}
          <label className="idle-sure">
            <input type="checkbox" checked={sure} onChange={(e) => setSure(e.target.checked)} />
            {t('idle.sure', { n: picked.size })}
          </label>
        </>
      )}
      <p className="muted small">{data?.days ? t('idle.auto_on', { n: data.days }) : t('idle.auto_off')}</p>
      <ErrorNote error={error} />
      <div className="modal-actions">
        <Button variant="ghost" onClick={onClose}>
          {t('action.cancel')}
        </Button>
        <Button variant="danger" busy={busy} disabled={!sure || picked.size === 0} onClick={stop}>
          {t('idle.stop', { n: picked.size })}
        </Button>
      </div>
    </Modal>
  );
}
