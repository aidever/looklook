// 对话界面的宠物（docs/PET_PACK.md）：用哪只按“设置 → 宠物”里给这种对话配的，跟着对话状态换动作，
// 看得懂对话里在干什么（events.js），按运行数据碎碎念（lines.js），记好感度（越处越熟，升级有庆祝）。
// 网页里宠物在整个页面上，拖到哪停在哪；桌面应用开了桌面宠物时，宠物住在桌面窗口里（DeskPet.jsx），这里不再放。
// 鼠标移上去有个小叉可以藏起来，藏起来后留一个很淡的小点，点了叫回来（记在这个浏览器里）。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { api } from './api.js';
import { isTouch } from './KeyBar.jsx';
import PetPlayer from './pet/PetPlayer.jsx';
import { agentState, textOf } from './pet/brain.js';
import { createWatcher } from './pet/events.js';
import { greetKey } from './pet/lines.js';
import { levelOf } from './pet/bond.js';

/** 设置页改了宠物（换了、导入、删除、生成了台词）：对话页重新取 */
export const PETS_CHANGED = 'll-pets-changed';
const HIDE_KEY = 'll-chat-pet';
const enc = encodeURIComponent;

/** 在桌面应用的主窗口里（不是宠物窗口） */
export const inDesktopMain = () => typeof window !== 'undefined' && Boolean(window.__LOOKLOOK_DESKTOP__) && !window.__LOOKLOOK_PET_DESK__;

function readHidden() {
  try {
    return localStorage.getItem(HIDE_KEY) === 'off';
  } catch {
    return false;
  }
}

// 宠物相关的东西变了（设置、装 / 删、生成了台词，可能是在别的标签页、桌面宠物窗口、远程打开的页面里改的）：
// 客户端有个版本号，长轮询它，变了就发 PETS_CHANGED，各处重新取。整页共用一个轮询。
const live = { n: 0, ver: null, run: false };
function liveLoop() {
  if (!live.n) {
    live.run = false;
    return;
  }
  api.get(`/pets/version${live.ver == null ? '' : `?since=${live.ver}`}`).then(
    (r) => {
      if (live.ver != null && r?.ver !== live.ver) window.dispatchEvent(new Event(PETS_CHANGED));
      live.ver = r?.ver ?? live.ver;
      setTimeout(liveLoop, 200);
    },
    // 连不上（客户端旧、断网）：过一会儿再试
    () => setTimeout(liveLoop, 10000),
  );
}
export function usePetsLive() {
  useEffect(() => {
    live.n += 1;
    if (!live.run) {
      live.run = true;
      liveLoop();
    }
    return () => {
      live.n -= 1;
    };
  }, []);
}

// ---------------- 宠物来报信 ----------------
// 开着宠物、设置里“会话提醒由宠物来说”开着时，别的会话做完 / 出错 / 等你处理，不弹右上角的卡片，
// 改由宠物说一句（点气泡打开那个会话）。页面里有宠物的话页面里的说；桌面应用开着桌面宠物时桌面宠物说。
const NOTE_KEY = { done: 'notifyDone', failed: 'notifyError', attn: 'notifyWait' };
const sinks = new Set();
const desk = { on: false, open: null };

/** 交给宠物说：`x` = { id, kind, title }；说了返回 true（调用方就不再弹卡片） */
export function petNotify(x, open) {
  const key = NOTE_KEY[x.kind];
  if (!key) return false;
  for (const f of sinks) if (f(key, x, open)) return true;
  if (desk.on) {
    api.put('/desktop/pet', { note: { kind: x.kind, title: x.title ?? '', id: x.id } }).catch(() => {});
    return true;
  }
  return false;
}

/**
 * 桌面应用的主窗口：桌面宠物开着、设置里开了“宠物来报信”时，提醒转给它（petNotify）；
 * 点了桌面宠物的提醒气泡，主窗口打开那个会话（`open(id)`）。
 */
