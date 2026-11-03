// 页面里看大图（yet-another-react-lightbox）：缩放（滚轮、双击、双指）、拖动、左右切换、下载。对话里所有点开图片的地方都用它，
// 不开新窗口、新标签页。
import { useEffect, useMemo, useState } from 'react';
import Lightbox from 'yet-another-react-lightbox';
import Counter from 'yet-another-react-lightbox/plugins/counter';
import Download from 'yet-another-react-lightbox/plugins/download';
import Zoom from 'yet-another-react-lightbox/plugins/zoom';
import 'yet-another-react-lightbox/styles.css';
import 'yet-another-react-lightbox/plugins/counter.css';
import { fetchRaw, plainLinksWork, rtcMode, saveViaFrame } from './api.js';

/** 接口路径（`/instances/…`，不带 `/api`）：本机直接用地址，远程（经 WebRTC 转发、多台电脑）先取成 blob。其他地址原样用 */
function useSrcs(urls) {
  const api = (u) => typeof u === 'string' && u.startsWith('/') && !u.startsWith('/api/');
  const [got, setGot] = useState({});
  useEffect(() => {
    if (plainLinksWork()) return undefined;
    let alive = true;
    const made = [];
    for (const u of urls.filter(api)) {
      fetchRaw(u)
        .then((r) => r.blob())
        .then((b) => {
          const s = URL.createObjectURL(b);
          made.push(s);
          if (alive) setGot((g) => ({ ...g, [u]: s }));
        })
        .catch(() => alive && setGot((g) => ({ ...g, [u]: 'data:,' })));
    }
    return () => {
      alive = false;
      made.forEach((s) => URL.revokeObjectURL(s));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 按地址内容比较
  }, [urls.join('\n')]);
  return urls.map((u) => (!api(u) ? u : plainLinksWork() ? `/api${u}` : got[u] ?? null));
}

/**
 * `images`：[{ url, name? }]，url 是接口路径（`/instances/…`）或能直接显示的地址；`index` 先看哪张；`onClose` 关掉。
 * 有名字的能下载。
 */
export default function ImageViewer({ images, index = 0, onClose }) {
  const srcs = useSrcs(images.map((i) => i.url));
  const slides = useMemo(
    () => images.map((img, i) => ({ src: srcs[i] ?? 'data:,', alt: img.name ?? '', download: img.name ? { url: srcs[i] ?? '', filename: img.name } : false })),
    [images, srcs],
  );
  // Esc 只关看图，不让对话页拿去做别的（比如停下当前这一轮）
  useEffect(() => {
    const key = (e) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      onClose();
    };
    window.addEventListener('keydown', key, true);
    return () => window.removeEventListener('keydown', key, true);
  }, [onClose]);
  const many = images.length > 1;
  return (
    <Lightbox
      open
      close={onClose}
      index={index}
      slides={slides}
      plugins={[Zoom, Download, ...(many ? [Counter] : [])]}
      carousel={{ finite: !many, preload: 1 }}
      controller={{ closeOnBackdropClick: true, closeOnPullDown: true }}
      zoom={{ maxZoomPixelRatio: 4, scrollToZoom: true }}
      render={many ? undefined : { buttonPrev: () => null, buttonNext: () => null }}
      download={{
        // 远程打开时普通链接会变成顶层导航：经 iframe 下载（见 api.js saveViaFrame）
        download: ({ slide, saveAs }) => {
          const url = typeof slide.download === 'object' ? slide.download.url : slide.src;
          if (rtcMode && url.startsWith('/api/')) saveViaFrame(url);
          else saveAs(url, typeof slide.download === 'object' ? slide.download.filename : undefined);
        },
      }}
      styles={{ container: { backgroundColor: 'rgba(0, 0, 0, .88)' } }}
    />
  );
}
