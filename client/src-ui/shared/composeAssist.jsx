// 输入框的 @ 文件、/ 命令和技能（Claude Code）、$ 技能（Codex）提示，给还没有会话的草稿页用（Draft.jsx）。
// 和对话页输入框（ChatView.jsx 的 Composer）同样的写法、同样的样子；区别是按要开的工具和项目目录取
// （/draft/commands、/draft/mentions），不是按会话。
import { FileText, Folder } from 'lucide-react';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { insertMention, insertSkill, mentionAt, skillAt, slashAt } from './chat.js';
import { imeComposing } from './ime.js';

const enc = encodeURIComponent;

/**
 * `agent`：'claude' / 'codex'（别的不提示）；`workdir`：项目目录；`area`：输入框的 ref。
 * 返回 { list（放在输入框上面）, onKeyDown（处理了返回 true）, onCaret（输入框的 onSelect/onClick/onKeyUp）, trigger(ch), open }
 */
export function useComposeAssist({ client, agent, workdir, text, setText, area }) {
  const { t } = useTranslation();
  const on = Boolean(workdir) && (agent === 'claude' || agent === 'codex');
  const [caret, setCaret] = useState(0);
  const at = Math.min(caret, text.length);
  const [cmds, setCmds] = useState(null);
  useEffect(() => setCmds(null), [agent, workdir]);
  const slashWord = on && agent === 'claude' ? slashAt(text, at) : null;
  const skillQ = on && agent === 'codex' ? skillAt(text, at) : null;
  const [off, setOff] = useState(null);
  const wantCmds = slashWord != null || skillQ != null;
  useEffect(() => {
    if (!wantCmds || cmds !== null) return;
    setCmds([]);
    client.get(`/draft/commands?agent=${agent}&workdir=${enc(workdir)}`).then(
      (r) => setCmds(r?.items ?? []),
      () => {},
    );
  }, [wantCmds, cmds]);
  const mention = on && slashWord == null && !skillQ ? mentionAt(text, at) : null;
  const mentQ = mention && mention.start !== off ? mention.q : null;
  const [found, setFound] = useState({ q: null, items: [] });
  useEffect(() => {
    if (mentQ == null) return undefined;
    let alive = true;
    const timer = setTimeout(() => {
      client.get(`/draft/mentions?workdir=${enc(workdir)}&q=${enc(mentQ)}`).then(
        (r) => alive && setFound({ q: mentQ, items: r?.items ?? [] }),
        () => {},
      );
    }, 120);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [mentQ, workdir]);
  const items = useMemo(() => {
    if (slashWord != null && off !== 0) {
      const q = slashWord.toLowerCase();
      const all = (cmds ?? []).filter((c) => c.name.startsWith('/'));
      const hit = all.filter((c) => c.name.toLowerCase().startsWith(q));
      return (hit.length ? hit : all.filter((c) => c.name.toLowerCase().includes(q.slice(1)))).slice(0, 40).map((c) => ({ ...c, kind: 'slash' }));
    }
    if (skillQ && skillQ.start !== off) {
      const q = skillQ.q.toLowerCase();
      const hit = (cmds ?? []).filter((c) => c.name.startsWith('$') && c.name.slice(1).toLowerCase().includes(q));
      hit.sort((a, b) => Number(!a.name.slice(1).toLowerCase().startsWith(q)) - Number(!b.name.slice(1).toLowerCase().startsWith(q)));
      return hit.slice(0, 40).map((c) => ({ ...c, kind: 'skill' }));
    }
    if (mentQ != null && found.q != null) return found.items.map((m) => ({ ...m, kind: 'file' }));
    return [];
  }, [slashWord, skillQ?.start, skillQ?.q, off, cmds, mentQ, found]);
  const [pick, setPick] = useState(0);
  useEffect(() => setPick(0), [items.length, slashWord, skillQ?.q, mentQ]);
  // 程序改了字以后把光标放好（渲染完、下一个按键之前）
  const pending = useRef(null);
  useLayoutEffect(() => {
    const p = pending.current;
    const el = area.current;
    if (p == null || !el) return;
    pending.current = null;
    el.focus();
    el.setSelectionRange(p, p);
  });
  const caretTo = (p) => {
    pending.current = p;
    setCaret(p);
  };
  const choose = (x) => {
    if (!x) return;
    let r;
    if (x.kind === 'slash') {
      const rest = text.slice(slashWord.length).replace(/^\s+/, '');
      r = { text: `${x.name} ${rest}`, caret: x.name.length + 1 };
    } else if (x.kind === 'skill') r = insertSkill(text, skillQ.start, at, x.name);
    else r = insertMention(text, mention.start, at, x.path);
    setOff(null);
    setText(r.text);
    caretTo(r.caret);
  };
  const onKeyDown = (e) => {
    if (!items.length || imeComposing(e) || e.altKey || e.ctrlKey || e.metaKey) return false;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      setPick((p) => (p + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length);
      return true;
    }
    if ((e.key === 'Enter' || e.key === 'Tab') && !e.shiftKey) {
      e.preventDefault();
      choose(items[pick] ?? items[0]);
      return true;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      setOff(slashWord != null ? 0 : (skillQ ?? mention).start);
      return true;
    }
    return false;
  };
  const onCaret = (e) => setCaret(e.currentTarget.selectionStart ?? text.length);
  // 输入框下面的 @、技能按钮（手机上打 @、$、/ 不方便）：在光标处插入，弹出同样的提示
  const trigger = (ch) => {
    const el = area.current;
    const p = el ? el.selectionStart ?? text.length : text.length;
    setOff(null);
    if (ch === '/') {
      const rest = text.replace(/^\s+/, '');
      setText(rest ? `/ ${rest}` : '/');
      caretTo(1);
      return;
    }
    const pad = p > 0 && !/\s/.test(text[p - 1]) ? ' ' : '';
    setText(`${text.slice(0, p)}${pad}${ch}${text.slice(p)}`);
    caretTo(p + pad.length + 1);
  };
  const list = items.length ? (
    <ul className={`chat-slash draft-assist ${items[0].kind === 'file' ? 'chat-mention' : ''}`} role="listbox" aria-label={t(items[0].kind === 'file' ? 'chat.mention' : 'chat.skill_pick')}>
      {items.map((x, n) => (
        <li key={`${x.kind}:${x.path ?? x.name}`}>
          <button type="button" aria-selected={pick === n} onMouseDown={(e) => e.preventDefault()} onClick={() => choose(x)} title={x.path ?? x.name}>
            {x.kind === 'file' ? (
              <>
                {x.dir ? <Folder size={15} aria-hidden="true" /> : <FileText size={15} aria-hidden="true" />}
                <span className="ellipsis">{x.path}</span>
              </>
            ) : (
              <>
                <b>{x.name}</b>
                <span className="chat-note ellipsis">{x.description}</span>
                {x.source && <span className="badge">{t(`chat.cmd_source_${x.source}`, { defaultValue: '' })}</span>}
              </>
            )}
          </button>
        </li>
      ))}
    </ul>
  ) : null;
  return { on, list, onKeyDown, onCaret, trigger };
}
