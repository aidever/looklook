// 快捷键小提示：常用的地方用一行小字随机提一个当下用得上的快捷键，帮用户顺手学会。
// 不抢视线：颜色很淡、不动画闪烁、几分钟才换一个；只在电脑上（有键盘）显示；
// 用户自己按过两次的就算学会了，不再提；全都学会了就什么都不显示。快捷键说明（⌘/）里能整个关掉。
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { IS_MAC, keyLabel } from './hotkeys.js';
import { isTouch } from './KeyBar.jsx';

const USED_KEY = 'll-keys-used';
const OFF_KEY = 'll-key-hints-off';
/** 按过几次算学会 */
const LEARNED = 2;
/** 多久换一个 */
const ROTATE_MS = 4 * 60 * 1000;

/**
 * 能提示的快捷键：id → [组合键, 说明的翻译键（keys.*）, 只在这里面按的才算（不带修饰键的普通键）]。
 * 组合键写法同 shortcuts.js（mod = ⌘ / Ctrl，Ctrl = 就是 Ctrl）。说明沿用快捷键说明页的文字，两边说法一致。
 */
export const HINTS = {
  quick: ['mod+K', 'quick'],
  new: ['mod+shift+O', 'new'],
  help: ['mod+/', 'help'],
  pane: ['mod+shift+S', 'pane'],
  side_tab: ['mod+shift+1', 'side_tab'],
  settings: ['mod+,', 'settings'],
  files_page: ['mod+shift+F', 'files_page'],
  find: ['mod+F', 'find'],
  jump: ['alt+↑', 'jump'],
  history: ['↑', 'history', '.chat-composer'],
  history_search: ['Ctrl+R', 'history_search'],
  mode: ['shift+Tab', 'mode', '.chat-composer'],
  newline: ['shift+Enter', 'newline', '.chat-composer'],
  stop: ['Esc', 'stop', '.chat-composer'],
  now: ['mod+Enter', 'send_now'],
};

const KEYS = { '↑': 'arrowup', '↓': 'arrowdown', esc: 'escape', space: ' ' };

/** '组合键' → 判断一次按键是不是它 */
function matcher(combo, within) {
  const parts = combo.toLowerCase().split('+');
  const key = KEYS[parts.at(-1)] ?? parts.at(-1);
  const want = { mod: parts.includes('mod'), ctrl: parts.includes('ctrl'), shift: parts.includes('shift'), alt: parts.includes('alt') };
  return (e) => {
    const k = (e.key || '').toLowerCase();
    const digit = /^digit(\d)$/i.exec(e.code || '')?.[1];
    if (k !== key && digit !== key) return false;
    if (want.mod && !(IS_MAC ? e.metaKey : e.ctrlKey)) return false;
    if (want.ctrl && !e.ctrlKey) return false;
    if (!want.mod && !want.ctrl && (e.ctrlKey || e.metaKey)) return false;
    if (e.shiftKey !== want.shift || e.altKey !== want.alt) return false;
    return !within || Boolean(e.target?.closest?.(within));
  };
}
const MATCH = Object.fromEntries(Object.entries(HINTS).map(([id, [combo, , within]]) => [id, matcher(combo, within)]));

function readUsed() {
  try {
    const v = JSON.parse(localStorage.getItem(USED_KEY) || '{}');
    return v && typeof v === 'object' ? v : {};
  } catch {
    return {};
  }
}

export function keyHintsOff() {
  try {
    return localStorage.getItem(OFF_KEY) === '1';
  } catch {
    return false;
  }
}

export function setKeyHintsOff(off) {
  try {
    if (off) localStorage.setItem(OFF_KEY, '1');
    else localStorage.removeItem(OFF_KEY);
  } catch {
    /* 记不住就只在这次打开里有效 */
  }
  window.dispatchEvent(new Event('ll-key-hints'));
}

// 全局记一份按过哪些（整个页面只挂一次）：按键在捕获阶段看，不拦也不改
let watching = false;
function watchKeys() {
  if (watching || typeof window === 'undefined') return;
  watching = true;
  window.addEventListener(
    'keydown',
    (e) => {
      if (e.repeat) return;
      const hit = Object.keys(MATCH).filter((id) => MATCH[id](e));
      if (!hit.length) return;
      const used = readUsed();
      for (const id of hit) used[id] = Math.min(99, (used[id] ?? 0) + 1);
      try {
        localStorage.setItem(USED_KEY, JSON.stringify(used));
      } catch {
        /* 只是少了“学会了就不提” */
      }
    },
    true,
  );
}

/**
 * 一行小字的快捷键提示。`ids`：这里用得上的（按 HINTS 的 id，顺序无所谓），会随场景变（比如运行中只提 Esc）。
 * 点一下打开快捷键说明。手机、平板上不显示。
 */
export default function KeyHint({ ids, className = '' }) {
  const { t } = useTranslation();
  const [off, setOff] = useState(keyHintsOff);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    watchKeys();
    const on = () => setOff(keyHintsOff());
    window.addEventListener('ll-key-hints', on);
    const timer = setInterval(() => setTick((n) => n + 1), ROTATE_MS);
    return () => {
      window.removeEventListener('ll-key-hints', on);
      clearInterval(timer);
    };
  }, []);
  const sig = ids.join(',');
  // 场景变了、到时间了才重新挑：同一场景里不会每次渲染都跳
  const pick = useMemo(() => {
    const used = readUsed();
    const left = ids.filter((id) => HINTS[id] && (used[id] ?? 0) < LEARNED);
    return left.length ? left[Math.floor(Math.random() * left.length)] : null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sig, tick]);
  if (off || !pick || isTouch()) return null;
  const [combo, what] = HINTS[pick];
  return (
    <button type="button" key={`${pick}:${tick}`} className={`keyhint ${className}`} onClick={() => window.dispatchEvent(new Event('ll-shortcuts'))} title={t('keys.hint_more')} tabIndex={-1}>
      <kbd>{keyLabel(combo)}</kbd>
      <span className="ellipsis">{t(`keys.${what}`)}</span>
    </button>
  );
}
