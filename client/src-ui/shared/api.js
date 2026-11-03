// 本机接口：同源、JSON；写请求带 X-Looklook-Client（避开平台保留的 X-LL-*）。错误统一抛出 ApiError（code + params）。

export class ApiError extends Error {
  constructor(status, code, params = {}) {
    super(code);
    this.status = status;
    this.code = code;
    this.params = params;
  }
}

// ---------------- 多设备（远程入口，服务端 docs/05 §6、§9） ----------------
// 远程打开且账户有多台电脑在线时，接口请求带 X-LL-Device 指定发给哪台电脑：
// 指定了 device 的请求发给那台；/instances/{id}… 发给这个终端所在的电脑；其余发给顶栏选中的电脑。
// 本机/局域网打开、或只有一台电脑时不带这个头，行为与单设备完全一样。
const routing = { multi: false, selected: null, devices: [], owners: new Map(), locating: new Map() };
const INSTANCE_PATH = /^\/instances\/([a-z0-9]+)(?:[/?]|$)/;

/** 由界面（设备列表加载后、切换设备时）设置。 */
export function configureDevices({ multi, selected, devices }) {
  routing.multi = Boolean(multi);
  routing.selected = multi ? selected ?? null : null;
  routing.devices = devices ?? [];
}

/** 记下终端所在的电脑（终端页按电脑加载列表后调用）。 */
export function rememberTerminals(device, items) {
  for (const i of items ?? []) if (i?.id) routing.owners.set(i.id, device);
}

/** 当前选中的电脑（不分设备时为 null）。 */
export function selectedDevice() {
  return routing.multi ? routing.selected : null;
}

export function terminalDevice(id) {
  return routing.owners.get(id) ?? null;
}

// 不知道终端在哪台电脑（例如直接打开 /t/{id}）：问一遍在线的电脑。
function locate(id) {
  if (!routing.locating.has(id)) {
    const online = routing.devices.filter((d) => d.online);
    routing.locating.set(
      id,
      Promise.all(online.map((d) => request('GET', '/instances', undefined, { device: d.id }).then((r) => rememberTerminals(d.id, r?.items), () => {}))).then(() => {
        routing.locating.delete(id);
        return routing.owners.get(id) ?? null;
      }),
    );
  }
  return routing.locating.get(id);
}

async function deviceFor(path, options) {
  if (!routing.multi) return null;
  if (options.device !== undefined) return options.device;
  const m = path.match(INSTANCE_PATH);
  if (m) return routing.owners.get(m[1]) ?? (await locate(m[1])) ?? routing.selected;
  return routing.selected;
}

/** 设备列表 `{items:[{id,name,os,online}], default, max}`：远程打开时由启动页所在的平台应答，本机 App 由这台电脑向平台取
 *  （default 是本机）；拿不到（旧服务端、网络）时为 null。 */
export async function fetchDevices() {
  try {
    const res = await fetch(rtcMode ? '/_ll/devices' : '/api/rtc/devices', { headers: { Accept: 'application/json' }, credentials: 'same-origin' });
    if (!res.ok) return null;
    const d = await res.json();
    if (!Array.isArray(d?.items)) return null;
    const items = d.items.filter((x) => x && typeof x.id === 'string').map((x) => ({ id: x.id, name: x.name || '', os: x.os || '', online: x.online !== false }));
    // local_only：本机 App 的“只看本机”（远程打开时没有这个字段）
    return { items, default: d.default ?? null, max: d.max ?? null, localOnly: typeof d.local_only === 'boolean' ? d.local_only : null,
      // 只看本机时同一局域网里在线的其他电脑（点“直接打开”才连）
      lan: Array.isArray(d.lan) ? d.lan.filter((x) => x && typeof x.id === 'string').map((x) => ({ id: x.id, name: x.name || '', os: x.os || '', online: true })) : [] };
  } catch {
    return null;
  }
}

async function request(method, path, body, options = {}) {
  const headers = { Accept: 'application/json' };
  if (method !== 'GET') headers['X-Looklook-Client'] = '1';
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const device = await deviceFor(path, options);
  if (device) headers['X-LL-Device'] = device;
  let res;
  try {
    // no-store：浏览器（Chrome 的缓存锁）会让同一网址的并发 GET 排队等前一个返回。多台电脑时各台的
    // `/api/instances` 网址相同、只是 X-LL-Device 不同，一台卡住会让其他电脑的请求也一起等。
    res = await fetch(`/api${path}`, { signal: options.signal, method, headers, credentials: 'same-origin', cache: 'no-store', body: body === undefined ? undefined : JSON.stringify(body) });
  } catch (e) {
    if (e.name === 'AbortError') throw e;
    throw new ApiError(0, 'CLIENT_UNREACHABLE');
  }
  if (res.status === 204) return null;
  const data = await res.json().catch(() => null);
  if (!res.ok) {
    const e = data?.error ?? {};
    if (res.status === 401 && e.code === 'UNAUTHENTICATED') reloadForSession();
    throw new ApiError(res.status, e.code ?? 'INTERNAL', e.params ?? {});
  }
  return data;
}

