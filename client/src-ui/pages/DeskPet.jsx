// 桌面宠物窗口的页面（`/pet-desk`，桌面应用的 src/desktop.rs 开的透明置顶窗口，铺满主屏）：
// 只放一只宠物，可以拖到屏幕任何地方；跟着主窗口正在看的对话（`/api/desktop/pet` 的 focus）。
// 宠物占的框报给桌面那边，框外的鼠标穿透到下面的窗口。
import { useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../shared/api.js';
import { LivePet, PETS_CHANGED, useInstancePet } from '../shared/ChatPet.jsx';
import { useChat } from './ChatView.jsx';

const enc = encodeURIComponent;
const IDLE = { phase: 'idle' };

export default function DeskPet() {
  const [focus, setFocus] = useState(null);
  // 背景透明（窗口本身是透明的）
  useEffect(() => {
    document.documentElement.classList.add('pet-desk-root');
    return () => document.documentElement.classList.remove('pet-desk-root');
  }, []);
  // 主窗口换了对话、设置改了：隔一会儿看一下
  useEffect(() => {
    let dead = false;
    const check = () => api.get('/desktop/pet').then((r) => !dead && setFocus(r.focus ?? null), () => {});
    check();
    const a = setInterval(check, 2000);
    const b = setInterval(() => window.dispatchEvent(new Event(PETS_CHANGED)), 30000);
    return () => {
      dead = true;
      clearInterval(a);
      clearInterval(b);
    };
  }, []);
  return focus ? <FocusPet id={focus} /> : <DefaultPet />;
}

function FocusPet({ id }) {
  const cfg = useInstancePet(id, true);
  const chat = useChat(id, true);
  const pet = cfg?.pet;
  const fileUrl = useMemo(() => (pet ? (key) => `/instances/${id}/pet/file/${enc(pet.source)}/${enc(pet.id)}/${key}` : null), [id, pet?.key]);
  if (!pet) return <DefaultPet />;
  return <Desk pet={pet} cfg={cfg} status={chat.status ?? IDLE} items={chat.items} fileUrl={fileUrl} />;
}

/** 还没打开过对话：用设置里的默认宠物，只是待着 */
function DefaultPet() {
  const [cfg, setCfg] = useState(null);
  useEffect(() => {
    let dead = false;
    const load = async () => {
      try {
        const s = await api.get('/pets/settings');
        const key = s.enabled ? s.default ?? Object.values(s.byAgent ?? {}).find(Boolean) : null;
        if (!key) return !dead && setCfg(null);
        const [source, pid] = key.split(':');
        const [m, gen] = await Promise.all([api.get(`/pets/${enc(source)}/${enc(pid)}`), api.get(`/pets/${enc(source)}/${enc(pid)}/lines`).catch(() => null)]);
        if (!dead) setCfg({ pet: { key, source, id: pid, manifest: m.manifest ?? m }, size: s.size, lines: s.lines, chatter: s.chatter, gen });
      } catch {
        if (!dead) setCfg(null);
      }
    };
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
  return <Desk pet={pet} cfg={cfg} status={IDLE} items={[]} fileUrl={fileUrl} />;
}

function Desk({ pet, cfg, status, items, fileUrl }) {
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
  return <LivePet key={pet.key} pet={pet} cfg={cfg} status={status} items={items} fileUrl={fileUrl} height={130 * (cfg.size || 1)} page storeKey="ll-desk-pet-pos" home={home} onHit={onHit} />;
}
