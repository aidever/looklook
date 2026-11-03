// 宠物素材：取成 blob 再播（不依赖 Range、Service Worker、X-LL-Device，远程、iOS 都一样），按大小留一份缓存。
import { fetchRaw } from '../api.js';

const LIMIT = 80 << 20;
/** path → { promise, size, at } */
const cache = new Map();
let total = 0;

function evict() {
  if (total <= LIMIT) return;
  const old = [...cache.entries()].filter(([, v]) => v.size).sort((a, b) => a[1].at - b[1].at);
  for (const [k, v] of old) {
    if (total <= LIMIT * 0.7) break;
    cache.delete(k);
    total -= v.size;
    v.promise.then((u) => URL.revokeObjectURL(u), () => {});
  }
}

/** 取一个文件，返回 blob: 地址（同一个地址只取一次） */
export function loadBlob(path) {
  const hit = cache.get(path);
  if (hit) {
    hit.at = Date.now();
    return hit.promise;
  }
  const entry = { size: 0, at: Date.now(), promise: null };
  entry.promise = fetchRaw(path, { cache: 'default' })
    .then((r) => r.blob())
    .then((b) => {
      entry.size = b.size;
      total += b.size;
      evict();
      return URL.createObjectURL(b);
    })
    .catch((e) => {
      cache.delete(path);
      throw e;
    });
  cache.set(path, entry);
  return entry.promise;
}

let fmt;
/**
 * 这个浏览器播哪种透明视频：Chromium（含 Edge WebView2、安卓）用 webm（VP9 带透明），
 * Safari / iOS / macOS App（WKWebView）用 mov（HEVC 带透明），Firefox 两种都不行（null，只显示封面）。
 */
export function videoFormat() {
  if (fmt !== undefined) return fmt;
  const ua = navigator.userAgent;
  const ios = /iPad|iPhone|iPod/.test(ua) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
  if (ios) fmt = 'mov';
  else if (/Chrome\/|Chromium\/|Edg\//.test(ua)) fmt = 'webm';
  else if (/Firefox\//.test(ua)) fmt = null;
  else if (/AppleWebKit/.test(ua)) fmt = 'mov';
  else fmt = 'webm';
  return fmt;
}

/** 人物在画面里的框 [x, y, w, h]（0~1）：按透明像素算。`el` 是已经能画的 <img> / <video>；`crop` 是只量其中一块（精灵图的第一格） */
export function measureBox(el, crop) {
  try {
    const w = 160;
    const [cx, cy, sw, sh] = crop ?? [0, 0, el.videoWidth || el.naturalWidth, el.videoHeight || el.naturalHeight];
    if (!sw || !sh) return null;
    const h = Math.max(1, Math.round((w * sh) / sw));
    const c = document.createElement('canvas');
    c.width = w;
    c.height = h;
    const g = c.getContext('2d', { willReadFrequently: true });
    g.drawImage(el, cx, cy, sw, sh, 0, 0, w, h);
    const d = g.getImageData(0, 0, w, h).data;
    let x0 = w;
    let y0 = h;
    let x1 = -1;
    let y1 = -1;
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        if (d[(y * w + x) * 4 + 3] > 40) {
          if (x < x0) x0 = x;
          if (x > x1) x1 = x;
          if (y < y0) y0 = y;
          if (y > y1) y1 = y;
        }
      }
    }
    if (x1 < 0) return null;
    return [x0 / w, y0 / h, (x1 - x0 + 1) / w, (y1 - y0 + 1) / h];
  } catch {
    return null;
  }
}
