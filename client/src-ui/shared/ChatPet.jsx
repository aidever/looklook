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
    <LivePet key={pet.key} pet={pet} cfg={cfg} status={status} items={items} fileUrl={fileUrl} height={(touch || isTouch() ? 96 : 130) * (cfg.size || 1)} page storeKey="ll-chat-pet-pos" home={chatHome}>
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
export function LivePet({ pet, cfg, status, items, fileUrl, height, page, storeKey, home, onHit, children }) {
  const { t, i18n } = useTranslation();
  const player = useRef(null);
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
    >
      {children}
    </PetPlayer>
  );
}
