// 分享对话（只读）：整段，或者挑几条（一条 = 你发的一句 + AI 这一轮做的），导出成一个独立的网页或 Markdown。
// 网页里只有文字和样式，没有脚本、不连任何地方：对方只能看，不能接着聊，也碰不到这台电脑。
// 挑的时候长的先折成几行，点“展开”看全文；Shift 点一下连着选一段。
import { ChevronDown, ChevronUp, ClipboardCopy, Download, LoaderCircle, Share2 } from 'lucide-react';
import { useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { copyText } from '../shared/chatCopy.js';
import { replyText, shareHtml, shareMarkdown, shareName, turnsOf } from '../shared/chatShare.js';
import { errorText } from '../shared/i18n.js';
import { Modal, useToast } from '../shared/ui.jsx';

let libs = null;
const loadLibs = () => {
  libs ??= Promise.all([import('marked'), import('dompurify')]).then(([m, d]) => ({ marked: m.marked, purify: d.default }));
  return libs;
};

/** 折起来时显示几行；超过就给“展开” */
const CLAMP = 3;
const isLong = (s) => s.split('\n').length > CLAMP || s.length > 180;

function Turn({ turn, n, on, open, onPick, onOpen }) {
  const { t } = useTranslation();
  const me = turn.user?.text.trim() ?? '';
  // 预览里去掉代码块的 ``` 行，免得占掉折叠的那几行
  const ai = replyText(turn).replace(/^\s*```.*$\n?/gm, '').trim();
  const long = isLong(me) || isLong(ai);
  return (
    <li className="share-turn" data-on={on ? '' : undefined} data-open={open ? '' : undefined}>
      <button type="button" className="share-turn-main" onClick={(e) => onPick(e.shiftKey)} aria-pressed={on}>
        <span className="share-check" aria-hidden="true" />
        <span className="share-turn-body">
          <span className="share-turn-head">
            <b>#{n}</b>
            <span>{t('share.meta', { chars: turn.chars.toLocaleString(), count: turn.steps })}</span>
          </span>
          {turn.user && (
            <span className="share-me">
              {me || (turn.user.images ? t('chat.images', { n: turn.user.images }) : '')}
            </span>
          )}
          {ai ? <span className="share-ai">{ai}</span> : turn.steps > 0 && <span className="share-ai share-ai-none">{t('share.only_steps', { count: turn.steps })}</span>}
        </span>
      </button>
      {long && (
        <button type="button" className="share-more" onClick={onOpen} aria-expanded={open}>
          {open ? <ChevronUp size={14} aria-hidden="true" /> : <ChevronDown size={14} aria-hidden="true" />}
          {t(open ? 'share.collapse' : 'share.expand')}
        </button>
      )}
    </li>
  );
}

export default function ShareDialog({ items, more, loadAll, title, agent, onClose }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const turns = useMemo(() => turnsOf(items), [items]);
  const [mode, setMode] = useState('all');
  const [picked, setPicked] = useState(() => new Set());
  const [opened, setOpened] = useState(() => new Set());
  const [tools, setTools] = useState(true);
  const [busy, setBusy] = useState(null);
  const last = useRef(null);
  const chosen = mode === 'all' ? turns : turns.filter((x) => picked.has(x.key));
  const pick = (i, range) => {
    const key = turns[i].key;
    // 先取出来：setPicked 的函数可能在下面改了 last 之后才执行
    const from = last.current;
    setPicked((s) => {
      const n = new Set(s);
      if (range && from != null) {
        // Shift：从上次点的那条到这条，都跟这条点完后的状态一样
        const want = !n.has(key);
        const [a, b] = [Math.min(from, i), Math.max(from, i)];
        for (let j = a; j <= b; j++) want ? n.add(turns[j].key) : n.delete(turns[j].key);
      } else if (n.has(key)) n.delete(key);
      else n.add(key);
      return n;
    });
    last.current = i;
  };
  const toggleOpen = (key) =>
    setOpened((s) => {
      const n = new Set(s);
      n.has(key) ? n.delete(key) : n.add(key);
      return n;
    });
  const name = title || t('share.untitled');
  const mdText = () => shareMarkdown(chosen, { user: t('chat.export_me'), agent, title: name, tools, images: (n) => `[${t('chat.images', { n })}]` });
  const htmlText = async () => {
    const { marked, purify } = await loadLibs();
    const md = (s) => purify.sanitize(marked.parse(s, { async: false }), { FORBID_TAGS: ['style', 'form', 'input', 'button'], FORBID_ATTR: ['style'] });
    return shareHtml(chosen, {
      md,
      tools,
      lang: i18n.language,
      labels: {
        title: name,
        user: t('chat.export_me'),
        agent,
        readonly: t('share.page_readonly'),
        exported: (when) => t('share.page_exported', { when }),
        steps: (count) => t('share.page_steps', { count }),
        images: (n) => t('chat.images', { n }),
        more: (count) => t('share.page_more', { count }),
      },
    });
  };
  const run = async (what, fn) => {
    setBusy(what);
    try {
      await fn();
    } catch (e) {
      if (e?.name !== 'AbortError') toast(errorText(t, e), 'error');
    } finally {
      setBusy(null);
    }
  };
  const save = (text, type, ext) => {
    const url = URL.createObjectURL(new Blob([text], { type }));
    const a = document.createElement('a');
    a.href = url;
    a.download = shareName(name, ext);
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  const download = () => run('html', async () => save(await htmlText(), 'text/html;charset=utf-8', 'html'));
  const copyMd = () =>
    run('md', async () => {
      const text = mdText();
      if (await copyText(text)) toast(t('share.copied', { count: chosen.length }));
      else window.prompt('', text);
    });
  // 手机上直接交给系统的分享（飞书、微信……），带着网页文件
  const canShare = typeof navigator !== 'undefined' && typeof navigator.canShare === 'function' && navigator.canShare({ files: [new File(['x'], 'x.html', { type: 'text/html' })] });
  const shareFile = () =>
    run('share', async () => {
      const file = new File([await htmlText()], shareName(name, 'html'), { type: 'text/html' });
      await navigator.share({ files: [file], title: name });
    });
  const none = chosen.length === 0;
  return (
    <Modal open onClose={onClose} title={t('share.title')} wide className="share-modal">
      <p className="muted small share-intro">{t('share.intro')}</p>
      <div className="share-seg" role="tablist">
        <button type="button" role="tab" aria-selected={mode === 'all'} onClick={() => setMode('all')}>
          {t('share.mode_all', { count: turns.length })}
        </button>
        <button type="button" role="tab" aria-selected={mode === 'pick'} onClick={() => setMode('pick')}>
          {t('share.mode_pick')}
        </button>
      </div>
      {more && (
        <p className="share-older small">
          {t('share.older')}{' '}
          <button type="button" className="btn btn-plain btn-sm" disabled={busy === 'older'} onClick={() => run('older', loadAll)}>
            {busy === 'older' && <LoaderCircle size={13} className="spin" aria-hidden="true" />}
            {t('share.load_older')}
          </button>
        </p>
      )}
      {mode === 'pick' && (
        <>
          <div className="share-pickbar small">
            <span className="grow">{t('share.picked', { count: picked.size, total: turns.length })}</span>
            <button type="button" className="btn btn-plain btn-sm" onClick={() => setPicked(new Set(turns.map((x) => x.key)))}>
              {t('share.all')}
            </button>
            <button type="button" className="btn btn-plain btn-sm" onClick={() => setPicked(new Set())} disabled={!picked.size}>
              {t('share.none')}
            </button>
          </div>
          <ol className="share-turns">
            {turns.map((turn, i) => (
              <Turn key={turn.key} turn={turn} n={i + 1} on={picked.has(turn.key)} open={opened.has(turn.key)} onPick={(range) => pick(i, range)} onOpen={() => toggleOpen(turn.key)} />
            ))}
          </ol>
          <p className="chat-note share-tip">{t('share.range_tip')}</p>
        </>
      )}
      <label className="share-opt small">
        <input type="checkbox" checked={tools} onChange={(e) => setTools(e.target.checked)} />
        {t('share.with_steps')}
      </label>
      <div className="modal-actions share-acts">
        {canShare && (
          <button type="button" className="btn" onClick={shareFile} disabled={none || !!busy}>
            {busy === 'share' ? <LoaderCircle size={15} className="spin" aria-hidden="true" /> : <Share2 size={15} aria-hidden="true" />}
            {t('share.system')}
          </button>
        )}
        <button type="button" className="btn" onClick={copyMd} disabled={none || !!busy}>
          <ClipboardCopy size={15} aria-hidden="true" />
          {t('share.copy_md')}
        </button>
        <button type="button" className="btn btn-primary" onClick={download} disabled={none || !!busy}>
          {busy === 'html' ? <LoaderCircle size={15} className="spin" aria-hidden="true" /> : <Download size={15} aria-hidden="true" />}
          {t('share.download_html', { count: chosen.length })}
        </button>
      </div>
    </Modal>
  );
}