// 远程打开时，平台或中转的子站会话（含 1 天的会话凭证）到期会返回 UNAUTHENTICATED（本机接口不返回这个码）。
// 刷新页面即重新走一次授权跳转；30 秒内最多一次，避免授权失败时反复刷新。
function reloadForSession() {
  const key = 'looklook.session-reload';
  try {
    const last = Number(sessionStorage.getItem(key) || 0);
    if (Date.now() - last < 30_000) return;
    sessionStorage.setItem(key, String(Date.now()));
  } catch {
    return;
  }
  window.location.reload();
}

/**
 * 读文件内容（预览、下载）用：带上多设备的 X-LL-Device，返回原始 Response；出错时抛 ApiError。
 * 普通链接（<a href> / <img src>）带不了请求头，远程有多台电脑时只有 `/api/instances/{id}/…` 能按终端转发，
 * 其他路径要用这个取回再显示或保存。
 */
export async function fetchRaw(path, options = {}) {
  const headers = {};
  const device = await deviceFor(path, options);
  if (device) headers['X-LL-Device'] = device;
  let res;
  try {
    // cache：网址带版本、内容不会变的（宠物动画）可以用 'default'，走浏览器缓存
    res = await fetch(`/api${path}`, { signal: options.signal, headers, credentials: 'same-origin', cache: options.cache ?? 'no-store' });
  } catch (e) {
    if (e.name === 'AbortError') throw e;
    throw new ApiError(0, 'CLIENT_UNREACHABLE');
  }
  if (!res.ok) {
    const e = (await res.json().catch(() => null))?.error ?? {};
    throw new ApiError(res.status, e.code ?? 'INTERNAL', e.params ?? {});
  }
  return res;
}

/** 请求体是文件本身（导入宠物包）：返回 JSON，出错抛 ApiError */
export async function postFile(path, file, options = {}) {
  const headers = { Accept: 'application/json', 'X-Looklook-Client': '1', 'Content-Type': 'application/octet-stream' };
  const device = await deviceFor(path, options);
  if (device) headers['X-LL-Device'] = device;
  let res;
  try {
    res = await fetch(`/api${path}`, { method: 'POST', headers, credentials: 'same-origin', cache: 'no-store', body: file });
  } catch {
    throw new ApiError(0, 'CLIENT_UNREACHABLE');
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) {
    const e = data?.error ?? {};
    throw new ApiError(res.status, e.code ?? (res.status === 413 ? 'TOO_LARGE' : 'INTERNAL'), e.params ?? {});
  }
  return data;
}

/** 普通链接能不能直接打开不属于终端的接口（远程且有多台电脑时不能，见 fetchRaw）。 */
export const plainLinksWork = () => !routing.multi;

/**
 * 远程管理台经 WebRTC 打开（服务端 docs/06）：顶层是平台的启动页，管理台在它的同源 iframe 里，
 * 请求经 Service Worker 转给启动页、再经 DataChannel 发给电脑。
 */
export const rtcMode = (() => {
  try {
    return window.top !== window && Boolean(window.top.__llrtc);
  } catch {
    return false;
  }
})();

/**
 * 在隐藏的 iframe 里打开下载地址：浏览器边收边存（响应带 Content-Disposition: attachment），不占页面内存。
 * WebRTC 模式下 iframe 的请求同样经 Service Worker 转发，`device` 用 `__lldev` 指定电脑（启动页会去掉这个参数）。
 */
export function saveViaFrame(href, device) {
  const url = new URL(href, location.href);
  if (device) url.searchParams.set('__lldev', device);
  const f = Object.assign(document.createElement('iframe'), { src: url.pathname + url.search, hidden: true });
  f.setAttribute('aria-hidden', 'true');
  document.body.appendChild(f);
  // 下载开始后 iframe 就没用了；大文件要传很久，留足时间再删（删了会中断还没收完的响应）
  setTimeout(() => f.remove(), 6 * 3600_000);
}

/**
 * WebRTC 模式下的下载链接（`<a download>`，含代码里 `a.click()` 的）：浏览器可能把它当成顶层导航直接交给平台，
 * 拿到的是启动页。改成在隐藏的 iframe 里打开（见 [`saveViaFrame`]）。以前是取回存成 blob 再保存，
 * 大文件要整个放在内存里，手机上常常存不下。
 */
export function installRtcDownloads() {
  if (!rtcMode) return;
  document.addEventListener(
    'click',
    (e) => {
      const a = e.target instanceof Element ? e.target.closest('a[download]') : null;
      if (!a || e.defaultPrevented) return;
      const url = new URL(a.href, location.href);
      // blob: 链接（这里自己生成的、fetchRaw 取回后保存的）照常下载
      if (url.protocol === 'blob:' || url.origin !== location.origin) return;
      e.preventDefault();
      saveViaFrame(url.href);
    },
    true,
  );
}

function client(device) {
  const o = device === undefined ? {} : { device };
  return {
    get: (p, options) => request('GET', p, undefined, { ...options, ...o }),
    post: (p, b) => request('POST', p, b ?? {}, o),
    put: (p, b) => request('PUT', p, b ?? {}, o),
    patch: (p, b) => request('PATCH', p, b ?? {}, o),
    del: (p) => request('DELETE', p, undefined, o),
  };
}

export const api = client();
/** 只发给本机、不按选中的电脑转发的接口（“只看本机”开关）。 */
export const localApi = client(null);

/** 发给指定电脑的接口（多设备时；device 为空时同 `api`）。 */
export function deviceApi(device) {
  return device ? client(device) : api;
}
