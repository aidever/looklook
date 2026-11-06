// 宠物播放器（docs/PET_PACK.md §4、§6.4）：对话页、桌面宠物窗口和设置页“试播”共用。
// 视频：两个 <video> 叠着交替，一个在播、一个准备下一段；精灵图：一个 <div> 按帧切背景。
// 位置、走动、拖拽甩抛直接改 style（每帧都动，不走 React 渲染）。
// 拖到哪就停在哪（不往下掉）；甩出去滑一段、碰边反弹再停下。
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { createBrain } from './brain.js';
import { lineFor } from './lines.js';
import { loadBlob, measureBox, videoFormat } from './media.js';

const DEFAULT_BOX = [0.3, 0.15, 0.4, 0.7];
const PAD_DEFAULT = 6;
/** 甩出去后的阻力（每秒速度衰减到 e^-k） */
const DRAG_K = 2.6;
/** 这些台词不管刚说过没，照说 */
const LOUD = new Set(['done', 'error', 'waiting', 'terminal', 'testPass', 'testFail', 'fixed', 'commit', 'push', 'danger', 'levelUp', 'hello', 'morning', 'evening', 'lateNight', 'longSession', 'recapQuick', 'recapLong', 'recapBusy', 'annoyed', 'petted', 'click', 'wake', 'sleepEnd', 'sleepStart']);
/** 特效（头顶飘起来的小符号） */
const FX = { heart: '❤', anger: '💢', sparkle: '✨', note: '♪', zzz: 'z', bang: '!', sweat: '💦', star: '⭐', question: '?', flower: '🌸', music: '♫' };
/** 闲着时自己冒的小表情 */
const IDLE_FX = ['note', 'music', 'sparkle', 'heart', 'question', 'flower', 'star'];
const FX_OF = { notifyDone: 'star', notifyWait: 'bang', notifyError: 'sweat', peerHello: 'heart', peerChat: 'note', petted: 'heart', annoyed: 'anger', testPass: 'sparkle', fixed: 'sparkle', commit: 'star', push: 'star', levelUp: 'heart', waiting: 'bang', danger: 'bang', testFail: 'sweat', errorStreak: 'sweat', error: 'sweat', sleepStart: 'zzz', longThink: 'note', hello: 'heart' };

const reducedMotion = () => {
  try {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  } catch {
    return false;
  }
};

function readPos(key) {
  if (!key) return null;
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return null;
    const v = JSON.parse(raw);
    if (typeof v === 'number') return v > 0 && v < 1 ? { x: v, y: null } : null;
    return v && v.x > 0 && v.x < 1 ? { x: v.x, y: typeof v.y === 'number' && v.y >= 0 && v.y <= 1 ? v.y : null } : null;
  } catch {
    return null;
  }
}

/**
 * manifest：换算后的清单；fileUrl(key)：文件的接口路径；
 * agent：{ state, once, n, events? }（n 变了才算一次新的信号；events 是 events.js 认出来的事）；
 * height：人物多高（像素）；lines：显示气泡；gen：按人设生成的台词；lang：界面语言；label：读屏用的名字；
 * storeKey：记住位置用的 localStorage 键（试播时不传）；page：铺满整个页面（拖到页面任何地方），否则只在父容器里；
 * home(w, h)：没记住位置时放在哪（{ x, y }，y 是离底边多高）；onInteract(kind)：被点、摸、拎、扔（好感度用）；
 * onHit(rect, grab)：人物占的框变了（桌面宠物窗口据此决定哪里能点）。
 */