export function useDeskNotify(open) {
  const on = inDesktopMain();
  const openRef = useRef(open);
  openRef.current = open;
  usePetsLive();
  useEffect(() => {
    if (!on) return undefined;
    let dead = false;
    let cfg = null;
    let st = null;
    let seen = null;
    const apply = () => {
      desk.on = Boolean(st?.running && cfg?.pet && cfg.desktop !== false && cfg.notify !== false && cfg.lines !== false);
    };
    const loadCfg = () =>
      api.get('/pets/current').then(
        (r) => {
          cfg = r;
          apply();
        },
        () => {},
      );
    const loop = async () => {
      while (!dead) {
        try {
          const r = await api.get(`/desktop/pet${st?.seq != null ? `?since=${st.seq}` : ''}`);
          if (dead) return;
          st = r;
          apply();
          const o = r?.open;
          if (o?.seq != null && o.seq !== seen) {
            // 刚开起来时只认最近几秒的（点了气泡、主窗口是那时才开的）
            if (seen != null || Date.now() - (o.at ?? 0) < 10000) {
              // 桌面宠物右键菜单里点了“宠物设置”
              if (o.kind === 'settings') window.dispatchEvent(new CustomEvent('ll-menu', { detail: 'pet_settings' }));
              else openRef.current?.(o.id);
            }
            seen = o.seq;
          } else if (seen == null) seen = o?.seq ?? 0;
        } catch {
          await new Promise((ok) => setTimeout(ok, 10000));
        }
      }
    };
    loadCfg();
    loop();
    window.addEventListener(PETS_CHANGED, loadCfg);
    return () => {
      dead = true;
      desk.on = false;
      window.removeEventListener(PETS_CHANGED, loadCfg);
    };
  }, [on]);
}

/** 页面里的宠物报提醒：活着、开着台词和“宠物来报信”时登记 */
function usePetSink(player, on) {
  useEffect(() => {
    if (!on) return undefined;
    const f = (key, x, open) => {
      if (!player.current) return false;
      player.current.say(key, { name: x.title || '' }, { force: true, state: x.kind === 'failed' ? 'error' : x.kind === 'attn' ? 'waiting' : 'done', onClick: () => open?.(x.id) });
      return true;
    };
    sinks.add(f);
    return () => sinks.delete(f);
  }, [on]);
}

/** 一个对话用的宠物：`{ pet: { key, source, id, manifest } | null, size, lines, chatter, desktop, gen }` */
export function useInstancePet(id, active) {
  const [cfg, setCfg] = useState(null);
  useEffect(() => {
    if (!active || !id) return undefined;
    let dead = false;
    const load = () =>
      api.get(`/instances/${id}/pet`).then(
        (r) => !dead && setCfg(r),
        () => {},
      );
    load();
    window.addEventListener(PETS_CHANGED, load);
    return () => {
      dead = true;
      window.removeEventListener(PETS_CHANGED, load);
    };
  }, [id, active]);
  return cfg;
}

/** 桌面应用：宠物窗口开着没有（开着的话主窗口不放宠物），并把正在看的对话报给它 */
function useDeskPet(id, active) {
  const [running, setRunning] = useState(false);
  const on = inDesktopMain();
  useEffect(() => {
    if (!on) return undefined;
    let dead = false;
    const check = () => api.get('/desktop/pet').then((r) => !dead && setRunning(Boolean(r.running)), () => {});
    check();
    const t = setInterval(check, 15000);
    window.addEventListener(PETS_CHANGED, check);
    return () => {
      dead = true;
      clearInterval(t);
      window.removeEventListener(PETS_CHANGED, check);
    };
  }, [on]);
  useEffect(() => {
    if (on && active && id && !document.hidden) api.put('/desktop/pet', { focus: id }).catch(() => {});
  }, [on, active, id]);
  return on && running;
}

