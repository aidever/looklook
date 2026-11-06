// 桌面宠物窗口的页面（`/pet-desk?screen=i`，桌面应用的 src/desktop.rs 给每块屏幕开一个透明置顶窗口，铺满那块屏幕的工作区）：
// 宠物只在一个窗口里（`owner`），可以拖到屏幕任何地方；走过屏幕边、拖到别的屏幕上松手就换到那块屏幕的窗口。
// 跟着主窗口正在看的对话（`/api/desktop/pet` 的 focus）。宠物占的框报给桌面那边，框外的鼠标穿透到下面的窗口。
// 同一台电脑上开着网页、网页里也有宠物（在前台）时，两只会互相打招呼、聊两句（见 ChatPet.jsx 的 usePeer）。
import { useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../shared/api.js';
import { LivePet, PETS_CHANGED, useInstancePet, usePeerChat, usePetsLive } from '../shared/ChatPet.jsx';
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
  // 框变了报给桌面那边；拖着的时候每帧都变，最多 80 ms 报一次
  const q = useRef({ t: 0, timer: 0, last: null });
  const onHit = (hit, grab) => {
    const s = q.current;
    s.last = { hit, grab };
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
  const edges = st.screens?.[SCREEN];
  return (
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
    />
  );
}
