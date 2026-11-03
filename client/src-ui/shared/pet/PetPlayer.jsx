// 宠物播放器（docs/PET_PACK.md §4、§6.4）：对话页角落和设置页“试播”共用。
// 视频：两个 <video> 叠着交替，一个在播、一个准备下一段；精灵图：一个 <div> 按帧切背景。
// 位置、走动、拖拽甩抛直接改 style（每帧都动，不走 React 渲染）。
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
import { createBrain, pickLine } from './brain.js';
import { loadBlob, measureBox, videoFormat } from './media.js';

const DEFAULT_BOX = [0.3, 0.15, 0.4, 0.7];
const PAD = 6;
const GRAVITY = 2400;

const reducedMotion = () => {
  try {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  } catch {
    return false;
  }
};

/**
 * manifest：换算后的清单；fileUrl(key)：文件的接口路径；agent：{ state, once, n }（n 变了才算一次新的信号）；
 * height：人物多高（像素）；lines：显示气泡；lang：界面语言；label：读屏用的名字；
 * storeKey：记住横向位置用的 localStorage 键（试播时不传）。
 */
const PetPlayer = forwardRef(function PetPlayer({ manifest: m, fileUrl, agent, height = 130, lines = true, lang = 'zh-CN', label, storeKey, onState, children }, ref) {
  const layer = useRef(null);
  const wrap = useRef(null);
  const vids = [useRef(null), useRef(null)];
  const front = useRef(0);
  const sprite = useRef(null);
  const brainRef = useRef(null);
  const token = useRef(0);
  const motion = useRef(null);
  const pos = useRef({ x: null, y: 0 });
  const timers = useRef({ frame: 0, bubble: 0 });
  const [box, setBox] = useState(m.stage?.box ?? null);
  const [bubble, setBubble] = useState(null);
  const [faded, setFaded] = useState(false);
  const [boxSize, setBoxSize] = useState({ w: 0, h: 0 });
  const isSprite = m.renderer === 'sprite';
  const fmt = isSprite ? 'sprite' : videoFormat();
  const still = useMemo(reducedMotion, []);
  // 这个浏览器能不能动：视频要有对应格式的文件
  const canMove = !still && (isSprite || (fmt && Object.values(m.clips).some((c) => c[fmt])));

  const b = box ?? DEFAULT_BOX;
  const stageW = m.stage?.width || 640;
  const stageH = m.stage?.height || 360;
  // 画面高度：让人物（框的高）正好是 height
  const H = Math.round(Math.min(560, Math.max(60, height / Math.max(0.1, b[3]))));
  const W = Math.round((H * stageW) / stageH);
  const scale = H / stageH;
  const ground = m.stage?.ground ?? 0.85;
  const charW = b[2] * W;

  // ---------------- 位置 ----------------
  const bounds = useCallback(() => {
    const cw = layer.current?.clientWidth ?? 0;
    const ch = layer.current?.clientHeight ?? 0;
    // y 是脚离地面多高：头顶（脚底往上 (ground - 框顶) × H）不出容器
    return { minX: charW / 2 + PAD, maxX: Math.max(charW / 2 + PAD, cw - charW / 2 - PAD), maxY: Math.max(0, ch - PAD * 2 - (ground - b[1]) * H) };
  }, [charW, H, b, ground]);

  const place = useCallback(() => {
    const el = wrap.current;
    if (!el || pos.current.x == null) return;
    const { minX, maxX, maxY } = bounds();
    const p = pos.current;
    p.x = Math.min(maxX, Math.max(minX, p.x));
    p.y = Math.min(maxY, Math.max(0, p.y));
    // 人物中心对准 x，脚底线对准 地面（容器底边往上 PAD）+ y
    const cx = b[0] + b[2] / 2;
    el.style.transform = `translate(${Math.round(p.x - cx * W)}px, ${Math.round(-(PAD + p.y) + (1 - ground) * H)}px)`;
  }, [bounds, b, W, H, ground]);

  const room = useCallback(() => {
    const { minX, maxX } = bounds();
    const x = pos.current.x ?? maxX;
    return { left: Math.max(0, (x - minX) / scale), right: Math.max(0, (maxX - x) / scale) };
  }, [bounds, scale]);

  const savePos = useCallback(() => {
    if (!storeKey || pos.current.x == null) return;
    const cw = layer.current?.clientWidth;
    if (!cw) return;
    try {
      localStorage.setItem(storeKey, String(Math.round((pos.current.x / cw) * 1000) / 1000));
    } catch {
      /* 记不住就算了 */
    }
  }, [storeKey]);

  // 容器尺寸变了（窗口、侧栏）：重新放一下
  useEffect(() => {
    const el = layer.current;
    if (!el) return undefined;
    const ro = new ResizeObserver(() => {
      setBoxSize({ w: el.clientWidth, h: el.clientHeight });
      if (pos.current.x == null && el.clientWidth) {
        let r = null;
        try {
          r = storeKey ? Number(localStorage.getItem(storeKey)) : null;
        } catch {
          r = null;
        }
        pos.current.x = r > 0 && r < 1 ? r * el.clientWidth : el.clientWidth - charW / 2 - 24;
      }
      place();
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [place, storeKey, charW]);
  useEffect(place, [place]);

  // ---------------- 播放 ----------------
  const say = useCallback(
    (key) => {
      if (!lines || !key) return;
      const text = pickLine(m.lines, lang, key);
      if (!text) return;
      clearTimeout(timers.current.bubble);
      setBubble({ text, n: Date.now() });
      timers.current.bubble = setTimeout(() => setBubble(null), 3200);
    },
    [lines, m.lines, lang],
  );

  const stopMotion = () => {
    if (motion.current) cancelAnimationFrame(motion.current.raf);
    motion.current = null;
  };

  /** 走动：视频在 [lead, 时长 - tail] 里匀速走完；循环的段（精灵图）按 pace 走，走完才算这段结束 */
  const startWalk = (mv, durSec, onDone) => {
    stopMotion();
    const x0 = pos.current.x;
    const dx = mv.dx * scale;
    const t0 = performance.now();
    const span = Math.max(0.2, durSec - mv.lead - mv.tail);
    const pace = Math.max(10, mv.pace * scale);
    const m0 = { raf: 0 };
    const step = (now) => {
      const t = (now - t0) / 1000;
      let k;
      if (mv.looped) k = Math.min(1, (t * pace) / Math.max(1, Math.abs(dx)));
      else k = Math.min(1, Math.max(0, (t - mv.lead) / span));
      pos.current.x = x0 + dx * k;
      place();
      if (k >= 1) {
        motion.current = null;
        savePos();
        if (mv.looped) onDone();
        return;
      }
      m0.raf = requestAnimationFrame(step);
    };
    m0.raf = requestAnimationFrame(step);
    motion.current = m0;
  };

  const next = useRef(null);
  const show = useCallback(
    async (p) => {
      if (!p || !canMove) return;
      const my = ++token.current;
      const c = m.clips[p.clip];
      if (!c) return;
      if (p.line) say(p.line);
      if (isSprite) {
        clearTimeout(timers.current.frame);
        const el = sprite.current;
        if (!el) return;
        const sp = m.sprite;
        let url;
        try {
          url = await loadBlob(fileUrl(sp.image));
        } catch {
          return;
        }
        if (my !== token.current) return;
        const [cw, chh] = sp.cell ?? [192, 208];
        el.style.backgroundImage = `url("${url}")`;
        el.style.backgroundSize = `${(sp.columns ?? 8) * cw * scale}px auto`;
        el.style.setProperty('--mirror', p.mirror ? '-1' : '1');
        const frames = c.frames ?? 1;
        const durs = c.durations?.length ? c.durations : null;
        const fps = c.fps ?? m.stage?.fps ?? 8;
        let i = 0;
        const total = durs ? durs.reduce((a, x) => a + x, 0) / 1000 : frames / fps;
        if (p.move) startWalk(p.move, total / (c.speed ?? 1), () => next.current?.());
        const tick = () => {
          if (my !== token.current) return;
          el.style.backgroundPosition = `${-i * cw * scale}px ${-(c.row ?? 0) * chh * scale}px`;
          const d = (durs ? durs[i % durs.length] : 1000 / fps) / (c.speed ?? 1);
          i++;
          if (i > frames) {
            // 循环的走动段：走完之前接着循环
            if (p.move?.looped && motion.current) {
              i = 0;
              timers.current.frame = setTimeout(tick, 0);
              return;
            }
            next.current?.();
            return;
          }
          timers.current.frame = setTimeout(tick, d);
        };
        tick();
        return;
      }
      const key = c[fmt];
      if (!key) return;
      let url;
      try {
        url = await loadBlob(fileUrl(key));
      } catch {
        return;
      }
      if (my !== token.current) return;
      const backIdx = 1 - front.current;
      const back = vids[backIdx].current;
      const cur = vids[front.current].current;
      if (!back || !cur) return;
      back.style.setProperty('--mirror', p.mirror ? '-1' : '1');
      if (back.dataset.src !== url) {
        back.src = url;
        back.dataset.src = url;
      }
      back.currentTime = 0;
      back.playbackRate = c.speed ?? 1;
      try {
        await back.play();
      } catch {
        /* 页面在后台、被省电模式拦了：等回来再播 */
      }
      if (my !== token.current) {
        back.pause();
        return;
      }
      // 新的一段叠在上面；要淡入时旧的那段等它显出来再撤
      back.style.transition = p.fade ? '' : 'none';
      back.style.zIndex = '2';
      cur.style.zIndex = '1';
      back.classList.add('on');
      front.current = backIdx;
      setTimeout(() => {
        if (front.current !== backIdx) return;
        cur.classList.remove('on');
        cur.pause();
      }, p.fade ? 170 : 0);
      if (p.move) {
        const dur = (back.duration || 3) / (c.speed ?? 1);
        startWalk(p.move, dur, () => {});
      } else if (!motion.current?.drag) {
        stopMotion();
      }
    },
    [canMove, m, fmt, fileUrl, scale, isSprite, say, place],
  );

  const now = () => performance.now();
  next.current = () => {
    const br = brainRef.current;
    if (!br) return;
    const p = br.ended(now(), room());
    if (p) show(p);
    else setFaded(br.state.agent === 'offline' && !br.hasOffline());
  };

  // 大脑：清单变了就换一个
  useEffect(() => {
    const br = createBrain(m, { now: now() });
    brainRef.current = br;
    token.current++;
    setFaded(false);
    if (agent?.state) br.setAgent(agent.state, null, now(), null);
    // 等容器量好尺寸再开始
    const id = setTimeout(() => show(br.start(now(), room())), 30);
    return () => {
      clearTimeout(id);
      clearTimeout(timers.current.frame);
      stopMotion();
      token.current++;
    };
  }, [m]);

  // agent 信号
  useEffect(() => {
    const br = brainRef.current;
    if (!br || !agent) return;
    const p = br.setAgent(agent.state, agent.once, now(), room());
    setFaded(agent.state === 'offline' && !br.hasOffline());
    if (p) show(p);
    else if (!canMove && agent.once) say(agent.once);
    onState?.(agent.state);
  }, [agent?.n]);

  // 人物框：清单没写就按封面（或第一段的第一帧）的透明像素算
  useEffect(() => {
    if (m.stage?.box) {
      setBox(m.stage.box);
      return undefined;
    }
    let dead = false;
    // 精灵图量第一格（待机的第一帧）；视频量封面，没有封面就量第一段的第一帧
    const sheet = isSprite ? m.sprite?.image : null;
    const src = sheet ?? m.preview ?? Object.values(m.clips).find((c) => c.webm)?.webm;
    if (!src) return undefined;
    const idleRow = isSprite ? m.clips[m.states?.idle?.[0]?.clip]?.row ?? 0 : 0;
    loadBlob(fileUrl(src))
      .then((url) => {
        if (dead) return;
        const isImg = src === m.preview || src === sheet;
        const el = isImg ? new Image() : document.createElement('video');
        const done = () => {
          const [cw, ch] = m.sprite?.cell ?? [192, 208];
          const r = measureBox(el, sheet ? [0, idleRow * ch, cw, ch] : undefined);
          if (!dead && r) setBox(r);
        };
        if (isImg) el.onload = done;
        else {
          el.muted = true;
          el.onloadeddata = done;
        }
        el.src = url;
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [m]);

  // 页面在后台、标签切走：暂停
  useEffect(() => {
    const onVis = () => {
      const v = vids[front.current].current;
      if (!v) return;
      if (document.hidden) v.pause();
      else if (!faded) v.play().catch(() => {});
    };
    document.addEventListener('visibilitychange', onVis);
    return () => document.removeEventListener('visibilitychange', onVis);
  }, [faded]);
  useEffect(() => {
    const v = vids[front.current].current;
    if (faded) v?.pause();
  }, [faded]);

  // ---------------- 点、拖、甩 ----------------
  const drag = useRef(null);
  const onPointerDown = (e) => {
    if (e.button !== 0 && e.pointerType === 'mouse') return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    drag.current = { id: e.pointerId, sx: e.clientX, sy: e.clientY, x0: pos.current.x, y0: pos.current.y, moved: false, samples: [{ x: e.clientX, y: e.clientY, t: now() }] };
  };
  const onPointerMove = (e) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) return;
    const dx = e.clientX - d.sx;
    const dy = e.clientY - d.sy;
    if (!d.moved && Math.hypot(dx, dy) < 6) return;
    if (!d.moved) {
      d.moved = true;
      stopMotion();
      motion.current = { raf: 0, drag: true };
      const p = brainRef.current?.dragStart(now());
      if (p) show(p);
    }
    pos.current.x = d.x0 + dx;
    pos.current.y = d.y0 - dy;
    place();
    d.samples.push({ x: e.clientX, y: e.clientY, t: now() });
    if (d.samples.length > 6) d.samples.shift();
  };
  const onPointerUp = (e) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) return;
    drag.current = null;
    if (!d.moved) {
      const p = brainRef.current?.click(now());
      if (p) show(p);
      return;
    }
    // 松手：按最后一小段的速度甩出去，碰边反弹、落地
    const a = d.samples[0];
    const z = d.samples[d.samples.length - 1];
    const dt = Math.max(0.016, (z.t - a.t) / 1000);
    let vx = Math.max(-3000, Math.min(3000, (z.x - a.x) / dt));
    let vy = Math.max(-3000, Math.min(3000, -(z.y - a.y) / dt));
    let last = now();
    const m0 = { raf: 0, drag: true };
    const step = (t) => {
      const s = Math.min(0.05, (t - last) / 1000);
      last = t;
      const { minX, maxX, maxY } = bounds();
      const p = pos.current;
      vy -= GRAVITY * s;
      p.x += vx * s;
      p.y += vy * s;
      if (p.x < minX) {
        p.x = minX;
        vx = -vx * 0.6;
      } else if (p.x > maxX) {
        p.x = maxX;
        vx = -vx * 0.6;
      }
      if (p.y > maxY) {
        p.y = maxY;
        vy = -Math.abs(vy) * 0.5;
      }
      if (p.y <= 0) {
        p.y = 0;
        vy = Math.abs(vy) > 260 ? Math.abs(vy) * 0.35 : 0;
        vx *= Math.exp(-8 * s);
      }
      place();
      if (p.y <= 0 && vy === 0 && Math.abs(vx) < 25) {
        motion.current = null;
        savePos();
        const q = brainRef.current?.dragEnd(now(), room());
        if (q) show(q);
        return;
      }
      m0.raf = requestAnimationFrame(step);
    };
    m0.raf = requestAnimationFrame(step);
    motion.current = m0;
  };

  // 试播、右键菜单用
  useImperativeHandle(ref, () => ({
    play(kind, arg) {
      const br = brainRef.current;
      if (!br) return;
      let p = null;
      if (kind === 'state') p = br.setAgent(arg, null, now(), room()) ?? br.ended(now(), room());
      else if (kind === 'once') p = br.setAgent(br.state.agent, arg, now(), room());
      else if (kind === 'click') p = br.click(now());
      else if (kind === 'sleep') p = br.sleepNow(now());
      else if (kind === 'clip') p = br.force(arg);
      if (p) show(p);
    },
  }));

  const onEnded = (i) => () => {
    if (i === front.current) next.current?.();
  };
  const cx = b[0] + b[2] / 2;
  const headTop = b[1] * H;
  return (
    <div className="pet-layer" ref={layer} data-w={boxSize.w}>
      <div
        className="pet"
        ref={wrap}
        style={{ width: W, height: H, '--bx': `${b[0] * 100}%`, '--by': `${b[1] * 100}%`, '--bw': `${b[2] * 100}%`, '--bh': `${b[3] * 100}%` }}
        data-faded={faded ? '' : undefined}
      >
        {canMove ? (
          isSprite ? (
            <div className="pet-media pet-sprite on" ref={sprite} />
          ) : (
            <>
              <video className="pet-media on" ref={vids[0]} muted playsInline disablePictureInPicture preload="auto" onEnded={onEnded(0)} />
              <video className="pet-media" ref={vids[1]} muted playsInline disablePictureInPicture preload="auto" onEnded={onEnded(1)} />
            </>
          )
        ) : (
          <StillImage m={m} fileUrl={fileUrl} />
        )}
        <button
          type="button"
          className="pet-hit"
          style={{ left: `${b[0] * 100}%`, top: `${b[1] * 100}%`, width: `${b[2] * 100}%`, height: `${b[3] * 100}%` }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onContextMenu={(e) => {
            e.preventDefault();
            const p = brainRef.current?.sleeping ? brainRef.current.click(now()) : brainRef.current?.sleepNow(now());
            if (p) show(p);
          }}
          aria-label={label}
          title={label}
        />
        {bubble && (
          <div key={bubble.n} className="pet-bubble" style={{ left: `${cx * 100}%`, bottom: H - headTop + 6 }}>
            {bubble.text}
          </div>
        )}
        {children}
      </div>
    </div>
  );
});

/** 动不了时（减少动态效果、浏览器不支持透明视频、包里没有这个格式）：显示封面 */
function StillImage({ m, fileUrl }) {
  const [url, setUrl] = useState(null);
  useEffect(() => {
    let dead = false;
    if (m.preview) loadBlob(fileUrl(m.preview)).then((u) => !dead && setUrl(u), () => {});
    return () => {
      dead = true;
    };
  }, [m.preview]);
  return url ? <img className="pet-media on" src={url} alt="" draggable={false} /> : null;
}

export default PetPlayer;