export default function ChatPet({ id, active, status, items, touch }) {
  const { t } = useTranslation();
  usePetsLive();
  const cfg = useInstancePet(id, active);
  const desk = useDeskPet(id, active);
  const [hidden, setHidden] = useState(readHidden);
  const toggle = (v) => {
    setHidden(v);
    try {
      if (v) localStorage.setItem(HIDE_KEY, 'off');
      else localStorage.removeItem(HIDE_KEY);
    } catch {
      // 存不了（隐私模式）：这次照样生效
    }
  };
  const pet = cfg?.pet;
  const fileUrl = useMemo(() => (pet ? (key) => `/instances/${id}/pet/file/${enc(pet.source)}/${enc(pet.id)}/${key}` : null), [id, pet?.key]);
  const player = useRef(null);
  const shown = Boolean(active && pet && !(desk && cfg.desktop !== false) && !hidden);
  // 网页（不是桌面应用的主窗口）：这台电脑上的桌面宠物也开着时，两只互动
  usePeerWeb(player, shown && !inDesktopMain() && cfg?.lines !== false, pet ? textOf(pet.manifest?.name, 'zh-CN') : '');
  usePetSink(player, shown && cfg?.lines !== false && cfg?.notify !== false);
  if (!active || !pet) return null;
  if (desk && cfg.desktop !== false) return null;
  if (hidden) {
    return (
      <div className="pet-layer">
        <button type="button" className="pet-peek" onClick={() => toggle(false)} title={t('pet.show')} aria-label={t('pet.show')} />
      </div>
    );
  }
  // 嵌在终端页里的对话标签总是传 touch=false，按设备再认一次（手机上人物小一点）
  return (
    <LivePet key={pet.key} pet={pet} cfg={cfg} status={status} items={items} fileUrl={fileUrl} height={(touch || isTouch() ? 96 : 130) * (cfg.size || 1)} page storeKey="ll-chat-pet-pos" home={chatHome} playerRef={player}>
      <button type="button" className="pet-hide" onClick={() => toggle(true)} title={t('pet.hide')} aria-label={t('pet.hide')}>
        <X size={11} />
      </button>
    </LivePet>
  );
}

/** 没拖过时站在对话区右下角、输入框上面 */
function chatHome(w, h) {
  const main = document.querySelector('.chat-main')?.getBoundingClientRect();
  const dock = document.querySelector('.chat-dock')?.getBoundingClientRect();
  const right = main?.right ?? w;
  const top = dock?.top ?? main?.bottom ?? h;
  return { x: right - 110, y: Math.max(0, h - top + 4) };
}

const today = () => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
};
/** 好感度：每种事加多少 */
const XP = { clicks: 1, pets: 3, turns: 2, tests: 1, fixes: 3, commits: 2, hello: 5 };

/**
 * 一只活的宠物：跟着对话状态和条目走、记好感度、碎碎念。对话页和桌面宠物窗口共用。
 * `home`：没记住位置时放在哪；`onHit`：桌面窗口用。
 */
