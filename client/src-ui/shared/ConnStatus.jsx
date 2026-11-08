// 远程打开时唯一的连接状态提示（docs/REMOTE_UX_2026-11-07.md §4、§5）：顶上居中一个小胶囊，不挡内容、不清空页面。
// 状态来自启动页（conn.js），这里只负责按时长逐级说清楚：抖一下什么都不显示，久了说原因，再久给“重试”。
// 连回来时断开超过 3 秒才轻提示“已重新连接”；确认连接期间没发出去的终端按键给个提示。
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { clearSnapshot } from './snapshot.js';
import { connManaged, retryNow, useConn } from './conn.js';

/** 断开（重连中）多久以后才显示：心跳确认、并行建连一般 2 秒内就好了。 */
const SHOW_DROP_AFTER = 2000;
/** 第一次连接（按快照显示着内容）多久以后才显示“正在连接”。 */
const SHOW_CONNECT_AFTER = 1000;
/** 多久以后换成“暂时连不上”并给“重试”。 */
const SLOW_AFTER = 15000;
/** 断开超过这么久，连回来时提示一下。 */
const TOAST_AFTER = 3000;

function useNow(active) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return undefined;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [active]);
  return now;
}

export default function ConnStatus() {
  const { t } = useTranslation();
  const s = useConn();
  const busy = ['connecting', 'boot', 'resuming', 'reconnecting'].includes(s.phase);
  const now = useNow(busy);
  const [toast, setToast] = useState('');
  const prev = useRef(s);
  useEffect(() => {
    const p = prev.current;
    prev.current = s;
    let msg = '';
    if (s.phase === 'online' && p.phase !== 'online' && ['resuming', 'reconnecting'].includes(p.phase) && Date.now() - p.since >= TOAST_AFTER) msg = t('rlink.back');
    if (s.dropped > p.dropped) msg = t('rlink.dropped', { count: s.dropped - p.dropped });
    if (s.phase === 'auth') clearSnapshot();
    if (!msg) return undefined;
    setToast(msg);
    const id = setTimeout(() => setToast(''), msg === t('rlink.back') ? 2000 : 4500);
    return () => clearTimeout(id);
  }, [s, t]);
  if (!connManaged) return null;

  const age = now - (s.since || now);
  const name = s.device?.name;
  let text = '';
  let slow = false;
  if (s.phase === 'resuming') text = t('rlink.resuming');
  else if (s.phase === 'reconnecting' && age >= SHOW_DROP_AFTER) {
    slow = age >= SLOW_AFTER;
    const secs = Math.round(age / 1000);
    if (s.cause === 'net') text = t('rlink.net');
    else if (s.cause === 'device_offline') text = t('rlink.device_offline', { name: name || t('rlink.computer') });
    else text = slow ? t('rlink.slow', { s: secs }) : t('rlink.reconnecting', { s: secs });
  } else if ((s.phase === 'connecting' || s.phase === 'boot') && age >= SHOW_CONNECT_AFTER) {
    slow = age >= SLOW_AFTER / 2;
    text = name ? t('rlink.connecting_name', { name }) : t('rlink.connecting');
  }
  if (!text && !toast) return null;
  return (
    <div className={`conn-pill ${text ? '' : 'conn-toast'} ${slow ? 'slow' : ''}`} role="status" aria-live="polite">
      {text ? (
        <>
          <span className="conn-dots" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          <span className="conn-text">{text}</span>
          {slow && (
            <button type="button" onClick={() => retryNow()}>
              {t('rlink.retry')}
            </button>
          )}
        </>
      ) : (
        <span className="conn-text">{toast}</span>
      )}
    </div>
  );
}
