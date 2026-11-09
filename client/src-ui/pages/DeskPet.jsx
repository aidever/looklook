// 桌面宠物窗口的页面（`/pet-desk?screen=i`，桌面应用的 src/desktop.rs 给每块屏幕开一个透明置顶窗口，铺满那块屏幕的工作区）：
// 宠物只在一个窗口里（`owner`），可以拖到屏幕任何地方；走过屏幕边、拖到别的屏幕上松手就换到那块屏幕的窗口。
// 跟着主窗口正在看的对话（`/api/desktop/pet` 的 focus）。宠物占的框报给桌面那边，框外的鼠标穿透到下面的窗口。
// 同一台电脑上开着网页、网页里也有宠物（在前台）时，两只会互相打招呼、聊两句（见 ChatPet.jsx 的 usePeer）。
// 不影响干活：点、拖宠物不抢焦点（桌面那边把窗口设成不能获得焦点）；可以半透明（鼠标放上去变实，桌面那边报 `hot`）、
// 可以“不挡操作”（鼠标直接穿过，按住 Ctrl / ⌥ 才能抓）。右键宠物就地调这些。
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '../shared/api.js';
import { LivePet, PETS_CHANGED, useInstancePet, usePeerChat, usePetsLive } from '../shared/ChatPet.jsx';
import { Seg } from '../shared/ui.jsx';
import { useChat } from './ChatView.jsx';

const enc = encodeURIComponent;
const IDLE = { phase: 'idle' };
const SCREEN = (() => {
  const n = Number(new URLSearchParams(window.location.search).get('screen'));
  return Number.isInteger(n) && n >= 0 ? n : 0;
})();

/** 桌面宠物的共享状态（长轮询，变了马上拿到）：宠物在哪块屏幕、刚换过来时放哪、网页宠物在不在、说了什么 */
function useDeskState() {
  const [st, setSt] = useState(null);
  useEffect(() => {
    let dead = false;
    let seq = null;
    const loop = () => {
      if (dead) return;
      api.get(`/desktop/pet${seq == null ? '' : `?since=${seq}`}`).then(
        (r) => {
          if (dead) return;
          seq = r?.seq ?? seq;
          setSt(r);
          setTimeout(loop, 50);
        },
        () => setTimeout(loop, 3000),
      );
    };
    loop();
    return () => {
      dead = true;
    };
  }, []);
  return st;
}

export default function DeskPet() {
  const st = useDeskState();
  // 设置里改了：马上跟着换
  usePetsLive();
  // 背景透明（窗口本身是透明的）
  useEffect(() => {
    document.documentElement.classList.add('pet-desk-root');
    return () => document.documentElement.classList.remove('pet-desk-root');
  }, []);
  if (!st) return null;
  // 宠物在别的屏幕上：这个窗口空着（一直鼠标穿透）
  if ((st.owner ?? 0) !== SCREEN) return null;
  return st.focus ? <FocusPet id={st.focus} st={st} /> : <DefaultPet st={st} />;
}

function FocusPet({ id, st }) {
  const cfg = useInstancePet(id, true);
  const chat = useChat(id, true);
  const pet = cfg?.pet;
  const fileUrl = useMemo(() => (pet ? (key) => `/instances/${id}/pet/file/${enc(pet.source)}/${enc(pet.id)}/${key}` : null), [id, pet?.key]);
  if (!pet) return <DefaultPet st={st} />;
  return <Desk pet={pet} cfg={cfg} status={chat.status ?? IDLE} items={chat.items} fileUrl={fileUrl} st={st} />;
}

/** 还没打开过对话：用设置里的宠物，只是待着 */
function DefaultPet({ st }) {
  const [cfg, setCfg] = useState(null);
  useEffect(() => {
    let dead = false;
    const load = () =>
      api.get('/pets/current').then(
        (r) => !dead && setCfg(r?.pet ? r : null),
        () => !dead && setCfg(null),
      );
    load();
    window.addEventListener(PETS_CHANGED, load);
    return () => {
      dead = true;
      window.removeEventListener(PETS_CHANGED, load);
    };
  }, []);
  const pet = cfg?.pet;
  const fileUrl = useMemo(() => (pet ? (key) => `/pets/${enc(pet.source)}/${enc(pet.id)}/file/${key}` : null), [pet?.key]);
  if (!pet) return null;
  return <Desk pet={pet} cfg={cfg} status={IDLE} items={[]} fileUrl={fileUrl} st={st} />;
}