export function LivePet({ pet, cfg, status, items, fileUrl, height, page, storeKey, home, onHit, playerRef, roam = true, pad, edges, onExit, onMenu, alpha, children }) {
  const { t, i18n } = useTranslation();
  const own = useRef(null);
  const player = playerRef ?? own;
  const chatter = cfg.chatter !== false;
  const gen = cfg.gen && typeof cfg.gen === 'object' ? cfg.gen : null;
  const name = textOf(pet.manifest.name, i18n.language);

  // ---------------- 好感度 ----------------
  const [bond, setBond] = useState(null);
  const pending = useRef({});
  const flushT = useRef(0);
  const bondUrl = `/pets/${enc(pet.source)}/${enc(pet.id)}/bond`;
  const bondRef = useRef(null);
  bondRef.current = bond;
  const flush = useCallback(() => {
    clearTimeout(flushT.current);
    const add = pending.current;
    pending.current = {};
    if (!Object.keys(add).length) return;
    const before = levelOf(bondRef.current?.xp ?? 0).level;
    api.post(bondUrl, { add, day: today() }).then(
      (b) => {
        setBond(b);
        const after = levelOf(b.xp).level;
        if (bondRef.current && after > before) player.current?.say('levelUp', { level: after }, { state: 'levelUp', force: true });
      },
      () => {},
    );
  }, [bondUrl]);
  const addBond = useCallback(
    (kind, n = 1) => {
      pending.current[kind] = (pending.current[kind] ?? 0) + n;
      if (XP[kind]) pending.current.xp = (pending.current.xp ?? 0) + XP[kind] * n;
      clearTimeout(flushT.current);
      flushT.current = setTimeout(flush, 1500);
    },
    [flush],
  );
  useEffect(() => () => flush(), [flush]);
  // 打开时取一次；今天第一次见：打招呼
  useEffect(() => {
    let dead = false;
    api.post(bondUrl, { add: {}, day: today() }).then(
      (b) => {
        if (dead) return;
        setBond(b);
        if (b.newDay) {
          setTimeout(() => {
            if (dead) return;
            player.current?.say(b.days <= 1 ? 'hello' : greetKey(), null, { state: 'hello', force: true });
            addBond('hello');
          }, 1500);
        }
      },
      () => {},
    );
    return () => {
      dead = true;
    };
  }, [bondUrl]);

  const lastClick = useRef(0);
  const onInteract = useCallback(
    (kind) => {
      if (kind === 'pet') addBond('pets');
      // 连着点只算第一下，免得刷好感
      if (kind === 'click' && Date.now() - lastClick.current > 3000) {
        lastClick.current = Date.now();
        addBond('clicks');
      }
    },
    [addBond],
  );

  // ---------------- 对话 → 宠物 ----------------
  const prev = useRef(null);
  const seq = useRef(0);
  const watcher = useMemo(createWatcher, [pet.key]);
  const reminded = useRef({ late: 0, rest: 0 });
  // 一直在想、没有新条目时也要隔一会儿看看（“想了好久”）
  const [tick, setTick] = useState(0);
  const running = status?.phase === 'running';
  useEffect(() => {
    if (!running) return undefined;
    const iv = setInterval(() => setTick((n) => n + 1), 30000);
    return () => clearInterval(iv);
  }, [running]);
  const sig = `${tick}|${status?.phase}|${status?.tool ?? ''}|${status?.error ?? ''}|${items?.length ?? 0}|${items?.[items.length - 1]?.key ?? ''}|${items?.[items.length - 1]?.blocks?.length ?? 0}`;
  const [agent, setAgent] = useState({ state: 'idle', once: null, n: 0, events: [] });
  useEffect(() => {
    const r = agentState(status, prev.current);
    const changed = !prev.current || prev.current.phase !== status?.phase || prev.current.tool !== (status?.tool ?? null) || prev.current.error !== (status?.error ?? null);
    prev.current = status ? { phase: status.phase, tool: status.tool ?? null, error: status.error ?? null } : null;
    const now = Date.now();
    let events = watcher.step(items ?? [], status, now);
    for (const e of events) if (e.count) addBond(e.count);
    if (!chatter) events = [];
    // 深夜还在发消息：心疼一下（一小时最多一次）
    const h = new Date().getHours();
    events = events.map((e) => {
      if (e.key === 'sent' && h < 5 && now - reminded.current.late > 3600e3) {
        reminded.current.late = now;
        return { key: 'lateNight' };
      }
      return e;
    });
    // 连着干太久：做完一轮时劝歇会儿（90 分钟一次）
    const min = watcher.sessionMinutes(now);
    if (events.some((e) => e.recap) && min >= 90 && now - reminded.current.rest > 90 * 60e3) {
      reminded.current.rest = now;
      events.push({ key: 'longSession', vars: { min } });
    }
    if (!changed && !events.length) return;
    seq.current += 1;
    setAgent({ ...r, n: seq.current, events });
  }, [sig, chatter, watcher]);

  const lv = bond ? levelOf(bond.xp) : null;
  const label = lv ? `${t('pet.label', { name })} · Lv.${lv.level}` : t('pet.label', { name });
  return (
    <PetPlayer
      ref={player}
      manifest={pet.manifest}
      fileUrl={fileUrl}
      agent={agent}
      height={height}
      lines={cfg.lines !== false}
      gen={gen}
      lang={i18n.language}
      label={label}
      storeKey={storeKey}
      page={page}
      home={home}
      onInteract={onInteract}
      onHit={onHit}
      roam={roam}
      idleTalk={chatter && cfg.lines !== false}
      pad={pad}
      edges={edges}
      onExit={onExit}
      onMenu={onMenu}
      alpha={alpha}
    >
      {children}
    </PetPlayer>
  );
}

/**
 * 两只宠物说话（桌面的和网页里的，同一台电脑上）：对方刚说了一句、还不是在接话，就过一会儿接一句。
 * `me` = 'desk' / 'web'。返回 `say(key)`：自己先开口（对方会接）。
 */
