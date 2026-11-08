// “保持屏幕常亮”（docs/REMOTE_UX_2026-11-07.md §5.2）：盯长任务时手机不锁屏，也就不会切到后台断开。
// 只在用户主动打开时生效；切到后台时系统会收回，回到前台时（开关还开着）重新申请；离开会话页时关掉。
import { useEffect, useSyncExternalStore } from 'react';

export const wakeLockSupported = typeof navigator !== 'undefined' && 'wakeLock' in navigator;

let on = false;
let sentinel = null;
const subs = new Set();
const emit = () => subs.forEach((f) => f());

async function acquire() {
  if (!on || sentinel || document.hidden) return;
  try {
    sentinel = await navigator.wakeLock.request('screen');
    sentinel.addEventListener('release', () => {
      sentinel = null;
    });
  } catch {
    // 省电模式、iframe 没有 screen-wake-lock 权限（旧启动页）：拿不到就算了
    on = false;
    emit();
  }
}

function release() {
  sentinel?.release().catch(() => {});
  sentinel = null;
}

if (wakeLockSupported) document.addEventListener('visibilitychange', () => !document.hidden && acquire());

export function setWakeLock(v) {
  if (!wakeLockSupported || on === v) return;
  on = v;
  if (v) acquire();
  else release();
  emit();
}

/** 开关的状态；`scoped`：这个组件卸载时（离开会话页）关掉。 */
export function useWakeLock(scoped = true) {
  const value = useSyncExternalStore(
    (f) => {
      subs.add(f);
      return () => subs.delete(f);
    },
    () => on,
  );
  useEffect(() => (scoped ? () => setWakeLock(false) : undefined), [scoped]);
  return value;
}
