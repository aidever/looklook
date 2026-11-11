/**
 * 电脑那边换了版本（后台自动更新、手动升级后重启），这个页面还在跑旧版的界面代码：
 * 旧代码会照旧版的逻辑发请求（例如 1.73.0 一打开会话就把对话交给终端），所以要换成新版界面。
 *
 * 每台电脑记第一次看到的版本，之后同一台报的版本变了就算过时。刷新会丢掉输入框里没发的字，
 * 所以只在页面在后台、或者没在打字（没有输入框拿着焦点、也没有写了一半的内容）时刷新；
 * 正在打字就等切到后台、切回来或者输入框空下来再刷新。
 */
const seen = new Map();
let stale = false;

function typing() {
  const el = document.activeElement;
  if (el && (el.isContentEditable || el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && !['button', 'checkbox', 'radio'].includes(el.type)))) {
    return true;
  }
  return [...document.querySelectorAll('textarea')].some((t) => t.value.trim() !== '');
}

function tryReload() {
  if (!stale) return;
  if (document.hidden || !typing()) window.location.reload();
}

/** 每次拿到 /status 时调用：`device` 是这份状态来自的电脑（单机为 null） */
export function noteVersion(device, version) {
  if (!version) return;
  const key = device ?? '';
  const was = seen.get(key);
  if (was === undefined) {
    seen.set(key, version);
    return;
  }
  if (was === version) {
    tryReload();
    return;
  }
  if (!stale) {
    stale = true;
    document.addEventListener('visibilitychange', tryReload);
    document.addEventListener('focusout', () => setTimeout(tryReload, 0));
  }
  tryReload();
}