export function usePeerChat(player, st, me) {
  const seen = useRef(null);
  useEffect(() => {
    const c = st?.chat;
    if (!c || c.seq === seen.current) return undefined;
    const first = seen.current == null;
    seen.current = c.seq;
    // 刚打开时看到的是以前的
    if (first || c.from === me) return undefined;
    // 对方先开口：转过去看它、接一句；对方是在接话就只做个表情
    const id = setTimeout(
      () => {
        const dir = peerDir(st, me);
        if (dir) player.current?.look(dir);
        if (c.reply) return player.current?.fx('heart');
        player.current?.say(c.key === 'peerHello' ? 'peerHello' : 'peerChat', null, { force: true });
        api.put('/desktop/pet', { chat: { from: me, key: 'peerChat', reply: true } }).catch(() => {});
      },
      1200 + Math.random() * 800,
    );
    return () => clearTimeout(id);
  }, [st?.chat?.seq]);
  // 对方在的时候，隔一两分钟自己找它聊一句
  const present = Boolean(st?.peer && st?.pos && st?.running);
  useEffect(() => {
    if (!present) return undefined;
    const id = setInterval(() => {
      if (document.hidden || Math.random() > 0.35) return;
      const dir = peerDir(stRef.current, me);
      if (dir) player.current?.look(dir);
      player.current?.say('peerChat', null, { force: true });
      api.put('/desktop/pet', { chat: { from: me, key: 'peerChat' } }).catch(() => {});
    }, 60000);
    return () => clearInterval(id);
  }, [present]);
  const stRef = useRef(st);
  stRef.current = st;
}

/** 对方在自己的哪一边（屏幕上的物理像素比较） */
export function peerDir(st, me) {
  if (!st?.peer || !st?.pos) return null;
  const a = me === 'desk' ? st.pos.x : st.peer.x;
  const b = me === 'desk' ? st.peer.x : st.pos.x;
  if (Math.abs(a - b) < 40) return null;
  return b < a ? 'left' : 'right';
}

/**
 * 网页里的宠物（同一台电脑上开着桌面宠物时）：页面在前台就隔几秒报一下自己在屏幕上的位置，
 * 刚切到前台时转向桌面宠物打个招呼，对方说话时接一句（usePeerChat）。
 */
function usePeerWeb(player, on, name) {
  const [st, setSt] = useState(null);
  const was = useRef({ active: false, hello: 0 });
  useEffect(() => {
    if (!on) return undefined;
    let dead = false;
    const tick = async () => {
      const active = document.visibilityState === 'visible' && document.hasFocus();
      if (!active) {
        was.current.active = false;
        return;
      }
      let r;
      try {
        r = await api.get('/desktop/pet');
      } catch {
        return;
      }
      if (dead) return;
      // 桌面宠物没开、不是这台电脑上打开的、桌面宠物没显示
      if (!r?.running || !r?.here || !r?.pos) {
        setSt(null);
        was.current.active = false;
        return;
      }
      const rect = document.querySelector('.pet-page .pet-hit')?.getBoundingClientRect();
      if (!rect?.width) return;
      // 估一下自己在屏幕上的位置（物理像素）：浏览器窗口位置 + 标题栏 + 页面里的位置
      const k = window.devicePixelRatio || 1;
      const x = (window.screenX + rect.left + rect.width / 2) * k;
      const y = (window.screenY + Math.max(0, window.outerHeight - window.innerHeight) + rect.top + rect.height / 2) * k;
      const r2 = await api.put('/desktop/pet', { peer: { x, y, name } }).catch(() => null);
      if (dead) return;
      setSt(r2 ?? r);
      if (!was.current.active && Date.now() - was.current.hello > 120000) {
        was.current.hello = Date.now();
        player.current?.look(r.pos.x < x ? 'left' : 'right');
        player.current?.say('peerHello', null, { force: true });
        api.put('/desktop/pet', { chat: { from: 'web', key: 'peerHello' } }).catch(() => {});
      }
      was.current.active = true;
    };
    const first = setTimeout(tick, 1500);
    const id = setInterval(tick, 3000);
    window.addEventListener('focus', tick);
    return () => {
      dead = true;
      clearTimeout(first);
      clearInterval(id);
      window.removeEventListener('focus', tick);
    };
  }, [on]);
  usePeerChat(player, st, 'web');
}
