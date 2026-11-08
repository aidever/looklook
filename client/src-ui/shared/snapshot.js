// 远程打开时的本地快照（docs/REMOTE_UX_2026-11-07.md §2.3、§6）：上次的状态、电脑列表、会话列表（只有标题、状态这类元数据），
// 下次打开时不等连上电脑就按它显示界面，连上后静默换成最新的。不存对话正文、终端输出、文件内容。
// 退出登录、登录过期（看到 logged_in 为假）时清掉。只在远程（WebRTC）模式用：本机、局域网打开本来就快。
import { rtcMode } from './api.js';

const KEY = 'll-snap-v1';
/** 快照最多用这么久（太旧了不如等最新的）。 */
const MAX_AGE = 14 * 86400_000;

let cache;

function read() {
  if (cache !== undefined) return cache;
  cache = null;
  if (!rtcMode) return cache;
  try {
    const v = JSON.parse(localStorage.getItem(KEY) || 'null');
    if (v && typeof v === 'object' && Date.now() - (v.at || 0) < MAX_AGE && v.status?.account?.logged_in) cache = v;
  } catch {
    /* 坏了就当没有 */
  }
  return cache;
}

let timer = 0;
function save() {
  if (!rtcMode || timer) return;
  timer = setTimeout(() => {
    timer = 0;
    try {
      if (cache) localStorage.setItem(KEY, JSON.stringify({ ...cache, at: Date.now() }));
    } catch {
      /* 存满了就算了 */
    }
  }, 1000);
}

/** 有没有快照（启动页据此决定能不能马上收起骨架）。 */
export const hasSnapshot = () => Boolean(read());

/** 上次的 /status（标了 `_snap`，界面据此知道是旧数据）。 */
export function snapStatus() {
  const v = read();
  return v?.status ? { ...v.status, _snap: true } : undefined;
}

/** 上次的电脑列表（`fetchDevices` 的格式）。 */
export function snapDevices() {
  return read()?.devices ?? null;
}

/** 上次某台电脑（单台时为 'self'）的会话列表。 */
export function snapInstances(dev) {
  const items = read()?.instances?.[dev ?? 'self'];
  return Array.isArray(items) ? items : null;
}

function ensure() {
  if (!cache) cache = { status: null, devices: null, instances: {} };
  return cache;
}

export function rememberStatus(s) {
  if (!rtcMode || !s) return;
  if (!s.account?.logged_in) return clearSnapshot();
  // 只留界面需要的元数据；用量、诊断这类会变的、和显示无关的不存
  const { metrics, relay_check, pid, ...keep } = s;
  void metrics;
  void relay_check;
  void pid;
  read();
  ensure().status = keep;
  save();
}

export function rememberDevices(d) {
  if (!rtcMode || !d) return;
  read();
  ensure().devices = d;
  save();
}

export function rememberInstances(dev, items) {
  if (!rtcMode || !Array.isArray(items)) return;
  read();
  const c = ensure();
  // 会话列表本来就只有元数据（标题、目录、状态、类型），没有对话内容
  c.instances = { ...c.instances, [dev ?? 'self']: items.slice(0, 200) };
  save();
}

export function clearSnapshot() {
  cache = null;
  clearTimeout(timer);
  timer = 0;
  try {
    localStorage.removeItem(KEY);
  } catch {
    /* 忽略 */
  }
}