const PetPlayer = forwardRef(function PetPlayer(
  { manifest: m, fileUrl, agent, height = 130, lines = true, gen, lang = 'zh-CN', label, storeKey, page = false, home, onInteract, onHit, onState, pad: PAD = PAD_DEFAULT, roam = false, idleTalk = true, edges, onExit, children },
  ref,
) {
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
  const talk = useRef({ at: 0, last: null });
  const [box, setBox] = useState(m.stage?.box ?? null);
  const [bubble, setBubble] = useState(null);
  const [fx, setFx] = useState([]);
  const [faded, setFaded] = useState(false);
  const [boxSize, setBoxSize] = useState({ w: 0, h: 0 });
  // 清单内容没变就不重建大脑（外面隔一会儿重新取配置，对象换了但内容一样；重建会把朝向、睡觉都重置）
  const mKey = useMemo(() => JSON.stringify(m), [m]);
  const isSprite = m.renderer === 'sprite';
  const fmt = isSprite ? 'sprite' : videoFormat();
  const still = useMemo(reducedMotion, []);
  // 这个浏览器能不能动：视频要有对应格式的文件
  const canMove = !still && (isSprite || (fmt && Object.values(m.clips).some((c) => c[fmt])));
  const cb = useRef({});
  cb.current = { onInteract, onHit, home, onExit, edges };

  const b = box ?? DEFAULT_BOX;
  const stageW = m.stage?.width || 640;
  const stageH = m.stage?.height || 360;
  // 画面高度：让人物（框的高）正好是 height
  const H = Math.round(Math.min(560, Math.max(60, height / Math.max(0.1, b[3]))));
  const W = Math.round((H * stageW) / stageH);
  const scale = H / stageH;
  // 脚底线：清单写了用它，否则按量出来的人物框底边（默认 0.85 常常比脚低，人就悬在边上）
  const ground = m.stage?.ground ?? (box ? Math.min(1, box[1] + box[3]) : 0.85);
  const charW = b[2] * W;

  // ---------------- 位置 ----------------
  const bounds = useCallback(() => {
    const cw = layer.current?.clientWidth ?? 0;
    const ch = layer.current?.clientHeight ?? 0;
    // y 是脚离底边多高：头顶（脚底往上 (ground - 框顶) × H）不出容器
    // 那边还有屏幕（桌面宠物、多显示器）：可以走出去，走过边就交给那块屏幕的窗口
    const e = cb.current.edges;
    return {
      minX: e?.left ? -charW : charW / 2 + PAD,
      maxX: e?.right ? cw + charW : Math.max(charW / 2 + PAD, cw - charW / 2 - PAD),
      maxY: Math.max(0, ch - PAD * 2 - (ground - b[1]) * H),
      cw,
    };
  }, [charW, H, b, ground]);

  const lastHit = useRef('');
  const exited = useRef(false);
  const tapBubble = useRef(false);
  const reportHit = useCallback(
    (grab = Boolean(motion.current?.drag)) => {
      const f = cb.current.onHit;
      if (!f || pos.current.x == null) return;
      const ch = layer.current?.clientHeight ?? 0;
      const top = ch - PAD - pos.current.y - (ground - b[1]) * H;
      // 头顶留出气泡的地方（能点的气泡更高、更宽）
      const up = tapBubble.current ? 90 : 30;
      const wide = tapBubble.current ? Math.max(charW, 240) : charW;
      const r = [Math.round(pos.current.x - wide / 2), Math.round(top - up), Math.round(wide), Math.round(b[3] * H + up)];
      const k = `${r.join(',')}|${grab}`;
      if (k === lastHit.current) return;
      lastHit.current = k;
      f(r, grab);
    },
    [charW, H, b, ground],
  );

  const place = useCallback(() => {
    const el = wrap.current;
    if (!el || pos.current.x == null) return;
    const { minX, maxX, maxY } = bounds();
    const p = pos.current;
    p.x = Math.min(maxX, Math.max(minX, p.x));
    p.y = Math.min(maxY, Math.max(0, p.y));
    // 人物中心对准 x，脚底线对准 底边往上 PAD + y
    const cx = b[0] + b[2] / 2;
    el.style.transform = `translate(${Math.round(p.x - cx * W)}px, ${Math.round(-(PAD + p.y) + (1 - ground) * H)}px)`;
    reportHit();
    // 人物中心过了屏幕边：交出去（只报一次，由外面换窗口）
    const f = cb.current.onExit;
    const cw = layer.current?.clientWidth ?? 0;
    if (f && cw && !exited.current && !motion.current?.drag && (p.x < 0 || p.x > cw)) {
      exited.current = true;
      f({ x: p.x, y: p.y, side: p.x < 0 ? 'left' : 'right' });
    }
  }, [bounds, b, W, H, ground, reportHit]);

  const room = useCallback(() => {
    const { minX, maxX } = bounds();
    const x = pos.current.x ?? maxX;
    const e = cb.current.edges;
    return { left: e?.left ? 4000 : Math.max(0, (x - minX) / scale), right: e?.right ? 4000 : Math.max(0, (maxX - x) / scale) };
  }, [bounds, scale]);

  const savePos = useCallback(() => {
    if (!storeKey || pos.current.x == null) return;
    const cw = layer.current?.clientWidth;
    const ch = layer.current?.clientHeight;
    if (!cw || !ch) return;
    try {
      localStorage.setItem(storeKey, JSON.stringify({ x: Math.round((pos.current.x / cw) * 1000) / 1000, y: Math.round((pos.current.y / ch) * 1000) / 1000 }));
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
        const saved = readPos(storeKey);
        const h = cb.current.home?.(el.clientWidth, el.clientHeight);
        pos.current.x = saved ? saved.x * el.clientWidth : h?.x ?? el.clientWidth - charW / 2 - 24;
        pos.current.y = saved?.y != null ? saved.y * el.clientHeight : h?.y ?? 0;
      }
      place();
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [place, storeKey, charW]);
  useEffect(place, [place]);

  // ---------------- 说话、特效 ----------------
  const sparkle = useCallback(
    (kind) => {
      if (still || !FX[kind]) return;
      const id = Math.random();
      const n = kind === 'heart' || kind === 'sparkle' ? 3 : 1;
      const items = Array.from({ length: n }, (_, i) => ({ id: id + i, ch: FX[kind], dx: (Math.random() - 0.5) * 40, delay: i * 140 }));
      setFx((l) => [...l.slice(-8), ...items]);
      setTimeout(() => setFx((l) => l.filter((x) => !items.includes(x))), 1600);
    },
    [still],
  );

  /** 说一句：`quiet` 的（改文件、搜索）只在好久没说话时偶尔说；普通的 5 秒内不连着说；LOUD 的照说 */
  const speak = useCallback(
    (key, vars, { quiet = false, force = false, onClick = null } = {}) => {
      if (FX_OF[key]) sparkle(FX_OF[key]);
      if (!lines || !key) return;
      const now = Date.now();
      const t0 = talk.current;
      if (!force && !LOUD.has(key)) {
        if (quiet && (now - t0.at < 25000 || Math.random() > 0.35)) return;
        if (now - t0.at < 5000) return;
      }
      const text = lineFor({ manifest: m, gen, lang, key, vars, last: t0.last });
      if (!text) return;
      talk.current = { at: now, last: text };
      clearTimeout(timers.current.bubble);
      // 能点的气泡（会话通知：点了打开那个会话）留久一点，桌面宠物窗口要把气泡也算进能点的框
      setBubble({ text, n: now, onClick });
      tapBubble.current = Boolean(onClick);
      reportHit();
      timers.current.bubble = setTimeout(
        () => {
          setBubble(null);
          tapBubble.current = false;
          reportHit();
        },
        onClick ? 10000 : Math.min(6000, 2600 + text.length * 70),
      );
    },
    [lines, m, gen, lang, sparkle, reportHit],
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

  const setMirror = (on) => {
    const el = isSprite ? sprite.current : vids[front.current].current;
    el?.style.setProperty('--mirror', on ? '-1' : '1');
  };

  const next = useRef(null);
  const show = useCallback(
    async (p, { line = p?.line } = {}) => {
      if (!p) return;
      if (p.flip) {
        setMirror(p.mirror);
        return;
      }
      if (line) speak(line, null, { force: true });
      if (!canMove) return;
      const my = ++token.current;
      const c = m.clips[p.clip];
      if (!c) return;
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
    [canMove, m, fmt, fileUrl, scale, isSprite, speak, place],
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
    const br = createBrain(m, { now: now(), roam });
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
  }, [mKey, roam]);

  // 闲着时自己冒个小表情、偶尔自言自语（隔 15~45 秒一次；在忙、睡着、被拎着时不冒）
  useEffect(() => {
    if (still) return undefined;
    let wait = 15000 + Math.random() * 30000;
    let since = Date.now();
    const id = setInterval(() => {
      const br = brainRef.current;
      if (!br || document.hidden || drag.current || br.sleeping || br.state.agent !== 'idle') {
        since = Date.now();
        return;
      }
      if (Date.now() - since < wait) return;
      since = Date.now();
      wait = 15000 + Math.random() * 30000;
      sparkle(IDLE_FX[Math.floor(Math.random() * IDLE_FX.length)]);
      if (idleTalk && Math.random() < 0.35) speak(br.state.walked || motion.current ? 'roam' : 'idleChat');
    }, 2000);
    return () => clearInterval(id);
  }, [still, idleTalk, sparkle, speak]);

  // agent 信号：状态变了换动作；同时认出来的事件（测试过了、提交了……）插播动作、说一句
  useEffect(() => {
    const br = brainRef.current;
    if (!br || !agent) return;
    const events = agent.events ?? [];
    const recap = events.find((e) => e.recap);
    const p = br.setAgent(agent.state, agent.once, now(), room());
    setFaded(agent.state === 'offline' && !br.hasOffline());
    // 做完一轮：有总结就说总结，不说干巴巴的“搞定”
    const line = recap && p?.line === 'done' ? null : p?.line;
    if (p) show(p, { line });
    else if (agent.once && !recap) speak(agent.once, null, { force: true });
    let played = Boolean(p);
    for (const e of events) {
      if (e.state && !played) {
        const q = br.event(e.state, now());
        if (q) {
          show(q, { line: null });
          played = true;
        }
      }
      speak(e.key, e.vars, { quiet: e.quiet, force: e.recap });
    }
    onState?.(agent.state);
  }, [agent?.n]);

  // 人物框：清单没写就按封面（或第一段的第一帧）的透明像素算
  useEffect(() => {
    if (m.stage?.box) {
      setBox(m.stage.box);
      return undefined;
    }
    let dead = false;
    // 精灵图量第一格（待机的第一帧）；视频量封面，没有封面就量待机段（用这个浏览器实际播的格式：macOS / iOS 解不出 webm 的透明，
    // 量出来是整幅画面，人物框就比人大一圈，贴不到屏幕边上）
    const sheet = isSprite ? m.sprite?.image : null;
    const idleClip = m.clips[m.states?.idle?.[0]?.clip] ?? m.clips[m.states?.idle?.[0]] ?? null;
    const vf = fmt && fmt !== 'sprite' ? fmt : 'webm';
    const clipSrc = (idleClip && idleClip[vf]) ?? Object.values(m.clips).find((c) => c[vf])?.[vf];
    const src = sheet ?? m.preview ?? clipSrc;
    if (!src) return undefined;
    const cacheKey = `ll-pet-box:${mKey.length}:${src}`;
    try {
      const hit = JSON.parse(localStorage.getItem(cacheKey) ?? 'null');
      if (Array.isArray(hit) && hit.length === 4) {
        setBox(hit);
        return undefined;
      }
    } catch {
      // 读不了：现量
    }
    const idleRow = isSprite ? m.clips[m.states?.idle?.[0]?.clip]?.row ?? 0 : 0;
    const keep = (r) => {
      // 几乎整幅画面：这个浏览器没解出透明，量的不算
      if (dead || !r || (r[2] > 0.95 && r[3] > 0.95)) return;
      setBox(r);
      try {
        localStorage.setItem(cacheKey, JSON.stringify(r.map((v) => Math.round(v * 1000) / 1000)));
      } catch {
        // 存不了：下次再量
      }
    };
    loadBlob(fileUrl(src))
      .then((url) => {
        if (dead) return;
        const isImg = src === m.preview || src === sheet;
        if (isImg) {
          const el = new Image();
          el.onload = () => {
            const [cw, ch] = m.sprite?.cell ?? [192, 208];
            keep(measureBox(el, sheet ? [0, idleRow * ch, cw, ch] : undefined));
          };
          el.src = url;
          return;
        }
        // 视频：量开头和中间两帧取并集（呼吸、摆手时人会大一点）
        const el = document.createElement('video');
        el.muted = true;
        el.playsInline = true;
        el.preload = 'auto';
        let acc = null;
        const marks = [0.04, 0.5];
        let i = 0;
        const union = (a, b) => {
          if (!a) return b;
          if (!b) return a;
          const x0 = Math.min(a[0], b[0]);
          const y0 = Math.min(a[1], b[1]);
          return [x0, y0, Math.max(a[0] + a[2], b[0] + b[2]) - x0, Math.max(a[1] + a[3], b[1] + b[3]) - y0];
        };
        const next = () => {
          if (dead) return;
          if (i >= marks.length) return keep(acc);
          const tt = i === 0 ? marks[0] : (el.duration || 1) * marks[i];
          i += 1;
          el.currentTime = tt;
        };
        el.onseeked = () => {
          acc = union(acc, measureBox(el));
          next();
        };
        el.onloadeddata = next;
        el.src = url;
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [mKey]);

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

  // 卸掉时告诉桌面窗口：没有宠物了，整个窗口都穿透
  useEffect(() => () => cb.current.onHit?.(null, false), []);

  // ---------------- 看向鼠标、看向输入框 ----------------
  const centerX = () => (layer.current?.getBoundingClientRect().left ?? 0) + (pos.current.x ?? 0);
  const look = useRef({ dir: null, since: 0, at: 0 });
  const faceTo = useCallback(
    (clientX, hold) => {
      const br = brainRef.current;
      if (!br || pos.current.x == null || drag.current) return;
      const dx = clientX - centerX();
      const L = look.current;
      const t = Date.now();
      if (Math.abs(dx) < Math.max(60, charW * 0.6)) {
        L.dir = null;
        return;
      }
      const dir = dx < 0 ? 'left' : 'right';
      if (dir !== L.dir) {
        L.dir = dir;
        L.since = t;
      }
        if (t - L.since < hold || t - L.at < 6000 || dir === br.facing) return;
      const p = br.face(dir, now());
      if (p) {
        L.at = t;
        show(p, { line: null });
      }
    },
    [charW, show],
  );
  useEffect(() => {
    if (!canMove) return undefined;
    let last = 0;
    const onMove = (e) => {
      if (e.pointerType !== 'mouse') return;
      const t = Date.now();
      if (t - last < 200) return;
      last = t;
      faceTo(e.clientX, 1500);
    };
    const onKey = (e) => {
      const el = e.target;
      if (!(el instanceof HTMLElement) || !el.matches('textarea, input[type="text"], [contenteditable="true"]')) return;
      const r = el.getBoundingClientRect();
      faceTo(Math.min(r.right - 40, Math.max(r.left + 40, r.left + r.width / 2)), 0);
    };
    window.addEventListener('pointermove', onMove, { passive: true });
    document.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('pointermove', onMove);
      document.removeEventListener('keydown', onKey, true);
    };
  }, [canMove, faceTo]);

  // ---------------- 点、摸、拖、甩 ----------------
  const drag = useRef(null);
  const clicks = useRef([]);
  const rub = useRef({ dir: 0, x: 0, flips: [], at: 0 });

  /** 甩出去：滑一段、碰边反弹，停在哪就是哪 */
  const fling = (vx0, vy0, onStop) => {
    let vx = Math.max(-3200, Math.min(3200, vx0));
    let vy = Math.max(-3200, Math.min(3200, vy0));
    let last = now();
    const m0 = { raf: 0, drag: true };
    const step = (t) => {
      const s = Math.min(0.05, (t - last) / 1000);
      last = t;
      const { minX, maxX, maxY } = bounds();
      const p = pos.current;
      p.x += vx * s;
      p.y += vy * s;
      if (p.x < minX || p.x > maxX) {
        p.x = p.x < minX ? minX : maxX;
        vx = -vx * 0.6;
      }
      if (p.y < 0 || p.y > maxY) {
        p.y = p.y < 0 ? 0 : maxY;
        vy = -vy * 0.6;
      }
      const k = Math.exp(-DRAG_K * s);
      vx *= k;
      vy *= k;
      place();
      if (Math.hypot(vx, vy) < 30) {
        motion.current = null;
        savePos();
        reportHit(false);
        onStop();
        return;
      }
      m0.raf = requestAnimationFrame(step);
    };
    m0.raf = requestAnimationFrame(step);
    motion.current = m0;
  };

  const onPointerDown = (e) => {
    if (e.button !== 0 && e.pointerType === 'mouse') return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    drag.current = { id: e.pointerId, sx: e.clientX, sy: e.clientY, x0: pos.current.x, y0: pos.current.y, moved: false, samples: [{ x: e.clientX, y: e.clientY, t: now() }] };
    reportHit(true);
  };
  const onPointerMove = (e) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) {
      // 没按着、鼠标在身上来回蹭：摸头
      if (!d && e.pointerType === 'mouse' && e.buttons === 0) rubbed(e.clientX);
      return;
    }
    const dx = e.clientX - d.sx;
    const dy = e.clientY - d.sy;
    if (!d.moved && Math.hypot(dx, dy) < 6) return;
    if (!d.moved) {
      d.moved = true;
      stopMotion();
      motion.current = { raf: 0, drag: true };
      const p = brainRef.current?.dragStart(now());
      if (p) show(p, { line: null });
      speak('dragged');
      cb.current.onInteract?.('drag');
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
      reportHit(false);
      clicked(e.clientX);
      return;
    }
    // 拖到窗口外面松手（桌面宠物拖到另一块屏幕上）：交给那块屏幕的窗口
    const cw0 = layer.current?.clientWidth ?? 0;
    const ch0 = layer.current?.clientHeight ?? 0;
    if (cb.current.onExit && cw0 && (e.clientX < 0 || e.clientX > cw0 || e.clientY < 0 || e.clientY > ch0)) {
      motion.current = null;
      exited.current = true;
      reportHit(false);
      cb.current.onExit({ x: e.clientX, y: pos.current.y, drop: true, clientY: e.clientY });
      return;
    }
    // 松手：按松手前 100 ms 里的速度甩出去；停住了再松手就放在这儿
    const tUp = now();
    d.samples.push({ x: e.clientX, y: e.clientY, t: tUp });
    const recent = d.samples.filter((x) => tUp - x.t <= 100);
    const a = recent[0];
    const z = recent[recent.length - 1];
    const dt = Math.max(0.016, (z.t - a.t) / 1000);
    let vx = (z.x - a.x) / dt;
    let vy = -(z.y - a.y) / dt;
    if (recent.length < 2 || Math.hypot(vx, vy) < 300) vx = vy = 0;
    if (Math.hypot(vx, vy) > 1200) {
      speak('thrown');
      cb.current.onInteract?.('throw');
    }
    fling(vx, vy, () => {
      const q = brainRef.current?.dragEnd(now(), room());
      if (q) show(q, { line: null });
    });
  };

  /** 点了一下：2.5 秒里连点 5 下就烦了，8 下以上躲开 */
  const clicked = (clientX) => {
    const t = Date.now();
    clicks.current = [...clicks.current.filter((x) => t - x < 2500), t];
    const n = clicks.current.length;
    const br = brainRef.current;
    if (!br) return;
    cb.current.onInteract?.(n >= 5 ? 'annoyed' : 'click');
    if (n >= 5) {
      const p = br.annoyed(now());
      if (p) show(p, { line: null });
      speak('annoyed');
      if (n >= 8) {
        clicks.current = [];
        // 跳开：往离鼠标远的方向甩一下
        const away = clientX < centerX() ? 1 : -1;
        stopMotion();
        fling(away * 1400, 700, () => {
          const q = brainRef.current?.dragEnd(now(), room());
          if (q) show(q, { line: null });
        });
      }
      return;
    }
    const p = br.click(now());
    if (p) show(p);
    else speak('click');
  };

  /** 鼠标在身上左右来回蹭：1.2 秒里换向 4 次算摸头（1.5 秒最多一次） */
  const rubbed = (x) => {
    const r = rub.current;
    const t = Date.now();
    const dir = Math.sign(x - r.x);
    if (Math.abs(x - r.x) < 4) return;
    if (dir && r.dir && dir !== r.dir) r.flips = [...r.flips.filter((f) => t - f < 1200), t];
    r.dir = dir;
    r.x = x;
    if (r.flips.length >= 4 && t - r.at > 1500) {
      r.flips = [];
      r.at = t;
      const p = brainRef.current?.petted(now());
      if (p) show(p, { line: null });
      speak('petted');
      cb.current.onInteract?.('pet');
    }
  };

  // 试播、右键菜单、外面插话用
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
      else if (kind === 'event') p = br.event(arg, now());
      else if (kind === 'petted') p = br.petted(now());
      else if (kind === 'annoyed') p = br.annoyed(now());
      if (p) show(p, { line: kind === 'event' ? null : p.line });
    },
    /** 放到 (x, 离底边 y)；`walkIn`：从边上进来时往里走一段 */
    moveTo(x, y, { walkIn } = {}) {
      stopMotion();
      exited.current = false;
      pos.current.x = x;
      pos.current.y = Math.max(0, y ?? 0);
      place();
      savePos();
      const br = brainRef.current;
      if (walkIn && br) {
        const p = br.walkTo?.(walkIn, now(), room());
        if (p) show(p, { line: null });
      }
    },
    /** 头顶冒个小表情 */
    fx(kind) {
      sparkle(kind);
    },
    /** 转过去看一边（`left` / `right`） */
    look(dir) {
      const br = brainRef.current;
      const p = br && dir !== br.facing ? br.face(dir, now()) : null;
      if (p) show(p, { line: null });
    },
    /** 说一句（带数据）：`opts.state` 有的话顺便插播那个动作 */
    say(key, vars, opts = {}) {
      const br = brainRef.current;
      if (opts.state && br) {
        const p = br.event(opts.state, now());
        if (p) show(p, { line: null });
      }
      speak(key, vars, opts);
    },
  }));

  const onEnded = (i) => () => {
    if (i === front.current) next.current?.();
  };
  const cx = b[0] + b[2] / 2;
  const headTop = b[1] * H;
  const body = (
    <div className={`pet-layer${page ? ' pet-page' : ''}`} ref={layer} data-w={boxSize.w}>
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
        {fx.map((f) => (
          <span key={f.id} className="pet-fx" style={{ left: `calc(${cx * 100}% + ${f.dx}px)`, bottom: H - headTop - 4, animationDelay: `${f.delay}ms` }} aria-hidden="true">
            {f.ch}
          </span>
        ))}
        {bubble && (
          <div
            key={bubble.n}
            className={`pet-bubble${bubble.onClick ? ' pet-bubble-link' : ''}`}
            style={{ left: `${cx * 100}%`, bottom: H - headTop + 6 }}
            role={bubble.onClick ? 'button' : undefined}
            tabIndex={bubble.onClick ? 0 : undefined}
            onClick={
              bubble.onClick
                ? () => {
                    const f = bubble.onClick;
                    setBubble(null);
                    tapBubble.current = false;
                    reportHit();
                    f();
                  }
                : undefined
            }
          >
            {bubble.text}
          </div>
        )}
        {children}
      </div>
    </div>
  );
  return page ? createPortal(body, document.body) : body;
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
