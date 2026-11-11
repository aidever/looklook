// 快捷键注册表（docs/REMOTE_UX_2026-11-07.md §2.4、§8.2）：说明页（Shortcuts.jsx）由它生成，新加的全局快捷键也在这里处理。
// `combo`：mod = ⌘（Mac）/ Ctrl（其他），`app`：只在桌面 App 里有（浏览器自己占着这个组合），`mac`：只在 Mac 上列出。
//
// 终端获得焦点时（按键在终端的 iframe 里）：Mac 上 ⌘ 组合终端用不到，由 term-inject.js 转给管理台；
// Windows / Linux 上 Ctrl+K、Ctrl+W、Ctrl+R、Ctrl+F 都是终端和命令行里的键，一律留给终端，管理台只接 Ctrl+Shift 开头的组合
// （Ctrl+Shift+K = 快速切换，Ctrl+Shift+/ = 快捷键说明……），见 TERM_REMAP。
import { useEffect } from 'react';

import { refreshAll } from './conn.js';
import { IS_MAC } from './hotkeys.js';
import { isDesktopApp } from './layers.js';
import { useRouter } from './router.jsx';

export const GROUPS = [
  ['app', [
    ['mod+K', 'quick'],
    ['mod+shift+O', 'new'],
    ['mod+shift+S', 'pane'],
    ['mod+shift+1 / 2 / 3 / 4', 'side_tab'],
    ['mod+shift+F', 'files_page'],
    ['mod+W', 'close_tab'],
    ['mod+,', 'settings'],
    ['mod+/', 'help'],
    ['Esc', 'close_layer'],
  ]],
  ['nav', [
    [IS_MAC ? 'mod+[ / mod+]' : 'alt+← / alt+→', 'back_forward', { app: true }],
    ['mod+R', 'refresh', { app: true }],
  ]],
  ['chat', [['Enter', 'send'], ['shift+Enter', 'newline'], ['mod+Enter', 'send_now'], ['mod+F', 'find'], ['alt+↑ / alt+↓', 'jump'], ['↑ / ↓', 'history'], ['Ctrl+R', 'history_search'], ['Tab', 'suggest'], ['shift+Tab', 'mode'], ['Esc', 'stop']]],
  ['files', [
    ['↑ / ↓', 'fb_move'],
    ['Enter', 'fb_open'],
    ['Backspace / alt+↑', 'fb_up'],
    ['Space', 'fb_peek'],
    ['mod+A', 'fb_all'],
    ['F2', 'fb_rename'],
    ['Delete', 'fb_delete'],
    ['mod+shift+C', 'fb_copy_path'],
    ['/', 'fb_filter'],
    ['mod+P', 'fb_search'],
  ]],
  ['terminal', [[IS_MAC ? 'mod+K / mod+/ …' : 'Ctrl+Shift+K / Ctrl+Shift+/ …', 'term_rule']]],
];

/** 当前环境能用的（桌面 App 专属的在浏览器里不列）。 */
export function visibleGroups() {
  return GROUPS.map(([g, rows]) => [g, rows.filter(([, , o]) => !o?.app || isDesktopApp)]).filter(([, rows]) => rows.length);
}

const TABS = ['chat', 'term', 'git', 'files'];

/** 按下的是不是 mod+shift+数字（按物理键算：Shift+1 在不同键盘上 e.key 不一样）。返回 1–9 或 0。 */
function modShiftDigit(e) {
  const mod = IS_MAC ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || !e.shiftKey || e.altKey) return 0;
  const m = /^Digit([1-9])$/.exec(e.code || '');
  return m ? Number(m[1]) : 0;
}

const isModShift = (e, code) => (IS_MAC ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey) && e.shiftKey && !e.altKey && e.code === code;

/** 管理台的全局快捷键（新加的这几个；原有的 ⌘K、⌘, 等仍由各自的组件处理）。 */
export function useGlobalShortcuts() {
  const { navigate } = useRouter();
  useEffect(() => {
    const on = (e) => {
      const n = modShiftDigit(e);
      if (n >= 1 && n <= 4) {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent('ll-side-tab', { detail: TABS[n - 1] }));
        return;
      }
      if (isModShift(e, 'KeyF')) {
        e.preventDefault();
        navigate('/files');
        return;
      }
      // 桌面 App：⌘R / Ctrl+R / F5 是重新连接 + 重新加载（浏览器里就是浏览器的刷新）
      if (isDesktopApp && (e.key === 'F5' || ((IS_MAC ? e.metaKey : e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'r'))) {
        e.preventDefault();
        refreshAll();
      }
    };
    window.addEventListener('keydown', on);
    return () => window.removeEventListener('keydown', on);
  }, [navigate]);
  // 终端里按的（term-inject.js 转来的 looklook:hotkey）：在管理台上重放一次，各处的处理照常生效
  useEffect(() => {
    const on = (e) => {
      const d = e.data;
      if (d?.type !== 'looklook:hotkey') return;
      if (![...document.querySelectorAll('iframe')].some((f) => f.contentWindow === e.source)) return;
      window.dispatchEvent(new KeyboardEvent('keydown', { key: d.key, code: d.code, metaKey: d.meta, ctrlKey: d.ctrl, shiftKey: d.shift, altKey: d.alt, bubbles: true, cancelable: true }));
    };
    window.addEventListener('message', on);
    return () => window.removeEventListener('message', on);
  }, []);
}
