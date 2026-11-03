// 对话界面角落的宠物（docs/PET_PACK.md）：用哪只按“设置 → 宠物”里给这种对话配的，跟着对话状态换动作。
// 鼠标移上去有个小叉可以藏起来，藏起来后留一个很淡的小点，点了叫回来（记在这个浏览器里）。
import { useEffect, useMemo, useRef, useState } from 'react';
import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { api } from './api.js';
import { isTouch } from './KeyBar.jsx';
import PetPlayer from './pet/PetPlayer.jsx';
import { agentState, textOf } from './pet/brain.js';

/** 设置页改了宠物（换了、导入、删除）：对话页重新取 */
export const PETS_CHANGED = 'll-pets-changed';
const HIDE_KEY = 'll-chat-pet';

function readHidden() {
  try {
    return localStorage.getItem(HIDE_KEY) === 'off';
  } catch {
    return false;
  }
}

/** 一个对话用的宠物：`{ pet: { key, source, id, manifest } | null, size, lines }` */
function usePet(id, active) {
  const [cfg, setCfg] = useState(null);
  useEffect(() => {
    if (!active) return undefined;
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

export default function ChatPet({ id, active, status, touch }) {
  const { t, i18n } = useTranslation();
  const cfg = usePet(id, active);
  const [hidden, setHidden] = useState(readHidden);
  const prev = useRef(null);
  const seq = useRef(0);
  // 对话状态 → 宠物状态；跟上一次比，认出“刚做完”“刚出错”“一个工具刚结束”
  const agent = useMemo(() => {
    const r = agentState(status, prev.current);
    prev.current = status ? { phase: status.phase, tool: status.tool ?? null, error: status.error ?? null } : null;
    seq.current += 1;
    return { ...r, n: seq.current };
  }, [status?.phase, status?.tool, status?.error]);
  const pet = cfg?.pet;
  const fileUrl = useMemo(() => (pet ? (key) => `/instances/${id}/pet/file/${encodeURIComponent(pet.source)}/${encodeURIComponent(pet.id)}/${key}` : null), [id, pet?.key]);
  const toggle = (v) => {
    setHidden(v);
    try {
      if (v) localStorage.setItem(HIDE_KEY, 'off');
      else localStorage.removeItem(HIDE_KEY);
    } catch {
      // 存不了（隐私模式）：这次照样生效
    }
  };
  // 嵌在终端页里的对话标签总是传 touch=false，按设备再认一次（手机上人物小一点）
  if (!active || !pet) return null;
  if (hidden) {
    return (
      <div className="pet-layer">
        <button type="button" className="pet-peek" onClick={() => toggle(false)} title={t('pet.show')} aria-label={t('pet.show')} />
      </div>
    );
  }
  const name = textOf(pet.manifest.name, i18n.language);
  return (
    <PetPlayer
      key={pet.key}
      manifest={pet.manifest}
      fileUrl={fileUrl}
      agent={agent}
      height={(touch || isTouch() ? 96 : 130) * (cfg.size || 1)}
      lines={cfg.lines !== false}
      lang={i18n.language}
      label={t('pet.label', { name })}
      storeKey="ll-chat-pet-x"
    >
      <button type="button" className="pet-hide" onClick={() => toggle(true)} title={t('pet.hide')} aria-label={t('pet.hide')}>
        <X size={11} />
      </button>
    </PetPlayer>
  );
}
