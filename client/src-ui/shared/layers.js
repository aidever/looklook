// 层栈与返回键（docs/REMOTE_UX_2026-11-07.md §2.2、§7、§8.1）。
//
// 弹窗、底部菜单、全屏预览、看大图、多选模式这些“层”打开时压一条历史记录（地址不变，state 里带层的 id）：
// - 手机的返回键 / 返回手势、浏览器后退、鼠标侧键：关掉最上面那一层，不会一下退出页面；
// - 用 ×、Esc、点遮罩关掉时，自己把压进去的那条用掉（history.back），历史记录里不留空层；
// - 层里点链接去了别的页面：不去动历史（那条已经不在当前位置）。
// 前进键落到一条已经关掉的层上：把它换成普通记录（相当于空按一次），不重新打开。
//
// 另外两件和历史记录有关的事也在这里：从链接直接打开深层页面时垫一条首页（返回回到首页而不是离开网站），
// 桌面 App（没有浏览器的前进 / 后退按钮）的 ⌘[ / ⌘]、Alt+← / →、鼠标侧键。
import { useEffect, useRef } from 'react';

import { IS_MAC } from './hotkeys.js';

const stack = []; // { id, close }
let seq = 0;
/** 自己调 history.back() 用掉层的记录时，接下来的 popstate 不当成用户按了返回。 */
let consuming = 0;

const stateOf = () => window.history.state || {};

function onPop() {
  if (consuming > 0) {
    consuming--;
    return;
  }
  const want = stateOf().llLayer;
  const keep = want ? stack.findIndex((l) => l.id === want) : -1;
  // 返回：关掉当前记录以上的层（一般就是最上面那一层；长按返回选了更早的记录时一次关几层）
  while (stack.length - 1 > keep) {
    const top = stack.pop();
    top.popped = true;
    top.close();
  }
  // 前进 / 返回落到一条已经关掉的层上：换成普通记录
  if (want && keep < 0) {
    const { llLayer, ...rest } = stateOf();
    void llLayer;
    window.history.replaceState(Object.keys(rest).length ? rest : null, '');
  }
}

if (typeof window !== 'undefined') window.addEventListener('popstate', onPop);

/**
 * `open` 为真时这一层算打开着：压一条历史记录，返回键关掉它（调 `onClose`）。
 * `history: false`：不进历史（桌面上的下拉菜单这类点别处就关的）。
 */
export function useLayer(open, onClose, { history = true } = {}) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    if (!open || !history) return undefined;
    const me = { id: `l${++seq}`, close: () => close.current?.(), popped: false };
    stack.push(me);
    window.history.pushState({ ...stateOf(), llLayer: me.id }, '');
    return () => {
      const i = stack.indexOf(me);
      if (i >= 0) stack.splice(i, 1);
      // 不是返回键关的（×、Esc、点遮罩、程序关掉）：当前记录还是这一层的就用掉它
      if (!me.popped && stateOf().llLayer === me.id) {
        consuming++;
        window.history.back();
      }
    };
  }, [open, history]);
}

/** 现在有没有打开着的层（返回键会先关它）。 */
export const hasLayer = () => stack.length > 0;

/** 触屏（没有鼠标悬停）：浮层在这里通常是整屏或底部弹出的，返回键应该先关它；桌面上的小浮层点别处就关，不进历史。 */
export const touchUi = typeof window !== 'undefined' && Boolean(window.matchMedia?.('(hover: none) and (pointer: coarse)').matches);

/**
 * 从链接直接打开深层页面（飞书通知里的会话链接等）：先把这条记录换成首页、再压回目标页，
 * 这样返回回到首页，而不是离开网站（§7.2 最后一行）。每个标签页只做一次（刷新不再垫）。
 */
export function baseDeepLink(isDeep) {
  try {
    if (sessionStorage.getItem('ll-based')) return;
    sessionStorage.setItem('ll-based', '1');
    const nav = performance.getEntriesByType?.('navigation')?.[0];
    if (nav && nav.type !== 'navigate') return;
    const { pathname, search, hash } = window.location;
    if (!isDeep(pathname) || window.history.state) return;
    window.history.replaceState(null, '', '/');
    window.history.pushState(null, '', pathname + search + hash);
  } catch {
    /* 隐私模式等拿不到 sessionStorage：不垫 */
  }
}

export const isDesktopApp = (() => {
  try {
    return Boolean(window.__LOOKLOOK_DESKTOP__ || window.top.__LOOKLOOK_DESKTOP__);
  } catch {
    return Boolean(window.__LOOKLOOK_DESKTOP__);
  }
})();

/** 焦点在终端里（iframe）或者输入框里：Alt+← / → 是按词移动，不当成后退。 */
function editing(e) {
  const el = e.target;
  if (!el || el === document.body) return false;
  return el.tagName === 'IFRAME' || el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable;
}

/**
 * 桌面 App 的前进 / 后退（浏览器里有自己的按钮和快捷键，不重复处理）：mac ⌘[ / ⌘]，其他 Alt+← / →，鼠标侧键。
 * 终端里的按键由 term-inject.js 发来 looklook:nav（它知道哪些键该留给终端）。
 */
export function installDesktopNav() {
  if (!isDesktopApp) return () => {};
  const go = (dir) => (dir < 0 ? window.history.back() : window.history.forward());
  const key = (e) => {
    if (IS_MAC && e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey && (e.key === '[' || e.key === ']')) {
      e.preventDefault();
      return go(e.key === '[' ? -1 : 1);
    }
    if (!IS_MAC && e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight') && !editing(e)) {
      e.preventDefault();
      go(e.key === 'ArrowLeft' ? -1 : 1);
    }
  };
  const mouse = (e) => {
    if (e.button !== 3 && e.button !== 4) return;
    e.preventDefault();
    go(e.button === 3 ? -1 : 1);
  };
  const msg = (e) => {
    if (e.data?.type !== 'looklook:nav') return;
    if (![...document.querySelectorAll('iframe')].some((f) => f.contentWindow === e.source)) return;
    go(e.data.dir < 0 ? -1 : 1);
  };
  window.addEventListener('keydown', key);
  window.addEventListener('mouseup', mouse);
  window.addEventListener('message', msg);
  return () => {
    window.removeEventListener('keydown', key);
    window.removeEventListener('mouseup', mouse);
    window.removeEventListener('message', msg);
  };
}