function Desk({ pet, cfg, status, items, fileUrl, st }) {
  const player = useRef(null);
  // 右键菜单开着：整个窗口接鼠标（点菜单外面关掉它）
  const [menu, setMenu] = useState(null);
  const menuOpen = useRef(false);
  menuOpen.current = Boolean(menu);
  // 框变了报给桌面那边；拖着的时候每帧都变，最多 80 ms 报一次
  const q = useRef({ t: 0, timer: 0, last: null, raw: null });
  const onHit = (hit, grab, tap = false) => {
    const s = q.current;
    s.raw = { hit, grab, tap };
    s.last = { hit, grab: grab || menuOpen.current, tap };
    if (s.timer) return;
    const send = () => {
      s.timer = 0;
      s.t = Date.now();
      api.put('/desktop/pet', s.last).catch(() => {});
    };
    const wait = Math.max(0, 80 - (Date.now() - s.t));
    if (!wait || grab === false) send();
    else s.timer = setTimeout(send, wait);
  };
  const home = (w) => ({ x: w - 140, y: 0 });
  // 走出 / 拖出了这块屏幕：报给桌面那边，它算出落在哪块屏幕上
  const onExit = ({ x, y, side, drop, clientY }) => {
    const top = drop ? clientY : window.innerHeight - y - 2;
    api.put('/desktop/pet', { exit: { screen: SCREEN, x, y: top, side: drop ? null : side, bottom: y }, hit: null }).catch(() => {});
  };
  // 刚换到这块屏幕（或那边没屏幕、退回来）：放到给的位置，从边上往里走
  const entrySeq = useRef(null);
  useEffect(() => {
    const e = st.entry;
    if (!e || e.seq === entrySeq.current) return undefined;
    entrySeq.current = e.seq;
    const id = setTimeout(() => player.current?.moveTo(e.x, e.y, { walkIn: e.walk ?? undefined }), 80);
    return () => clearTimeout(id);
  }, [st.entry?.seq]);
  usePeerChat(player, st, 'desk');
  // 主窗口转过来的会话提醒：说一句，点气泡打开那个会话（主窗口会被叫出来）
  const noteSeq = useRef(null);
  useEffect(() => {
    const n = st.note;
    if (!n || n.seq === noteSeq.current) return undefined;
    const first = noteSeq.current == null;
    noteSeq.current = n.seq;
    if (first && Date.now() - (n.at ?? 0) > 5000) return undefined;
    const key = { done: 'notifyDone', failed: 'notifyError', attn: 'notifyWait' }[n.kind];
    const state = { done: 'done', failed: 'error', attn: 'waiting' }[n.kind];
    const id = setTimeout(() => player.current?.say(key, { name: n.title || '' }, { force: true, state, onClick: () => api.put('/desktop/pet', { open: n.id }).catch(() => {}) }), 300);
    return () => clearTimeout(id);
  }, [st.note?.seq]);
  // 菜单开、关：重新报一次（grab 跟着变）
  useEffect(() => {
    const r = q.current.raw;
    if (r) onHit(r.hit, r.grab, r.tap);
  }, [Boolean(menu)]);
  const edges = st.screens?.[SCREEN];
  const opacity = Number(cfg.opacity) > 0 && Number(cfg.opacity) < 1 ? Number(cfg.opacity) : 1;
  return (
    <>
    <LivePet
      key={pet.key}
      pet={pet}
      cfg={cfg}
      status={status}
      items={items}
      fileUrl={fileUrl}
      height={130 * (cfg.size || 1)}
      page
      storeKey={`ll-desk-pet-pos-${SCREEN}`}
      home={home}
      onHit={onHit}
      playerRef={player}
      pad={0}
      edges={edges ? { left: Boolean(edges.left), right: Boolean(edges.right) } : undefined}
      onExit={edges?.left || edges?.right ? onExit : undefined}
      onMenu={setMenu}
      alpha={st.hot || menu ? 1 : opacity}
    />
    {menu && <PetMenu at={menu} cfg={cfg} grabKey={st.grabKey} player={player} onClose={() => setMenu(null)} />}
    </>
  );
}

