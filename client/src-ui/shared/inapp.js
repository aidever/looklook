// 看看开着时的提醒（docs/FEISHU_AGENT.md §12）：页面在前台时每 20 秒告诉各台电脑一声，电脑就先不发飞书和其它渠道的通知
// （规则 fg_quiet，notify.rs），改成这里弹提醒：别的会话在等你处理、做完了、出错了。正开着的那个会话不提醒。
import { useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { deviceApi } from './api.js';
import { boardState } from './board.js';
import { useSessions } from './sessions.jsx';
import { useToast } from './ui.jsx';

const PING_MS = 20000;
// 跟电脑端“做完了”通知一样：太短的一轮不提醒
const DONE_MIN_MS = 30000;

const foreground = () => !document.hidden && document.hasFocus();

/** 页面在前台时告诉列表里的每台电脑（旧版电脑没有这个接口：出错就不再报） */
export function useForegroundPing() {
  const sessions = useSessions();
  const devs = useMemo(() => [...new Set((sessions?.items ?? []).filter((i) => !i.offline).map((i) => i.device ?? ''))].sort().join('\n'), [sessions?.items]);
  const dead = useRef(new Set());
  useEffect(() => {
    const list = devs ? devs.split('\n') : [''];
    const ping = () => {
      if (!foreground()) return;
      for (const d of list) {
        if (dead.current.has(d)) continue;
        deviceApi(d || null)
          .post('/notify/foreground')
          .catch((e) => {
            if (e?.status === 404 || e?.status === 405) dead.current.add(d);
          });
      }
    };
    ping();
    const timer = setInterval(ping, PING_MS);
    window.addEventListener('focus', ping);
    document.addEventListener('visibilitychange', ping);
    return () => {
      clearInterval(timer);
      window.removeEventListener('focus', ping);
      document.removeEventListener('visibilitychange', ping);
    };
  }, [devs]);
}

/** 别的会话在等你处理、这一轮做完 / 出错了：弹一条带“打开”的提醒。`activeId`：正开着的会话 */
export function useInAppAlerts(activeId, open) {
  const { t } = useTranslation();
  const toast = useToast();
  const sessions = useSessions();
  const seen = useRef(null);
  const openRef = useRef(open);
  openRef.current = open;
  useEffect(() => {
    const items = sessions?.items;
    if (!items) return;
    const prev = seen.current;
    const next = new Map();
    for (const i of items) {
      const c = i.chat ?? {};
      const attn = boardState(i) === 'attn' ? `${c.since ?? ''}|${c.pending?.kind ?? c.phase ?? ''}` : '';
      const end = c.turn?.ended_at ?? 0;
      next.set(i.id, { attn, end });
      const p = prev?.get(i.id);
      if (!p || i.id === activeId || !foreground()) continue;
      const title = c.title || i.name || (i.workdir ?? '').split(/[\\/]/).filter(Boolean).pop() || '';
      const action = { label: t('inapp.open'), onClick: () => openRef.current?.(i.id) };
      if (attn && attn !== p.attn) {
        toast(t('inapp.attn', { title }), 'ok', action);
      } else if (end > p.end && c.turn?.outcome === 'failed') {
        toast(t('inapp.failed', { title }), 'error', action);
      } else if (end > p.end && c.turn?.outcome === 'done' && end - (c.turn.started_at ?? end) >= DONE_MIN_MS) {
        toast(t('inapp.done', { title }), 'ok', action);
      }
    }
    seen.current = next;
  }, [sessions?.items]);
}
