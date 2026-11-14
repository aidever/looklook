// 手机上的下拉刷新：浏览器自带的在这里常常用不了（页面是固定高度、加到主屏幕的网页没有），自己做一个，所有页面一样。
// - 列表页：页面和手指下面能滚动的区域都在最上面时，往下拉超过一段距离、松开就刷新整页；
// - 终端页（整页终端 .term-page）不处理：终端正常时下拉是翻看终端；连不上、正在连接时由终端里注入的脚本处理，
//   它发来 looklook:pull-refresh，这里刷新整页（term-inject.js 的 setupPullRefresh）。
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { refreshAll } from './conn.js';
import { isTouch } from './KeyBar.jsx';

const LIMIT = 72;
// 手指先动多少才判断方向（在这之前不显示、不算距离，免得点一下、轻微抖动就冒出提示）
const SLOP = 10;

/** 手指下面、往上能滚动的区域（不在最上面就不算下拉刷新）。 */
function scrolledAbove(el) {
  for (let n = el; n && n !== document.body && n !== document.documentElement; n = n.parentElement) {
    if (n.scrollTop > 0) {
      const oy = getComputedStyle(n).overflowY;
      if (oy === 'auto' || oy === 'scroll') return true;
    }
  }
  return window.scrollY > 0;
}

/** 手指下面的东西自己处理拖动（touch-action 不许上下平移：宠物、分隔条、按键条、通知卡片……），这时下拉是在拖它，不是刷新。 */
function ownsDrag(el) {
  for (let n = el; n && n.nodeType === 1 && n !== document.body; n = n.parentElement) {
    if (n.draggable) return true;
    const ta = getComputedStyle(n).touchAction;
    if (ta && ta !== 'auto' && ta !== 'manipulation' && !ta.includes('pan-y')) return true;
  }
  return false;
}

export default function PullRefresh() {
  const { t } = useTranslation();
  const [dist, setDist] = useState(0);
  const [busy, setBusy] = useState(false);
  // { x, y：按下的位置；el：手指下面的元素；pull：已判定是往下拉（null = 还没动够 SLOP） }
  const start = useRef(null);
  const distRef = useRef(0);
  useEffect(() => {
    const onMsg = (e) => {
      if (e.data?.type !== 'looklook:pull-refresh') return;
      if (![...document.querySelectorAll('iframe')].some((f) => f.contentWindow === e.source)) return;
      refreshAll();
    };
    window.addEventListener('message', onMsg);
    if (!isTouch()) return () => window.removeEventListener('message', onMsg);
    const set = (v) => {
      distRef.current = v;
      setDist(v);
    };
    const cancel = () => {
      start.current = null;
      if (distRef.current) set(0);
    };
    const down = (e) => {
      start.current = null;
      if (e.touches.length !== 1) return cancel();
      const el = e.target;
      // 终端页、侧栏、弹窗、菜单、抽屉、输入框、宠物里不处理；底部弹出的菜单开着时（往下划是关菜单）整页都不处理
      if (document.querySelector('.menu-pop-sheet')) return;
      const skip = '.term-page, .side.open, .modal-backdrop, .menu-pop-list, .menu-pop-backdrop, .drawer-backdrop, .chat-sheet-back, '
        + '.files-panel, input, textarea, select, [contenteditable="true"], .keybar, .pet-layer, .pet-menu-backdrop';
      if (el.closest?.(skip) || ownsDrag(el) || scrolledAbove(el)) return;
      // 正在选字（拖选区的把手）时不算
      if (String(window.getSelection?.() ?? '')) return;
      start.current = { x: e.touches[0].clientX, y: e.touches[0].clientY, el, pull: null };
    };
    const move = (e) => {
      const s = start.current;
      if (!s) return;
      if (e.touches.length !== 1) return cancel(); // 两指缩放
      const dx = e.touches[0].clientX - s.x;
      const dy = e.touches[0].clientY - s.y;
      if (s.pull === null) {
        if (Math.hypot(dx, dy) < SLOP) return;
        // 先往上划（翻列表）、横着划（左右滑的标签条、轮播）：这一下都不算下拉
        if (dy <= 0 || Math.abs(dx) > dy) return cancel();
        s.pull = true;
        s.y += SLOP; // 从判定那一刻起算距离，提示不会一下子跳出来
      }
      // 拉的过程中下面的区域被滚动了（例如先往下拉又往上推）：不刷新
      if (scrolledAbove(s.el)) return cancel();
      const d = e.touches[0].clientY - s.y;
      set(d > 0 ? Math.min(d * 0.55, LIMIT * 1.4) : 0);
    };
    const up = () => {
      const go = start.current?.pull && distRef.current >= LIMIT;
      start.current = null;
      set(0);
      if (!go) return;
      // 远程时交给启动页：先确认 / 重建连接再重载（只重载这一层的话，连接坏了刷新也没用）。
      // 重载前显示“正在刷新…”，松手后有反馈；万一没重载（例如启动页判断不用），一会儿自己消失
      setBusy(true);
      setTimeout(() => setBusy(false), 6000);
      refreshAll();
    };
    document.addEventListener('touchstart', down, { passive: true });
    document.addEventListener('touchmove', move, { passive: true });
    document.addEventListener('touchend', up, { passive: true });
    document.addEventListener('touchcancel', cancel, { passive: true });
    return () => {
      window.removeEventListener('message', onMsg);
      document.removeEventListener('touchstart', down);
      document.removeEventListener('touchmove', move);
      document.removeEventListener('touchend', up);
      document.removeEventListener('touchcancel', cancel);
    };
  }, []);
  if (busy) {
    return (
      <div className="pull" style={{ transform: `translate(-50%, ${LIMIT / 2}px)` }} role="status">
        <span className="pull-arrow pull-spin" aria-hidden="true">
          ↻
        </span>
        {t('pull.refreshing')}
      </div>
    );
  }
  if (dist < 8) return null;
  const ready = dist >= LIMIT;
  return (
    <div className="pull" style={{ transform: `translate(-50%, ${dist / 2}px)`, opacity: Math.min(1, dist / LIMIT) }} role="status">
      <span className={`pull-arrow ${ready ? 'ready' : ''}`} aria-hidden="true">
        ↓
      </span>
      {t(ready ? 'pull.release' : 'pull.pull')}
    </div>
  );
}