/** 改一项宠物设置（和设置页存的是同一份；存完宠物窗口、设置页都会跟着变） */
async function patchSettings(patch) {
  const s = await api.get('/pets/settings');
  await api.put('/pets/settings', { ...s, ...patch });
}

const ALPHAS = [1, 0.7, 0.5, 0.3];
const SIZES = [0.75, 1, 1.25, 1.5];

/** 右键宠物的菜单：透明度、不挡操作、大小、睡觉、宠物设置、收回对话页 */
function PetMenu({ at, cfg, grabKey, player, onClose }) {
  const { t } = useTranslation();
  const box = useRef(null);
  const [pos, setPos] = useState({ left: at.x, top: at.y, visibility: 'hidden' });
  // 改了马上显示（存完、配置重新取回来之前）
  const [local, setLocal] = useState({});
  const v = { opacity: Number(cfg.opacity) || 1, through: Boolean(cfg.through), size: Number(cfg.size) || 1, ...local };
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const left = Math.max(8, Math.min(at.x, window.innerWidth - r.width - 8));
    const top = at.y + r.height + 8 > window.innerHeight ? Math.max(8, at.y - r.height) : at.y;
    setPos({ left, top });
  }, [at.x, at.y]);
  const set = (k, val) => {
    setLocal((x) => ({ ...x, [k]: val }));
    patchSettings({ [k]: val }).catch(() => {});
  };
  const act = (f) => () => {
    onClose();
    f();
  };
  const sleeping = player.current?.sleeping?.() ?? false;
  const near = (x, list) => list.reduce((a, b) => (Math.abs(b - x) < Math.abs(a - x) ? b : a));
  const keyName = grabKey === 'ctrl' ? 'Ctrl' : grabKey === 'option' ? '⌥ Option' : null;
  return (
    <div className="pet-menu-backdrop" onPointerDown={(e) => e.target === e.currentTarget && onClose()} onContextMenu={(e) => e.preventDefault()}>
      <div className="pet-menu" ref={box} style={pos} role="menu" aria-label={t('pet.menu')}>
        <div className="pet-menu-row">
          <span>{t('pet.alpha')}</span>
          <Seg value={near(v.opacity, ALPHAS)} label={t('pet.alpha')} onChange={(x) => set('opacity', x)} options={ALPHAS.map((n) => [n, n === 1 ? t('pet.alpha_off') : `${Math.round((1 - n) * 100)}%`])} />
        </div>
        <div className="pet-menu-row">
          <span>{t('pet.size')}</span>
          <Seg value={near(v.size, SIZES)} label={t('pet.size')} onChange={(x) => set('size', x)} options={SIZES.map((n) => [n, `${Math.round(n * 100)}%`])} />
        </div>
        <button type="button" role="menuitemcheckbox" aria-checked={v.through} className="pet-menu-item pet-menu-check" onClick={() => set('through', !v.through)}>
          <span className="pet-menu-box" aria-hidden="true">{v.through ? '✓' : ''}</span>
          <span>
            {t('pet.through')}
            <small>{keyName ? t('pet.through_key', { key: keyName }) : t('pet.through_tray')}</small>
          </span>
        </button>
        <hr />
        <button type="button" role="menuitem" className="pet-menu-item" onClick={act(() => player.current?.toggleSleep?.())}>
          {sleeping ? t('pet.wake') : t('pet.sleep')}
        </button>
        <button type="button" role="menuitem" className="pet-menu-item" onClick={act(() => api.put('/desktop/pet', { settings: true }).catch(() => {}))}>
          {t('pet.open_settings')}
        </button>
        <button type="button" role="menuitem" className="pet-menu-item" onClick={act(() => patchSettings({ desktop: false }).catch(() => {}))}>
          {t('pet.to_page')}
        </button>
      </div>
    </div>
  );
}
