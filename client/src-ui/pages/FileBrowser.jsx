// 文件浏览：终端页的“文件”标签（传 id：从终端的当前目录开始）和管理台的“文件”页（不传 id：从主目录开始）。
// 浏览这台电脑上的文件，预览代码和文字（CodeMirror，CodeView.jsx）、图片（点开看大图）、音视频（Vidstack，MediaPlayer.jsx）、Markdown
// 和办公文件（PDF、Word、Excel、CSV，OfficePreview.jsx），下载、把路径填进终端 / 对话输入框；新建文件夹、重命名、删除、上传（拖进列表也行）。
// 和会话联动（docs/FILES_LINKAGE_2026-11-07.md）：每个会话的文件标签记住看到哪（文件夹、打开的文件、隐藏文件开关），
// 会话里 Git 改过的文件标出来，对话里点开的文件能“在文件中显示”，文件夹上能直接开新会话。
// 终端标签的浏览、预览、下载挂在 /api/instances/{id}/… 下面，多台电脑时按终端路由到它所在的电脑（服务端 edge/route.rs）；
// “文件”页与文件管理用 /api/fs/…，多台电脑时发给选中的那台（X-LL-Device）。
import { ArrowUp, ArrowUpDown, BookOpen, CirclePlay, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, ClipboardPaste, Clock, Code2, Copy, CopyPlus, Download, Eye, EyeOff, File, FileArchive, FilePen, FileAudio, FileCode, FileSpreadsheet, FileText, FileVideo, Folder, FolderInput, FolderPlus, Home, Image as ImageIcon, Link2, LocateFixed, Maximize2, MessageSquarePlus, MessagesSquare, PenLine, RefreshCw, Save, Scissors, ScrollText, Search, Share2, Sparkles, Star, TextCursorInput, Trash2, Upload, WrapText, X, Zap } from 'lucide-react';
import { lazy, Suspense, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';

import { deviceApi, fetchRaw, plainLinksWork, rtcMode, saveViaFrame, selectedDevice, terminalDevice } from '../shared/api.js';
import { connManaged, isOnline, useConn } from '../shared/conn.js';
import { errorText } from '../shared/i18n.js';
import ImageViewer from '../shared/ImageViewer.jsx';
import Markdown, { isMarkdown } from '../shared/Markdown.jsx';
import { OFFICE_MAX, officeKind, parseCsv, WHOLE } from '../shared/office.js';
import { relativeTime } from '../shared/time.js';
import { Button, ErrorNote, Field, Modal, useConfirm, useToast } from '../shared/ui.jsx';
import { dirUploadStore } from '../shared/uploader.js';
import { touchUi, useLayer } from '../shared/layers.js';
import { sendToChat } from '../shared/chatInsert.js';
import { sessionMode, sessionTitle, useSessions } from '../shared/sessions.jsx';
import { formatBytes, quotePath } from '../shared/xfer.js';
import { renderOffice, SheetView } from './OfficePreview.jsx';

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|ico|avif)$/i;
/** 像日志的文件：预览时提供“跟随” */
const LOGLIKE = /\.(log|out|err|jsonl|ndjson)$|(^|[._-])log$/i;
export const VIDEO = /\.(mp4|m4v|webm|mov|mkv|ogv)$/i;
export const AUDIO = /\.(mp3|m4a|aac|wav|flac|ogg|oga|opus)$/i;
/** 远程又不能用普通链接时（多台电脑、不经 WebRTC）音视频只能整个取回来再放：太大就不放了 */
const MEDIA_FETCH_MAX = 512 * 1024 * 1024;
const Player = lazy(() => import('../shared/MediaPlayer.jsx'));
const CodeView = lazy(() => import('../shared/CodeView.jsx'));
const PREVIEW_MAX = 1024 * 1024; // 与 transfer::PREVIEW_MAX 一致
const ACTIVE = ['queued', 'uploading', 'retrying'];

/** 终端标签（id）或“文件”页（无 id）用的接口地址。 */
export function endpoints(id) {
  const q = (path) => `?path=${encodeURIComponent(path)}`;
  return id
    ? { id, list: (path, hidden) => `/instances/${id}/fs${q(path)}&hidden=${hidden ? 1 : 0}`, preview: (p) => `/instances/${id}/preview${q(p)}`, download: (p) => `/instances/${id}/download${q(p)}`, plain: true }
    : { list: (path, hidden) => `/fs/list${q(path)}&files=1&hidden=${hidden ? 1 : 0}`, preview: (p) => `/fs/preview${q(p)}`, download: (p) => `/fs/download${q(p)}`, plain: plainLinksWork() };
}

/** 面包屑：`/a/b` → [/, /a, /a/b]；`C:\a\b` → [C:\, C:\a, C:\a\b]。 */
export function crumbs(path, sep) {
  if (!path) return [];
  const win = sep === '\\';
  const parts = path.split(win ? /[\\/]/ : '/').filter(Boolean);
  const out = [];
  let acc = win ? '' : '/';
  if (!win) out.push({ name: '/', path: '/' });
  parts.forEach((p, i) => {
    acc = win ? (i === 0 ? `${p}\\` : `${acc}${acc.endsWith('\\') ? '' : '\\'}${p}`) : `${acc}${acc.endsWith('/') ? '' : '/'}${p}`;
    out.push({ name: p, path: acc });
  });
  return out;
}

const join = (dir, name, sep) => (dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`);

/** 文件所在的文件夹：`/a/b.txt` → `/a`，`C:\\a\\b.txt` → `C:\\a`，`/b.txt` → `/` */
export function parentDir(path) {
  const sep = !path.includes('/') && path.includes('\\') ? '\\' : '/';
  const i = path.lastIndexOf(sep);
  if (i < 0) return '';
  const dir = path.slice(0, i);
  return !dir ? sep : /^[A-Za-z]:$/.test(dir) ? `${dir}\\` : dir;
}

/**
 * 音视频的播放地址：能用普通链接时直接指向下载接口（`inline=1`，边下边播）；远程经 WebRTC 时同样，
 * 用 `__lldev` 指定电脑（Service Worker 转发，Range 照常）。都不行时返回 null（只能取回来再放）。
 */
export function mediaSrc(ep, path, device) {
  const href = `/api${ep.download(path)}&inline=1`;
  if (rtcMode) return device ? `${href}&__lldev=${encodeURIComponent(device)}` : href;
  return ep.plain ? href : null;
}

/** 文本预览：按 UTF-8 解码，含 NUL 或解不出来就当二进制。 */
function decodeText(buf) {
  const bytes = new Uint8Array(buf);
  if (bytes.subarray(0, 8192).includes(0)) return null;
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    // 截断可能切在多字节字符中间：去掉末尾几个字节再试
    try {
      return new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, Math.max(0, bytes.length - 3)));
    } catch {
      return null;
    }
  }
}

/** 下载：能用普通链接时直接打开；远程多台电脑时先取回再保存（普通链接带不了 X-LL-Device）。 */
function DownloadLink({ ep, entry, size = 15 }) {
  const { t } = useTranslation();
  const toast = useToast();
  const label = t('files.download');
  if (ep.plain) {
    return (
      <a className="btn btn-plain btn-icon" href={`/api${ep.download(entry.path)}`} download={entry.name} aria-label={label} title={label} onClick={() => toast(t('fb.dl_start_one', { name: entry.name }))}>
        <Download size={size} />
      </a>
    );
  }
  const save = async () => {
    toast(t('fb.dl_start_one', { name: entry.name }));
    // 远程（WebRTC）：经 iframe 边收边存，不整个取进内存
    if (rtcMode) return saveViaFrame(`/api${ep.download(entry.path)}`, selectedDevice());
    try {
      const blob = await (await fetchRaw(ep.download(entry.path))).blob();
      const url = URL.createObjectURL(blob);
      const a = Object.assign(document.createElement('a'), { href: url, download: entry.name });
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 30000);
    } catch (e) {
      toast(errorText(t, e), 'error');
    }
  };
  return (
    <button type="button" className="btn btn-plain btn-icon" onClick={save} aria-label={label} title={label}>
      <Download size={size} />
    </button>
  );
}

/**
 * 一个文件的预览（“文件”页右边；对话页里点到的文件也用它，见 ChatView 的 FileViewer）。
 * 对话页另外传：focus（{ from, to }：标出这几行并滚过去）、extra（标题栏关闭按钮前的按钮）。
 * `device`：文件在哪台电脑（远程播放音视频时用）；`active`：看不见时（切到别的标签、别的会话）把正在放的停下。
 * `insertLabel`：填路径按钮的说明（对话会话里填进输入框，其他填进终端）。
 */
export function Preview({ ep, entry, onClose, onInsert, insertLabel, sep, focus = null, extra = null, device = null, active = true, onAsk }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const [state, setState] = useState({ loading: true });
  const dev = device ?? (ep.id ? terminalDevice(ep.id) : selectedDevice());
  const ops = deviceApi(dev ?? undefined);
  // 编辑（§9.4 J5）：草稿放在 ref 里（每个字都 setState 会让编辑器重建）；dirty 只用来显示“未保存”
  const [editing, setEditing] = useState(false);
  const [editBase, setEditBase] = useState('');
  const draft = useRef('');
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [sel, setSel] = useState(null);
  const [follow, setFollow] = useState(false);
  const shareFile = useRef(null);
  // 没传 onAsk：对话里点开的文件，“让 AI 改”填进这个会话的对话输入框；传 null 表示不提供
  const ask = onAsk === undefined ? (ep.id ? (text) => {
    window.dispatchEvent(new CustomEvent(`ll-chat-insert:${ep.id}`, { detail: text }));
    onClose?.();
  } : null) : onAsk;
  const [wrap, setWrap] = useState(true);
  const md = isMarkdown(entry.name);
  const office = officeKind(entry.name);
  const csv = office === 'csv';
  // Markdown 看排版效果 / 源码，CSV 看表格 / 源码；要标出某几行时先看源码
  const [rendered, setRendered] = useState(!focus?.from);
  const image = IMAGE.test(entry.name);
  const video = VIDEO.test(entry.name);
  const audio = !video && AUDIO.test(entry.name);
  const media = video || audio;
  const whole = WHOLE.includes(office);
  const [zoom, setZoom] = useState(false);
  useEffect(() => {
    let alive = true;
    let url = null;
    if (media) {
      const src = mediaSrc(ep, entry.path, device);
      if (src) setState({ media: src });
      else if ((entry.size ?? 0) > MEDIA_FETCH_MAX) setState({ office: 'media_big' });
      else {
        setState({ loading: true });
        fetchRaw(ep.download(entry.path))
          .then((r) => r.blob())
          .then((b) => {
            url = URL.createObjectURL(b);
            if (alive) setState({ media: url });
          })
          .catch((error) => alive && setState({ error }));
      }
      return () => {
        alive = false;
        if (url) URL.revokeObjectURL(url);
      };
    }
    // 浏览器不能在页面里显示 PDF（安卓上的 Chrome 等）时不取回来，提示下载
    const noPdf = office === 'pdf' && navigator.pdfViewerEnabled === false;
    if (office === 'other' || noPdf || (whole && entry.size > OFFICE_MAX)) {
      setState({ office: office === 'other' ? 'other' : noPdf ? 'no_pdf' : 'too_big' });
      return undefined;
    }
    setState({ loading: true });
    // 图片、PDF、Word、Excel 取整个文件（预览接口只给前 1 MiB）
    fetchRaw(image || whole ? ep.download(entry.path) : ep.preview(entry.path))
      .then(async (res) => {
        if (image) {
          url = URL.createObjectURL(await res.blob());
          if (alive) setState({ url });
          return;
        }
        if (whole) {
          const view = await renderOffice(office, await res.blob()).catch(() => null);
          url = view?.url ?? null;
          if (alive) setState(view ? { view } : { office: 'failed' });
          return;
        }
        const size = Number(res.headers.get('x-looklook-size') ?? entry.size ?? 0);
        const mtime = Number(res.headers.get('x-looklook-mtime') ?? 0) || null;
        const text = decodeText(await res.arrayBuffer());
        if (alive) setState({ text, truncated: size > PREVIEW_MAX, size, mtime });
      })
      .catch((error) => alive && setState({ error }));
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [entry.path]);
  const dir = entry.path.slice(0, Math.max(0, entry.path.lastIndexOf(sep))) || sep;
  const showMd = md && rendered && state.text != null;
  const showSheet = csv && rendered && state.text != null;
  const csvSheets = useMemo(() => (showSheet ? parseCsv(state.text, entry.name) : null), [showSheet, state.text, entry.name]);
  const insertText = insertLabel ?? t('fb.insert');
  // 跟随日志（§9.4 J4）：每 2 秒取新增的内容接在后面，滚到最后；看不见时停
  useEffect(() => {
    if (!follow || !active || state.text == null) return undefined;
    let alive = true;
    let from = state.truncated ? 0 : state.size ?? 0;
    let timer = 0;
    const tick = () =>
      ops
        .get(`/fs/tail?path=${encodeURIComponent(entry.path)}&from=${from}`)
        .then((r) => {
          if (!alive) return;
          from = r.size;
          if (r.reset || r.text) setState((st) => ({ ...st, text: (r.reset ? r.text : st.text + r.text).slice(-2_000_000), truncated: false, size: r.size }));
        })
        .catch((e) => {
          if (!alive) return;
          setFollow(false);
          toast(errorText(t, e.status === 404 ? { code: 'CLIENT_TOO_OLD' } : e), 'error');
        })
        .finally(() => {
          if (alive) timer = setTimeout(tick, 2000);
        });
    tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [follow, active, entry.path]);
  const plainText = state.text != null && !image && !media && !state.view && !state.office;
  const canEdit = plainText && !state.truncated && (state.size ?? entry.size ?? 0) <= PREVIEW_MAX && !follow;
  const canFollow = plainText && !editing && (LOGLIKE.test(entry.name) || (entry.mtime_ms && Date.now() - entry.mtime_ms < 10 * 60_000));
  const startEdit = () => {
    draft.current = state.text;
    setEditBase(state.text);
    setDirty(false);
    setRendered(false);
    setEditing(true);
  };
  const stopEdit = async () => {
    if (dirty && !(await confirm({ title: t('fb.edit_discard_title'), body: t('fb.edit_discard_body'), confirm: t('fb.edit_discard'), danger: true }))) return false;
    setEditing(false);
    setDirty(false);
    return true;
  };
  const save = async (force = false) => {
    if (saving) return;
    setSaving(true);
    try {
      const r = await ops.post('/fs/write', { path: entry.path, content: draft.current, if_mtime: force ? undefined : state.mtime ?? undefined });
      setState((st) => ({ ...st, text: draft.current, mtime: r.mtime_ms, size: new TextEncoder().encode(draft.current).length }));
      setDirty(false);
      setEditing(false);
      toast(t('fb.saved', { name: entry.name }));
    } catch (e) {
      if (e.code === 'FILE_CHANGED') {
        setSaving(false);
        if (await confirm({ title: t('fb.conflict_title'), body: t('fb.conflict_body', { name: entry.name }), confirm: t('fb.overwrite'), danger: true })) return save(true);
        return undefined;
      }
      toast(errorText(t, e.status === 404 ? { code: 'CLIENT_TOO_OLD' } : e), 'error');
    }
    setSaving(false);
    return undefined;
  };
  const close = async () => {
    if (editing && !(await stopEdit())) return;
    onClose?.();
  };
  const askAi = () => {
    if (!ask || !sel) return;
    const code = sel.text.length > 6000 ? `${sel.text.slice(0, 6000)}\n…` : sel.text;
    ask(t('fb.ask_ai_text', { path: entry.path, lines: sel.from === sel.to ? `${sel.from}` : `${sel.from}-${sel.to}`, code }));
  };
  const canShare = touchUi && typeof navigator.canShare === 'function' && !media;
  const share = async () => {
    try {
      if (!shareFile.current) {
        const blob = await (await fetchRaw(ep.download(entry.path))).blob();
        shareFile.current = new File([blob], entry.name, { type: blob.type || 'application/octet-stream' });
      }
      const data = { files: [shareFile.current], title: entry.name };
      if (!navigator.canShare(data)) {
        toast(t('fb.share_unsupported'), 'error');
        return;
      }
      await navigator.share(data);
    } catch (e) {
      if (e?.name === 'AbortError') return;
      // 取文件花了太久，浏览器不认这次点击了：文件已经在手里，再点一次就能分享
      if (e?.name === 'NotAllowedError' && shareFile.current) toast(t('fb.share_again'));
      else toast(errorText(t, e), 'error');
    }
  };
  return (
    <div className="fb-preview" role="region" aria-label={entry.name}>
      <div className="fb-preview-head">
        {media ? <CirclePlay size={16} aria-hidden="true" /> : image ? <ImageIcon size={16} aria-hidden="true" /> : office === 'xlsx' || csv ? <FileSpreadsheet size={16} aria-hidden="true" /> : <FileText size={16} aria-hidden="true" />}
        <b className="ellipsis grow" title={entry.path}>{entry.name}</b>
        {entry.size != null && <span className="muted small">{formatBytes(entry.size)}</span>}
        {(md || csv) && state.text != null && (
          <button
            type="button"
            className="btn btn-plain btn-icon"
            onClick={() => setRendered(!rendered)}
            aria-pressed={!rendered}
            aria-label={t(rendered ? 'fb.md_source' : csv ? 'fb.csv_table' : 'fb.md_rendered')}
            title={t(rendered ? 'fb.md_source' : csv ? 'fb.csv_table' : 'fb.md_rendered')}
          >
            {rendered ? <Code2 size={16} /> : csv ? <FileSpreadsheet size={16} /> : <BookOpen size={16} />}
          </button>
        )}
        {image && state.url && (
          <button type="button" className="btn btn-plain btn-icon" onClick={() => setZoom(true)} aria-label={t('fb.zoom')} title={t('fb.zoom')}>
            <Maximize2 size={16} />
          </button>
        )}
        {!image && !showMd && !showSheet && state.text != null && (
          <button type="button" className={`btn btn-plain btn-icon ${wrap ? 'on' : ''}`} onClick={() => setWrap(!wrap)} aria-pressed={wrap} aria-label={t('fb.wrap')} title={t('fb.wrap')}>
            <WrapText size={16} />
          </button>
        )}
        {editing ? (
          <>
            {dirty && <span className="muted small">{t('fb.unsaved')}</span>}
            <button type="button" className="btn btn-plain btn-sm" onClick={stopEdit} disabled={saving}>
              {t('fb.edit_cancel')}
            </button>
            <button type="button" className="btn btn-primary btn-sm" onClick={() => save()} disabled={saving || !dirty} title={t('fb.save_hint')}>
              <Save size={15} aria-hidden="true" />
              {t(saving ? 'fb.saving' : 'fb.save')}
            </button>
          </>
        ) : (
          <>
            {ask && sel && (
              <button type="button" className="btn btn-plain btn-sm fb-ask" onClick={askAi} title={t('fb.ask_ai_hint')}>
                <Sparkles size={15} aria-hidden="true" />
                {t('fb.ask_ai')}
              </button>
            )}
            {canFollow && (
              <button type="button" className={`btn btn-plain btn-icon ${follow ? 'on' : ''}`} onClick={() => setFollow(!follow)} aria-pressed={follow} aria-label={t('fb.follow')} title={t('fb.follow_hint')}>
                <ScrollText size={16} />
              </button>
            )}
            {canEdit && (
              <button type="button" className="btn btn-plain btn-icon" onClick={startEdit} aria-label={t('fb.edit')} title={t('fb.edit')}>
                <FilePen size={16} />
              </button>
            )}
            {onInsert && (
              <button type="button" className="btn btn-plain btn-icon" onClick={() => onInsert(entry.path)} aria-label={insertText} title={insertText}>
                <TextCursorInput size={16} />
              </button>
            )}
            {extra}
            {canShare && (
              <button type="button" className="btn btn-plain btn-icon" onClick={share} aria-label={t('fb.share')} title={t('fb.share')}>
                <Share2 size={16} />
              </button>
            )}
            <DownloadLink ep={ep} entry={entry} size={16} />
          </>
        )}
        <button type="button" className="btn btn-plain btn-icon" onClick={close} aria-label={t('fb.close_preview')} title={t('fb.close_preview')}>
          <X size={17} />
        </button>
      </div>
      <div className="fb-preview-body">
        {state.loading ? (
          <p className="muted small fb-pad">{t('fb.loading')}</p>
        ) : state.error ? (
          <p className="fb-pad small" style={{ color: 'var(--danger)' }}>{t(`files.err.${state.error.code}`, { defaultValue: errorText(t, state.error) })}</p>
        ) : state.media ? (
          <Suspense fallback={<p className="muted small fb-pad">{t('fb.loading')}</p>}>
            {active ? <Player key={state.media} src={state.media} name={entry.name} audio={audio} onError={() => setState({ office: 'media_failed' })} /> : <p className="muted small fb-pad">{t('fb.media_paused')}</p>}
          </Suspense>
        ) : image ? (
          <>
            <button type="button" className="fb-img" onClick={() => setZoom(true)} aria-label={t('fb.zoom')}>
              <img src={state.url} alt={entry.name} />
            </button>
            {zoom && <ImageViewer images={[{ url: state.url, name: entry.name }]} onClose={() => setZoom(false)} />}
          </>
        ) : state.office ? (
          <p className="muted small fb-pad">{t(`fb.office_${state.office}`, { size: formatBytes(OFFICE_MAX) })}</p>
        ) : state.view?.url ? (
          <iframe className="fb-pdf" src={state.view.url} title={entry.name} />
        ) : state.view?.html === '' ? (
          <p className="muted small fb-pad">{t('fb.empty_file')}</p>
        ) : state.view?.html != null ? (
          // eslint-disable-next-line react/no-danger -- 已用 DOMPurify 清洗（OfficePreview.jsx）
          <div className="md" dangerouslySetInnerHTML={{ __html: state.view.html }} />
        ) : state.view?.sheets ? (
          <SheetView sheets={state.view.sheets} />
        ) : state.text == null ? (
          <p className="muted small fb-pad">{t('fb.binary')}</p>
        ) : (
          <>
            {state.truncated && <p className="muted small fb-pad fb-note">{t('fb.truncated', { size: formatBytes(PREVIEW_MAX) })}</p>}
            {showSheet ? (
              <SheetView sheets={csvSheets} />
            ) : showMd ? (
              <Markdown text={state.text} imageUrl={ep.plain ? (src) => `/api${ep.preview(resolve(dir, src, sep))}` : null} />
            ) : !state.text && !editing ? (
              <pre className="fb-text">{t('fb.empty_file')}</pre>
            ) : (
              <Suspense fallback={<pre className={`fb-text ${wrap ? 'fb-wrap' : ''}`}>{state.text}</pre>}>
                <CodeView
                  text={editing ? editBase : state.text}
                  name={entry.name}
                  wrap={wrap}
                  focus={follow || editing ? null : focus}
                  editable={editing}
                  atEnd={follow}
                  onChange={(v) => {
                    draft.current = v;
                    if (!dirty) setDirty(true);
                  }}
                  onSave={() => save()}
                  onSelect={ask ? setSel : undefined}
                />
              </Suspense>
            )}
          </>
        )}
      </div>
    </div>
  );
}

/** Markdown 里的相对图片路径 → 这台电脑上的绝对路径。 */
function resolve(dir, rel, sep) {
  const parts = dir.split(/[\\/]/);
  for (const seg of rel.split(/[\\/]/)) {
    if (seg === '..') parts.length > 1 && parts.pop();
    else if (seg && seg !== '.') parts.push(seg);
  }
  const out = parts.join(sep);
  return out || sep;
}

/** 新建文件夹 / 重命名：一个输入框。 */
function NameDialog({ dialog, onClose }) {
  const { t } = useTranslation();
  const [name, setName] = useState(dialog?.initial ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  const input = useRef(null);
  useEffect(() => {
    setName(dialog?.initial ?? '');
    setError(null);
    // 重命名时只选中文件名、不选扩展名
    setTimeout(() => {
      const el = input.current;
      if (!el) return;
      el.focus();
      const dot = dialog?.isDir ? -1 : (dialog?.initial ?? '').lastIndexOf('.');
      el.setSelectionRange(0, dot > 0 ? dot : el.value.length);
    }, 0);
  }, [dialog]);
  if (!dialog) return null;
  const submit = async (e) => {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await dialog.run(name.trim());
      onClose();
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal open onClose={onClose} title={dialog.title}>
      <form onSubmit={submit}>
        <Field label={t('fb.name')}>
          <input ref={input} value={name} onChange={(e) => setName(e.target.value)} maxLength={255} autoComplete="off" spellCheck={false} />
        </Field>
        <ErrorNote error={error} />
        <div className="modal-actions">
          <Button type="button" variant="ghost" onClick={onClose}>
            {t('action.cancel')}
          </Button>
          <Button type="submit" busy={busy} disabled={!name.trim() || name.trim() === dialog.initial}>
            {dialog.confirm}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

// ---------------- 列表：排序、类型图标、时间（docs/CHAT_UX_2026-11-06.md §6） ----------------

const SORT_KEY = 'll-fb-sort';
const WIDTH_KEY = 'll-fb-preview-w';
const WEEK_MS = 7 * 86_400_000;
/** 一次最多下载几个文件、一共多大（多了浏览器会卡、也容易被当成恶意连续下载） */
export const BATCH_FILES = 20;
export const BATCH_BYTES = 2 * 1024 ** 3;
const BATCH_GAP_MS = 400;

const readStore = (k, d) => {
  try {
    const v = JSON.parse(localStorage.getItem(k) ?? 'null');
    return v ?? d;
  } catch {
    return d;
  }
};
const writeStore = (k, v) => {
  try {
    localStorage.setItem(k, JSON.stringify(v));
  } catch {
    /* 隐私模式等：不记 */
  }
};

const KINDS = [
  ['image', /\.(png|jpe?g|gif|webp|bmp|ico|avif|svg|heic|tiff?)$/i, ImageIcon],
  ['md', /\.(md|markdown|mdx)$/i, BookOpen],
  ['sheet', /\.(xlsx?|csv|tsv|ods|numbers)$/i, FileSpreadsheet],
  ['doc', /\.(pdf|docx?|odt|rtf|pptx?|key|pages)$/i, FileText],
  ['text', /\.(txt|log|out)$/i, FileText],
  ['archive', /\.(zip|tar|gz|tgz|bz2|xz|7z|rar|zst|dmg|iso|deb|rpm|apk)$/i, FileArchive],
  ['video', /\.(mp4|mov|mkv|webm|avi|m4v)$/i, FileVideo],
  ['audio', /\.(mp3|wav|flac|m4a|ogg|aac|opus)$/i, FileAudio],
  ['code', /\.(js|jsx|ts|tsx|mjs|cjs|rs|go|py|rb|java|kt|swift|c|cc|cpp|h|hpp|cs|php|sh|bash|zsh|ps1|lua|sql|html?|css|scss|vue|svelte|json|ya?ml|toml|ini|xml|lock|env|dockerfile|makefile)$/i, FileCode],
];
/** 文件类型：图标和颜色按它来 */
export function fileKind(name, isDir) {
  if (isDir) return { kind: 'dir', Icon: Folder };
  const hit = KINDS.find(([, re]) => re.test(name) || re.test(`.${name}`));
  return hit ? { kind: hit[0], Icon: hit[2] } : { kind: 'file', Icon: File };
}

/** 排序：文件夹始终在前；名字按自然顺序（a2 在 a10 前面） */
export function sortEntries(entries, { key, dir }) {
  const coll = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  const val = (e) => (key === 'mtime' ? e.mtime_ms ?? 0 : key === 'size' ? (e.is_dir ? -1 : e.size ?? 0) : 0);
  return [...entries].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    const d = key === 'name' ? coll.compare(a.name, b.name) : val(a) - val(b) || coll.compare(a.name, b.name);
    return d * dir;
  });
}

const pad = (n) => String(n).padStart(2, '0');
/** 修改时间：一周内相对时间，更早写日期（今年的不写年份） */
function fileTime(t, ms, now) {
  if (!ms) return '';
  if (now - ms < WEEK_MS && now >= ms) return relativeTime(t, new Date(ms).toISOString(), now);
  const d = new Date(ms);
  const day = `${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  return d.getFullYear() === new Date(now).getFullYear() ? `${day} ${pad(d.getHours())}:${pad(d.getMinutes())}` : `${d.getFullYear()}-${day}`;
}
const fullTime = (ms) => (ms ? new Date(ms).toLocaleString() : '');

/** 下载一个文件：能用普通链接时直接点；远程（WebRTC）经 iframe；多台电脑时取回再存 */
async function saveOne(ep, entry) {
  const href = `/api${ep.download(entry.path)}`;
  if (ep.plain) {
    const a = Object.assign(document.createElement('a'), { href, download: entry.name });
    document.body.appendChild(a);
    a.click();
    a.remove();
    return;
  }
  if (rtcMode) {
    saveViaFrame(href, selectedDevice());
    return;
  }
  const blob = await (await fetchRaw(ep.download(entry.path))).blob();
  const url = URL.createObjectURL(blob);
  const a = Object.assign(document.createElement('a'), { href: url, download: entry.name });
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30000);
}

/** 一次下载好几个文件：先查上限，开始时提示，远程取回时报进度 */
function useBatchDownload(ep) {
  const { t } = useTranslation();
  const toast = useToast();
  const [busy, setBusy] = useState(null); // { n, total, name }
  const run = async (entries) => {
    const files = entries.filter((e) => !e.is_dir);
    const dirs = entries.length - files.length;
    if (!files.length) {
      toast(t('fb.dl_no_dirs'), 'error');
      return;
    }
    const bytes = files.reduce((n, e) => n + (e.size ?? 0), 0);
    if (files.length > BATCH_FILES) {
      toast(t('fb.dl_too_many', { n: BATCH_FILES, count: files.length }), 'error');
      return;
    }
    if (bytes > BATCH_BYTES) {
      toast(t('fb.dl_too_big', { size: formatBytes(BATCH_BYTES), total: formatBytes(bytes) }), 'error');
      return;
    }
    if (files.length === 1) toast(t('fb.dl_start_one', { name: files[0].name }));
    else toast(t('fb.dl_start', { n: files.length, size: formatBytes(bytes) }) + (dirs ? ` ${t('fb.dl_skip_dirs', { n: dirs })}` : '') + ` ${t('fb.dl_allow_hint')}`);
    let failed = 0;
    for (let i = 0; i < files.length; i += 1) {
      setBusy({ n: i + 1, total: files.length, name: files[i].name });
      try {
        // eslint-disable-next-line no-await-in-loop -- 一个一个来，浏览器才不会把后面的吞掉
        await saveOne(ep, files[i]);
      } catch {
        failed += 1;
      }
      // eslint-disable-next-line no-await-in-loop
      if (i < files.length - 1) await new Promise((r) => setTimeout(r, BATCH_GAP_MS));
    }
    setBusy(null);
    if (failed) toast(t('fb.dl_failed', { n: failed }), 'error');
  };
  return { run, busy };
}

// ---------------- 打包下载（文件夹、多个文件：电脑端打成一个 zip，/api/fs/zip） ----------------

function saveZip(paths, device) {
  const href = `/api/fs/zip?p=${encodeURIComponent(JSON.stringify(paths))}`;
  if (plainLinksWork()) {
    const a = Object.assign(document.createElement('a'), { href, download: '' });
    document.body.appendChild(a);
    a.click();
    a.remove();
    return Promise.resolve();
  }
  if (rtcMode) {
    saveViaFrame(href, device);
    return Promise.resolve();
  }
  return fetchRaw(href.slice(4), { device }).then(async (res) => {
    const name = /filename\*=UTF-8''([^;]+)/.exec(res.headers.get('content-disposition') ?? '')?.[1];
    const url = URL.createObjectURL(await res.blob());
    const a = Object.assign(document.createElement('a'), { href: url, download: name ? decodeURIComponent(name) : 'files.zip' });
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 30000);
  });
}

// ---------------- 剪贴板：移动 / 复制（选好后去要放的文件夹点“放到这里”；电脑上也是 ⌘X / ⌘C / ⌘V） ----------------
// 跨文件标签、跨“文件”页都有效（同一台电脑），切到别的文件夹也还在，直到放好或取消。

let clip = null; // { op: 'move' | 'copy', items: [{ path, name, is_dir }], device }
const clipSubs = new Set();
const setClip = (v) => {
  clip = v;
  clipSubs.forEach((f) => f());
};
const subscribeClip = (f) => {
  clipSubs.add(f);
  return () => clipSubs.delete(f);
};
const useClip = () => useSyncExternalStore(subscribeClip, () => clip);

// ---------------- 收藏的文件夹（每台电脑一份，存在这个浏览器里） ----------------

const FAVS_KEY = 'll-fb-favs';
const favSubs = new Set();
let favs = readStore(FAVS_KEY, {});
const subscribeFavs = (f) => {
  favSubs.add(f);
  return () => favSubs.delete(f);
};
function useFavs(device) {
  const all = useSyncExternalStore(subscribeFavs, () => favs);
  const list = all[device ?? ''] ?? [];
  const toggle = (path) => {
    const has = list.includes(path);
    favs = { ...favs, [device ?? '']: has ? list.filter((x) => x !== path) : [path, ...list].slice(0, 30) };
    writeStore(FAVS_KEY, favs);
    favSubs.forEach((f) => f());
    return !has;
  };
  return { list, has: (p) => list.includes(p), toggle };
}

const DAY_MS = 24 * 3600_000;
const baseName = (p) => p.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || p;
const relDir = (path, root) => {
  const dir = parentDir(path) ?? '';
  if (!root || !dir.startsWith(root)) return dir;
  return dir.slice(root.length).replace(/^[\\/]/, '');
};

/** 在几个文件夹里按名字找（合并、名字开头的在前），给快速访问和“含子文件夹”用。 */
function useDeepSearch(ops, roots, q, hidden, on) {
  const [res, setRes] = useState(null); // { q, items, truncated, loading }
  const key = roots.join('\n');
  useEffect(() => {
    const words = q.trim();
    if (!on || !words || !roots.length) {
      setRes(null);
      return undefined;
    }
    let alive = true;
    setRes((r) => ({ ...(r ?? { items: [] }), q: words, loading: true }));
    const timer = setTimeout(() => {
      Promise.all(roots.slice(0, 6).map((root) => ops.get(`/fs/search?root=${encodeURIComponent(root)}&q=${encodeURIComponent(words)}&hidden=${hidden ? 1 : 0}`).then((r) => ({ ...r, root }), (e) => ({ items: [], error: e, root }))))
        .then((all) => {
          if (!alive) return;
          const last = words.toLowerCase().split(/\s+/).pop();
          const seen = new Set();
          const items = all
            .flatMap((r) => r.items.map((h) => ({ ...h, root: r.root })))
            .filter((h) => !seen.has(h.path) && seen.add(h.path))
            .sort((a, b) => Number(!a.name.toLowerCase().startsWith(last)) - Number(!b.name.toLowerCase().startsWith(last)) || a.path.length - b.path.length)
            .slice(0, 200);
          const error = all.every((r) => r.error) ? all[0].error : null;
          setRes({ q: words, items, truncated: all.some((r) => r.truncated), loading: false, error });
        });
    }, 280);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [on, q, hidden, key]);
  return res;
}

/** 找到的、最近改动的文件：一行（图标、名字、在哪、时间）。 */
function HitRow({ hit, onPick, now, sub }) {
  const { t } = useTranslation();
  const { kind, Icon } = fileKind(hit.name, hit.is_dir);
  return (
    <button type="button" className="fb-hit" onClick={() => onPick(hit)} title={hit.path}>
      <Icon size={18} aria-hidden="true" className={`fb-ico fb-ico-${kind}`} />
      <span className="fb-hit-text">
        <span className="fb-hit-name">{hit.name}</span>
        <span className="fb-hit-where muted mono">{sub ?? relDir(hit.path, hit.root)}</span>
      </span>
      {hit.mtime_ms ? <span className="muted small fb-hit-time">{fileTime(t, hit.mtime_ms, now)}</span> : null}
    </button>
  );
}

/**
 * 快速访问（“文件”页的首屏，§9.3）：最近改动、项目、收藏、常用位置；顶上的搜索框在项目里按名字找。
 * `onGo(dir)` 进文件夹，`onOpen(path)` 去文件所在的文件夹并打开它。
 */
function QuickAccess({ device, ops, onGo, onOpen }) {
  const { t } = useTranslation();
  const s = useSessions();
  const fav = useFavs(device);
  const [home, setHome] = useState(null);
  const [recent, setRecent] = useState(null);
  const [q, setQ] = useState('');
  const [allRecent, setAllRecent] = useState(false);
  const projects = useMemo(() => {
    const seen = new Set();
    return (s?.projects ?? [])
      .filter((p) => (!s?.multi || (p.device ?? null) === (device ?? null)) && !p.container && !p.offline && p.workdir && !seen.has(p.workdir) && seen.add(p.workdir))
      .slice(0, 12);
  }, [s?.projects, s?.multi, device]);
  const roots = projects.map((p) => p.workdir);
  useEffect(() => {
    let alive = true;
    ops.get('/fs/list?path=').then((d) => alive && setHome(d), () => {});
    return () => {
      alive = false;
    };
  }, [device]);
  useEffect(() => {
    if (!roots.length) {
      setRecent({ items: [] });
      return undefined;
    }
    let alive = true;
    ops.post('/fs/recent', { roots, since_ms: Date.now() - DAY_MS }).then(
      (r) => alive && setRecent(r),
      (error) => alive && setRecent({ items: [], error }),
    );
    return () => {
      alive = false;
    };
  }, [device, roots.join('\n')]);
  const found = useDeepSearch(ops, roots.length ? roots : home?.home ? [home.home] : [], q, false, true);
  const now = Date.now();
  const sep = home?.os === 'windows' ? '\\' : '/';
  const places = home?.home
    ? [
        { name: t('fb.qa_home'), path: home.home, Icon: Home },
        { name: t('fb.qa_desktop'), path: join(home.home, 'Desktop', sep) },
        { name: t('fb.qa_downloads'), path: join(home.home, 'Downloads', sep) },
        { name: t('fb.qa_documents'), path: join(home.home, 'Documents', sep) },
        ...(home.roots ?? []).map((r) => ({ name: r, path: r })),
      ]
    : [];
  const pick = (h) => (h.is_dir ? onGo(h.path) : onOpen(h.path));
  const items = recent?.items ?? [];
  const shown = allRecent ? items : items.slice(0, 8);
  const project = (root) => projects.find((p) => p.workdir === root)?.name ?? baseName(root ?? '');
  return (
    <div className="fb-qa">
      <label className="fb-qa-search">
        <Search size={16} aria-hidden="true" className="muted" />
        <input type="search" value={q} onChange={(e) => setQ(e.target.value)} placeholder={t(roots.length ? 'fb.qa_search_ph' : 'fb.qa_search_home_ph')} aria-label={t('fb.qa_search')} spellCheck={false} />
      </label>
      {q.trim() ? (
        <section className="fb-qa-sec">
          {found?.loading && !found.items.length && <p className="muted small fb-pad">{t('fb.deep_busy')}</p>}
          {found?.error && <p className="small fb-pad" style={{ color: 'var(--danger)' }}>{errorText(t, found.error)}</p>}
          {found && !found.loading && !found.error && !found.items.length && <p className="muted small fb-pad">{t('fb.search_none', { q })}</p>}
          {found?.items.map((h) => <HitRow key={h.path} hit={h} onPick={pick} now={now} sub={`${project(h.root)} · ${relDir(h.path, h.root) || '.'}`} />)}
          {found?.truncated && <p className="muted small fb-pad">{t('fb.deep_more')}</p>}
        </section>
      ) : (
        <>
          <section className="fb-qa-sec">
            <h3>
              <Clock size={14} aria-hidden="true" />
              {t('fb.qa_recent')}
            </h3>
            {!recent && <p className="muted small fb-pad">{t('fb.deep_busy')}</p>}
            {recent && !items.length && <p className="muted small fb-pad">{t(roots.length ? 'fb.qa_recent_none' : 'fb.qa_recent_noproj')}</p>}
            {shown.map((h) => <HitRow key={h.path} hit={h} onPick={pick} now={now} sub={`${project(h.root)} · ${relDir(h.path, h.root) || '.'}`} />)}
            {items.length > shown.length && (
              <button type="button" className="btn btn-plain fb-qa-more" onClick={() => setAllRecent(true)}>
                {t('fb.qa_more', { n: items.length - shown.length })}
              </button>
            )}
          </section>
          {projects.length > 0 && (
            <section className="fb-qa-sec">
              <h3>
                <Zap size={14} aria-hidden="true" />
                {t('fb.qa_projects')}
              </h3>
              <div className="fb-qa-grid">
                {projects.map((p) => (
                  <button key={p.key ?? p.workdir} type="button" className="fb-qa-place" onClick={() => onGo(p.workdir)} title={p.workdir}>
                    <Folder size={17} aria-hidden="true" className="fb-ico fb-ico-dir" />
                    <span className="ellipsis">{p.name || baseName(p.workdir)}</span>
                  </button>
                ))}
              </div>
            </section>
          )}
          {fav.list.length > 0 && (
            <section className="fb-qa-sec">
              <h3>
                <Star size={14} aria-hidden="true" />
                {t('fb.qa_favs')}
              </h3>
              <div className="fb-qa-grid">
                {fav.list.map((p) => (
                  <button key={p} type="button" className="fb-qa-place" onClick={() => onGo(p)} title={p}>
                    <Star size={16} aria-hidden="true" className="fb-ico fb-fav-on" />
                    <span className="ellipsis">{baseName(p)}</span>
                  </button>
                ))}
              </div>
            </section>
          )}
          <section className="fb-qa-sec">
            <h3>
              <Home size={14} aria-hidden="true" />
              {t('fb.qa_places')}
            </h3>
            <div className="fb-qa-grid">
              {places.map(({ name, path, Icon = Folder }) => (
                <button key={path} type="button" className="fb-qa-place" onClick={() => onGo(path)} title={path}>
                  <Icon size={17} aria-hidden="true" className="fb-ico fb-ico-dir" />
                  <span className="ellipsis">{name}</span>
                </button>
              ))}
            </div>
          </section>
        </>
      )}
    </div>
  );
}

/** “发给对话”（“文件”页）：选一个这台电脑上开着的对话会话，把路径填进它的输入框（不发送）。 */
function ChatPick({ device, text, onClose, openSession }) {
  const { t } = useTranslation();
  const s = useSessions();
  const list = (s?.items ?? [])
    .filter((i) => i.running && sessionMode(i) === 'chat' && (!s.multi || (terminalDevice(i.id) ?? null) === (device ?? null)))
    .slice(0, 30);
  return (
    <Modal open onClose={onClose} title={t('fb.to_chat_title')}>
      {list.length === 0 && <p className="muted">{t('fb.to_chat_none')}</p>}
      <div className="fb-chatpick">
        {list.map((i) => (
          <button
            key={i.id}
            type="button"
            className="fb-hit"
            onClick={() => {
              onClose();
              sendToChat(i.id, text);
              openSession(i.id);
            }}
          >
            <MessagesSquare size={17} aria-hidden="true" className="muted" />
            <span className="fb-hit-text">
              <span className="fb-hit-name">{sessionTitle(i) || i.id}</span>
              <span className="fb-hit-where muted mono">{i.workdir}</span>
            </span>
          </button>
        ))}
      </div>
    </Modal>
  );
}

/** 手机上长按一行进入多选（不震动，和终端的长按一样）；之后点一下就是勾选 / 取消。 */
function useLongPress(onLong) {
  const timer = useRef(0);
  const start = useRef(null);
  const fired = useRef(false);
  const cancel = () => {
    clearTimeout(timer.current);
    start.current = null;
  };
  return {
    fired,
    bind: (arg) => ({
      onPointerDown: (e) => {
        if (e.pointerType !== 'touch') return;
        fired.current = false;
        start.current = { x: e.clientX, y: e.clientY };
        clearTimeout(timer.current);
        timer.current = setTimeout(() => {
          fired.current = true;
          start.current = null;
          onLong(arg);
        }, 480);
      },
      onPointerMove: (e) => {
        if (start.current && Math.hypot(e.clientX - start.current.x, e.clientY - start.current.y) > 10) cancel();
      },
      onPointerUp: cancel,
      onPointerCancel: cancel,
      onContextMenu: (e) => {
        if (e.nativeEvent?.pointerType === 'touch' || fired.current || start.current) e.preventDefault();
      },
    }),
  };
}

// ---------------- 记住看到哪（每个会话的文件标签、每台电脑的“文件”页各记一份） ----------------

const VIEWS_KEY = 'll-fb-views';
const VIEWS_MAX = 60;
const views = new Map(Object.entries(readStore(VIEWS_KEY, {})));
/** 文件夹、打开的文件、隐藏文件开关；切标签、切会话、刷新页面后接着看 */
export function readView(key) {
  return views.get(key) ?? null;
}
function writeView(key, v) {
  views.delete(key);
  views.set(key, v);
  // 只留最近的几十个（会话删了也不会一直攒着）
  while (views.size > VIEWS_MAX) views.delete(views.keys().next().value);
  writeStore(VIEWS_KEY, Object.fromEntries(views));
}

// ---------------- Git 改动标记（会话的文件标签：这个仓库里改过的文件、里面有改动的文件夹） ----------------

const GIT_TTL_MS = 10_000;
/** `{ root, files }` → 绝对路径 → 标记（M 改了、A 新加、U 没跟踪、R 改名、! 冲突）；有改动的上级文件夹记 `dir` */
export function gitMarks(st, sep) {
  const out = new Map();
  if (!st?.repo || !st.root) return out;
  for (const f of st.files ?? []) {
    const code = f.conflict ? '!' : f.worktree === '?' ? 'U' : f.worktree !== '.' ? f.worktree : f.index;
    if (code === 'D') continue;
    const rel = sep === '\\' ? f.path.replaceAll('/', '\\') : f.path;
    const abs = join(st.root, rel, sep);
    out.set(abs, code === 'C' ? 'A' : code === 'T' ? 'M' : code);
    for (let d = parentDir(abs); d && d.length > st.root.length && !out.has(d); d = parentDir(d)) out.set(d, 'dir');
  }
  return out;
}

function useGitMarks(id, ops, dir, sep, tick, active) {
  const cache = useRef(null); // { root, st, at }
  const [marks, setMarks] = useState(() => new Map());
  useEffect(() => {
    if (!id || !dir || !active) return undefined;
    const c = cache.current;
    const inside = c?.st?.root && (dir === c.st.root || dir.startsWith(c.st.root + sep));
    if (inside && Date.now() - c.at < GIT_TTL_MS && c.tick === tick) {
      setMarks(gitMarks(c.st, sep));
      return undefined;
    }
    let alive = true;
    ops
      .get(`/instances/${id}/git?path=${encodeURIComponent(dir)}`)
      .then((st) => {
        if (!alive) return;
        cache.current = { st, at: Date.now(), tick };
        setMarks(gitMarks(st, sep));
      })
      .catch(() => alive && setMarks(new Map()));
    return () => {
      alive = false;
    };
  }, [id, dir, tick, active]);
  return marks;
}

/** 可以拖的分隔条：调预览的宽度（记住） */
function Splitter({ box, width, onWidth }) {
  const { t } = useTranslation();
  const start = (e) => {
    e.preventDefault();
    const el = box.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const move = (ev) => {
      const w = Math.round(rect.right - ev.clientX);
      onWidth(Math.min(Math.max(280, w), Math.max(320, rect.width - 260)));
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.style.cursor = '';
    };
    document.body.style.cursor = 'col-resize';
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };
  return (
    <div
      className="fb-split"
      role="separator"
      aria-orientation="vertical"
      aria-label={t('fb.resize')}
      aria-valuenow={width}
      tabIndex={0}
      onPointerDown={start}
      onKeyDown={(e) => {
        if (e.key === 'ArrowLeft') onWidth(width + 24);
        else if (e.key === 'ArrowRight') onWidth(Math.max(280, width - 24));
      }}
    />
  );
}

/**
 * `id` 终端（不传 = 管理台“文件”页，浏览选中的那台电脑）；`active` 是否可见（不可见时不刷新）；
 * `onInsert(text)` 把路径填进终端或对话输入框（没有时不显示这个按钮），`insertTo`：'chat' | 'term'（按钮上怎么说）；
 * `reveal`（{ path, n }）：打开这个文件所在的文件夹并预览它（对话里点开的文件“在文件中显示”）；
 * `onNewHere(dir)`：在这个文件夹里开新会话（没有时不显示）。
 */
/**
 * `path` / `onPath`：“文件”页把文件夹放在地址里（`?path=`，每进一个文件夹一条历史，返回键逐级退回）；
 * 这时 `path` 为空显示快速访问。`openSession`：“文件”页的“发给对话”打开会话用。
 */
export default function FileBrowser({ id, active = true, onInsert, insertTo = 'term', reveal = null, onNewHere, path: routePath, onPath, openSession }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const device = id ? terminalDevice(id) : selectedDevice();
  const viewKey = id ? `i:${id}` : `fs:${device ?? ''}`;
  const saved = useMemo(() => readView(viewKey), [viewKey]);
  const routed = typeof onPath === 'function';
  const [ownPath, setOwnPath] = useState(saved?.path ?? ''); // 空 = 终端的当前目录
  const path = routed ? routePath ?? '' : ownPath;
  const setPath = routed ? onPath : setOwnPath;
  const quick = routed && !path; // “文件”页没指定文件夹：快速访问
  const [hidden, setHidden] = useState(Boolean(saved?.hidden));
  const [tick, setTick] = useState(0);
  const [state, setState] = useState({ loading: true });
  const [open, setOpen] = useState(saved?.open ?? null);
  const [pending, setPending] = useState(null); // 要打开的文件（“在文件中显示”：等列表到了再开）
  const [dialog, setDialog] = useState(null);
  const [dropping, setDropping] = useState(false);
  const [sort, setSortState] = useState(() => readStore(SORT_KEY, { key: 'name', dir: 1 }));
  const [filter, setFilter] = useState('');
  const [searching, setSearching] = useState(false);
  const [checked, setChecked] = useState(() => new Set());
  const [cursor, setCursor] = useState(-1);
  const [previewW, setPreviewW] = useState(() => readStore(WIDTH_KEY, 460));
  const [deep, setDeep] = useState(false); // 搜索含子文件夹
  const [chatPick, setChatPick] = useState(null); // 发给对话：要填的文字
  const conn = useConn();
  const offline = connManaged && !isOnline(conn);
  const clipNow = useClip();
  const fav = useFavs(device);
  const fileInput = useRef(null);
  const listRef = useRef(null);
  const mainRef = useRef(null);
  const searchRef = useRef(null);
  const ep = endpoints(id);
  const ops = deviceApi(device ?? undefined);
  const store = dirUploadStore(device);
  const tasks = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const batch = useBatchDownload(ep);
  const reload = () => setTick((n) => n + 1);
  useEffect(() => {
    if (!active || quick) return undefined;
    let alive = true;
    setState((s) => ({ ...s, loading: true }));
    ops
      .get(ep.list(path, hidden))
      .then((data) => alive && setState({ data }))
      .catch((error) => alive && setState((s) => ({ data: s.data, error })));
    return () => {
      alive = false;
    };
  }, [id, device, path, hidden, tick, active, quick]);
  const data = quick ? null : state.data;
  const sep = data?.os === 'windows' ? '\\' : '/';
  const marks = useGitMarks(id, ops, data?.path, sep, tick, active);
  // 记住看到哪：真正到了的文件夹（不是“空 = 当前目录”），打开的文件
  useEffect(() => {
    if (!data?.path) return;
    const o = open && { name: open.name, path: open.path, size: open.size, mtime_ms: open.mtime_ms };
    writeView(viewKey, { path: data.path, hidden, open: o });
  }, [viewKey, data?.path, hidden, open?.path]);
  // “在文件中显示”：去它所在的文件夹，列表到了再打开它（是隐藏文件就把隐藏文件打开）
  const openAt = (file) => {
    const dir = parentDir(file);
    if (!dir) return;
    const name = file.slice(dir.length).replace(/^[\\/]/, '');
    if (name.startsWith('.')) setHidden(true);
    setPending(file);
    setOpen(null);
    setFilter('');
    setSearching(false);
    if (data?.path === dir) reload();
    else setPath(dir);
  };
  useEffect(() => {
    if (reveal?.path) openAt(reveal.path);
  }, [reveal?.n]);
  useEffect(() => {
    if (!pending || !data?.entries || state.loading) return;
    if (data.path !== parentDir(pending)) return;
    setPending(null);
    const name = pending.slice(data.path.length).replace(/^[\\/]/, '');
    const e = data.entries.find((x) => x.name === name);
    if (!e) {
      toast(t('fb.reveal_gone', { name }), 'error');
      return;
    }
    setOpen({ ...e, path: pending });
    requestAnimationFrame(() => listRef.current?.querySelector(`[data-name="${CSS.escape(name)}"]`)?.scrollIntoView({ block: 'center' }));
  }, [pending, data, state.loading]);
  // 换了文件夹：清掉勾选、筛选、光标；回到上一级时光标落在刚才那个文件夹上（§9.5）
  const lastDir = useRef(null);
  useEffect(() => {
    setChecked(new Set());
    setFilter('');
    setDeep(false);
    const from = lastDir.current;
    lastDir.current = data?.path ?? null;
    if (from && data?.path && parentDir(from) === data.path) {
      const name = from.slice(data.path.length).replace(/^[\\/]/, '');
      requestAnimationFrame(() => {
        const row = listRef.current?.querySelector(`[data-name="${CSS.escape(name)}"]`);
        if (!row) return;
        setCursor(Number(row.dataset.idx));
        row.scrollIntoView({ block: 'center' });
        row.classList.add('fb-flash');
        setTimeout(() => row.classList.remove('fb-flash'), 1200);
      });
    } else setCursor(-1);
  }, [data?.path]);
  // 路径很长（手机上）时把面包屑滚到最右，露出当前文件夹
  const crumbsRef = useRef(null);
  useEffect(() => {
    const el = crumbsRef.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [data?.path]);
  const setSort = (key) => {
    const next = { key, dir: sort.key === key ? -sort.dir : key === 'name' ? 1 : -1 };
    setSortState(next);
    writeStore(SORT_KEY, next);
  };
  const setWidth = (w) => {
    setPreviewW(w);
    writeStore(WIDTH_KEY, w);
  };
  const entries = useMemo(() => {
    if (!data?.entries) return [];
    const q = filter.trim().toLowerCase();
    const list = q ? data.entries.filter((e) => e.name.toLowerCase().includes(q)) : data.entries;
    return sortEntries(list, sort).map((e) => ({ ...e, path: join(data.path, e.name, sep) }));
  }, [data, filter, sort, sep]);
  const go = (p) => {
    setOpen(null);
    setPath(p);
  };
  const found = useDeepSearch(ops, data?.path ? [data.path] : [], filter, hidden, deep && searching);
  // 手机：预览是整屏的、多选是一种模式，返回键先关它们（§7）
  useLayer(Boolean(open) && touchUi, () => setOpen(null));
  useLayer(checked.size > 0 && touchUi, () => setChecked(new Set()));
  const insert = onInsert && ((p) => onInsert(`${quotePath(p, data?.os)} `));
  const insertLabel = t(insertTo === 'chat' ? 'fb.insert_chat' : 'fb.insert');
  const copyPaths = (paths) =>
    navigator.clipboard?.writeText(paths.join('\n')).then(
      () => toast(t(paths.length > 1 ? 'fb.copied_n' : 'fb.copied', { n: paths.length })),
      () => toast(t('fb.copy_failed'), 'error'),
    );
  const failed = (e) => toast(errorText(t, e), 'error');

  // ---------- 文件管理 ----------
  const mkdir = () =>
    setDialog({
      title: t('fb.new_folder'),
      confirm: t('fb.create'),
      initial: '',
      run: async (name) => {
        await ops.post('/fs/mkdir', { path: data.path, name });
        reload();
      },
    });
  const rename = (entry) =>
    setDialog({
      title: t('fb.rename_title', { name: entry.name }),
      confirm: t('fb.rename'),
      initial: entry.name,
      isDir: entry.is_dir,
      run: async (name) => {
        const r = await ops.post('/fs/rename', { path: entry.path, name });
        if (open?.path === entry.path) setOpen(null);
        reload();
        return r;
      },
    });
  const names = (list) => list.slice(0, 5).map((e) => e.name).join('、') + (list.length > 5 ? t('fb.and_more', { n: list.length }) : '');
  /** 永久删除（回收站用不了时，或者电脑上的看看版本太旧）。 */
  const destroy = async (list) => {
    let done = 0;
    for (const entry of list) {
      try {
        // eslint-disable-next-line no-await-in-loop
        await ops.post('/fs/delete', { path: entry.path });
        done += 1;
        if (open?.path === entry.path) setOpen(null);
      } catch (e) {
        failed(e);
      }
    }
    if (done) toast(list.length === 1 ? t('fb.deleted', { name: list[0].name }) : t('fb.deleted_n', { n: done }));
  };
  /** 删除 = 放进回收站（电脑上保留 7 天），提示条上能撤销；放不进去的再问一次永久删除。 */
  const remove = async (list) => {
    if (!list.length || offline) return;
    const one = list.length === 1 ? list[0] : null;
    const ok = await confirm(
      one
        ? { title: t(one.is_dir ? 'fb.delete_dir_title' : 'fb.delete_title', { name: one.name }), body: t(one.is_dir ? 'fb.trash_dir_body' : 'fb.trash_body', { path: one.path }), confirm: t('fb.delete'), danger: true }
        : { title: t('fb.delete_n_title', { n: list.length }), body: t('fb.trash_n_body', { names: names(list) }), confirm: t('fb.delete'), danger: true },
    );
    if (!ok) return;
    let r;
    try {
      r = await ops.post('/fs/trash', { paths: list.map((e) => e.path) });
    } catch (e) {
      if (e.status !== 404) {
        failed(e);
        return;
      }
      r = { items: [], failed: list.map((x) => ({ path: x.path, code: 'TRASH_UNAVAILABLE' })) };
    }
    setChecked(new Set());
    if (open && r.items.some((x) => x.path === open.path)) setOpen(null);
    const ids = r.items.map((x) => x.id);
    if (ids.length) {
      const undo = () =>
        ops.post('/fs/restore', { ids }).then((res) => {
          const back = res.items.filter((x) => x.to).length;
          if (back) toast(t('fb.restored', { n: back }));
          if (back < ids.length) toast(errorText(t, { code: res.items.find((x) => x.code)?.code }), 'error');
          reload();
        }, failed);
      toast(one ? t('fb.trashed', { name: one.name }) : t('fb.trashed_n', { n: ids.length }), 'ok', { label: t('fb.undo'), onClick: undo });
    }
    const noBin = r.failed.filter((x) => x.code === 'TRASH_UNAVAILABLE');
    const other = r.failed.filter((x) => x.code !== 'TRASH_UNAVAILABLE');
    if (other.length) toast(`${baseName(other[0].path)}：${errorText(t, { code: other[0].code })}`, 'error');
    if (noBin.length) {
      const rest = list.filter((e) => noBin.some((x) => x.path === e.path));
      const sure = await confirm({ title: t('fb.perm_title', { n: rest.length }), body: t('fb.perm_body', { names: names(rest) }), confirm: t('fb.delete_forever'), danger: true });
      if (sure) await destroy(rest);
    }
    reload();
  };
  // 移动 / 复制：先放进剪贴板，到了要放的文件夹再“放到这里”
  const cut = (list, op) => {
    if (!list.length) return;
    setClip({ op, items: list.map(({ path: p, name, is_dir }) => ({ path: p, name, is_dir })), device: device ?? null });
    setChecked(new Set());
    toast(t(op === 'move' ? 'fb.clip_move' : 'fb.clip_copy', { n: list.length }));
  };
  const paste = async () => {
    const c = clip;
    if (!c || !canWrite) return;
    if ((c.device ?? null) !== (device ?? null)) {
      toast(t('fb.clip_other_device'), 'error');
      return;
    }
    try {
      const r = await ops.post(`/fs/${c.op}`, { paths: c.items.map((e) => e.path), dest: data.path });
      const bad = r.items.filter((x) => x.code);
      const good = r.items.length - bad.length;
      if (c.op === 'move') setClip(null);
      if (good) toast(t(c.op === 'move' ? 'fb.moved_n' : 'fb.copied_n_files', { n: good }));
      if (bad.length) toast(`${baseName(bad[0].path)}：${errorText(t, { code: bad[0].code })}`, 'error');
    } catch (e) {
      failed(e.status === 404 ? { code: 'CLIENT_TOO_OLD' } : e);
    }
    reload();
  };
  const download = (list) => {
    if (!list.length) return;
    if (list.length === 1 && !list[0].is_dir) {
      batch.run(list);
      return;
    }
    toast(t('fb.zip_start', { n: list.length }));
    saveZip(list.map((e) => e.path), device).catch(failed);
  };
  const toChat = (list) => {
    const text = `${list.map((e) => quotePath(e.path, data?.os)).join(' ')} `;
    if (onInsert) onInsert(text);
    else if (openSession) setChatPick(text);
  };
  const upload = (list) => {
    const files = Array.from(list ?? []);
    if (!files.length || !data?.path) return;
    const dir = data.path;
    store.add(files, {
      dest: dir,
      onDone: (fresh, all) => {
        const bad = all.filter((x) => x.state === 'error');
        if (bad.length) toast(t('files.failed', { name: bad[0].name, reason: t(`files.err.${bad[0].error?.code}`, { defaultValue: errorText(t, bad[0].error) }) }), 'error');
        if (fresh.length) toast(t('fb.uploaded', { n: fresh.length }));
        reload();
      },
    });
  };
  const busy = tasks.filter((x) => ACTIVE.includes(x.state));
  const pct = busy.length ? Math.floor((busy.reduce((n, x) => n + Math.min(x.sent, x.size), 0) / Math.max(1, busy.reduce((n, x) => n + x.size, 0))) * 100) : 0;
  const hasFiles = (e) => Array.from(e.dataTransfer?.types ?? []).includes('Files');
  const canWrite = Boolean(data?.path && data.exists !== false && !data.denied && !offline);

  // ---------- 勾选 ----------
  const picked = entries.filter((e) => checked.has(e.name));
  const pickedBytes = picked.reduce((n, e) => n + (e.is_dir ? 0 : e.size ?? 0), 0);
  const toggle = (name) =>
    setChecked((cur) => {
      const next = new Set(cur);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  const allOn = entries.length > 0 && picked.length === entries.length;
  const toggleAll = () => setChecked(allOn ? new Set() : new Set(entries.map((e) => e.name)));

  // ---------- 打开、上一个 / 下一个、键盘 ----------
  const activate = (e) => (e.is_dir ? go(e.path) : setOpen(e));
  const files = entries.filter((e) => !e.is_dir);
  const openIdx = open ? files.findIndex((e) => e.path === open.path) : -1;
  const step = (d) => {
    const next = files[openIdx + d];
    if (!next) return;
    setOpen(next);
    setCursor(entries.indexOf(next));
  };
  useEffect(() => {
    if (cursor < 0) return;
    listRef.current?.querySelector(`[data-idx="${cursor}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [cursor]);
  const onKey = (e) => {
    if (e.target.closest('input, textarea, [role="dialog"]')) return;
    const mod = e.metaKey || e.ctrlKey;
    const cur = entries[cursor];
    const sel = picked.length ? picked : cur ? [cur] : [];
    const k = e.key.toLowerCase();
    if (mod && !e.shiftKey && k === 'a') {
      e.preventDefault();
      setChecked(new Set(entries.map((x) => x.name)));
    } else if (mod && !e.shiftKey && (k === 'x' || k === 'c') && sel.length) {
      e.preventDefault();
      cut(sel, k === 'x' ? 'move' : 'copy');
    } else if (mod && !e.shiftKey && k === 'v' && clip) {
      e.preventDefault();
      paste();
    } else if (mod && e.shiftKey && k === 'c' && sel.length) {
      e.preventDefault();
      copyPaths(sel.map((x) => x.path));
    } else if (e.key === 'F2' && cur) {
      e.preventDefault();
      rename(cur);
    } else if ((e.key === 'Delete' || (mod && e.key === 'Backspace')) && sel.length) {
      e.preventDefault();
      remove(sel);
    } else if (e.altKey && e.key === 'ArrowUp' && data?.parent) {
      e.preventDefault();
      go(data.parent);
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const n = Math.min(entries.length - 1, Math.max(0, cursor + (e.key === 'ArrowDown' ? 1 : -1)));
      setCursor(n);
      // 开着预览时跟着换（像 Finder 的快速查看）
      if (open && entries[n] && !entries[n].is_dir) setOpen(entries[n]);
    } else if (e.key === 'Enter' && entries[cursor]) {
      e.preventDefault();
      activate(entries[cursor]);
    } else if (e.key === ' ' && entries[cursor]) {
      e.preventDefault();
      toggle(entries[cursor].name);
    } else if (e.key === 'Backspace' && !mod && data?.parent) {
      e.preventDefault();
      go(data.parent);
    } else if (e.key === '/' || ((e.ctrlKey || e.metaKey) && e.key === 'f')) {
      e.preventDefault();
      setSearching(true);
      requestAnimationFrame(() => searchRef.current?.focus());
    } else if (e.key === 'Escape') {
      if (checked.size) setChecked(new Set());
      else if (open) setOpen(null);
    }
  };

  const now = Date.now();
  const sortBtn = (key, label, cls) => (
    <button type="button" className={`fb-th ${cls}`} onClick={() => setSort(key)} aria-sort={sort.key === key ? (sort.dir > 0 ? 'ascending' : 'descending') : undefined}>
      <span>{label}</span>
      {sort.key === key ? sort.dir > 0 ? <ChevronUp size={13} aria-hidden="true" /> : <ChevronDown size={13} aria-hidden="true" /> : <ArrowUpDown size={12} aria-hidden="true" className="fb-th-idle" />}
    </button>
  );
  const prevNext = open && files.length > 1 && (
    <>
      <button type="button" className="btn btn-plain btn-icon" disabled={openIdx <= 0} onClick={() => step(-1)} aria-label={t('fb.prev')} title={t('fb.prev')}>
        <ChevronLeft size={17} />
      </button>
      <button type="button" className="btn btn-plain btn-icon" disabled={openIdx < 0 || openIdx >= files.length - 1} onClick={() => step(1)} aria-label={t('fb.next')} title={t('fb.next')}>
        <ChevronRight size={17} />
      </button>
    </>
  );
  const longPress = useLongPress((e) => toggle(e.name));
  if (quick) {
    return (
      <div className="fb fb-quick">
        <QuickAccess device={device} ops={ops} onGo={go} onOpen={openAt} />
      </div>
    );
  }
  const clipHere = clipNow && (clipNow.device ?? null) === (device ?? null) ? clipNow : null;
  const pickbar = checked.size > 0 && (
    <div className="fb-pickbar" role="toolbar" aria-label={t('fb.picked_actions')}>
      <button type="button" className="btn btn-plain btn-icon" onClick={() => setChecked(new Set())} aria-label={t('fb.pick_clear')} title={t('fb.pick_clear')}>
        <X size={16} />
      </button>
      <span className="small grow ellipsis">
        <b>{t('fb.picked', { n: picked.length })}</b>
        {pickedBytes > 0 && <span className="muted"> · {formatBytes(pickedBytes)}</span>}
      </span>
      <button type="button" className="btn btn-plain fb-act" onClick={() => download(picked)} disabled={!!batch.busy || offline} title={t('fb.dl_zip_hint')}>
        <Download size={16} aria-hidden="true" />
        <span className="fb-act-label">{t('fb.download')}</span>
      </button>
      {(insert || openSession) && (
        <button type="button" className="btn btn-plain fb-act" onClick={() => toChat(picked)} title={insert ? insertLabel : t('fb.to_chat')}>
          {insert ? <TextCursorInput size={16} aria-hidden="true" /> : <MessagesSquare size={16} aria-hidden="true" />}
          <span className="fb-act-label">{insert ? t(insertTo === 'chat' ? 'fb.insert_chat_short' : 'fb.insert_short') : t('fb.to_chat_short')}</span>
        </button>
      )}
      <button type="button" className="btn btn-plain fb-act" onClick={() => cut(picked, 'move')} disabled={offline} title={t('fb.move_hint')}>
        <FolderInput size={16} aria-hidden="true" />
        <span className="fb-act-label">{t('fb.move')}</span>
      </button>
      <button type="button" className="btn btn-plain fb-act" onClick={() => cut(picked, 'copy')} disabled={offline} title={t('fb.copy_hint')}>
        <CopyPlus size={16} aria-hidden="true" />
        <span className="fb-act-label">{t('fb.copy_to')}</span>
      </button>
      <button type="button" className="btn btn-plain fb-act" onClick={() => copyPaths(picked.map((e) => e.path))} title={t('fb.copy_paths')}>
        <Copy size={16} aria-hidden="true" />
        <span className="fb-act-label">{t('fb.copy_paths_short')}</span>
      </button>
      <button type="button" className="btn btn-plain fb-act fb-danger" onClick={() => remove(picked)} disabled={offline} title={t('fb.delete')}>
        <Trash2 size={16} aria-hidden="true" />
        <span className="fb-act-label">{t('fb.delete')}</span>
      </button>
    </div>
  );
  const deepOn = deep && searching && filter.trim();
  return (
    <div
      className={`fb ${open ? 'fb-has-preview' : ''} ${dropping ? 'fb-dropping' : ''} ${checked.size ? 'fb-picking' : ''} ${insert ? 'fb-with-insert' : ''}`}
      onDragOver={(e) => {
        if (!canWrite || !hasFiles(e)) return;
        e.preventDefault();
        setDropping(true);
      }}
      onDragLeave={(e) => !e.currentTarget.contains(e.relatedTarget) && setDropping(false)}
      onDrop={(e) => {
        if (!canWrite || !hasFiles(e)) return;
        e.preventDefault();
        setDropping(false);
        upload(e.dataTransfer.files);
      }}
    >
      <div className="fb-bar">
        {routed && (
          <button type="button" className="btn btn-plain btn-icon" onClick={() => go('')} aria-label={t('fb.qa')} title={t('fb.qa')}>
            <Zap size={17} />
          </button>
        )}
        <button type="button" className="btn btn-plain btn-icon" disabled={!data?.parent} onClick={() => go(data.parent)} aria-label={t('fb.up')} title={t('fb.up_hint')}>
          <ArrowUp size={17} />
        </button>
        <nav className="fb-crumbs" ref={crumbsRef} aria-label={t('fb.location')}>
          {crumbs(data?.path, sep).map((c, i, all) => (
            <span key={c.path} className="fb-crumb">
              {i > 0 && c.name !== '/' && all[i - 1].name !== '/' && <ChevronRight size={13} aria-hidden="true" className="muted" />}
              <button type="button" className="fb-crumb-btn" onClick={() => go(c.path)} aria-current={i === all.length - 1 ? 'location' : undefined}>
                {c.name}
              </button>
            </span>
          ))}
        </nav>
        {data?.path && (
          <button type="button" className="btn btn-plain btn-icon" onClick={() => copyPaths([data.path])} aria-label={t('fb.copy_path')} title={t('fb.copy_path')}>
            <Copy size={16} />
          </button>
        )}
        {data?.path && (
          <button
            type="button"
            className={`btn btn-plain btn-icon ${fav.has(data.path) ? 'on' : ''}`}
            onClick={() => toast(t(fav.toggle(data.path) ? 'fb.fav_added' : 'fb.fav_removed'))}
            aria-pressed={fav.has(data.path)}
            aria-label={t('fb.fav')}
            title={t('fb.fav')}
          >
            <Star size={16} className={fav.has(data.path) ? 'fb-fav-on' : ''} />
          </button>
        )}
        {onNewHere && canWrite && (
          <button type="button" className="btn btn-plain btn-icon" onClick={() => onNewHere(data.path)} aria-label={t('fb.new_here')} title={t('fb.new_here')}>
            <MessageSquarePlus size={17} />
          </button>
        )}
        {id && (
          <button type="button" className="btn btn-plain btn-icon" onClick={() => go('')} aria-label={t('fb.cwd')} title={t('fb.cwd')}>
            <LocateFixed size={17} />
          </button>
        )}
        {data?.home && (
          <button type="button" className="btn btn-plain btn-icon" onClick={() => go(data.home)} aria-label={t('fb.home')} title={t('fb.home')}>
            <Home size={17} />
          </button>
        )}
        <button
          type="button"
          className={`btn btn-plain btn-icon ${searching || filter ? 'on' : ''}`}
          onClick={() => {
            const next = !searching;
            setSearching(next);
            if (next) requestAnimationFrame(() => searchRef.current?.focus());
            else setFilter('');
          }}
          aria-pressed={searching}
          aria-label={t('fb.search')}
          title={t('fb.search_hint')}
        >
          <Search size={17} />
        </button>
        <button type="button" className={`btn btn-plain btn-icon ${hidden ? 'on' : ''}`} onClick={() => setHidden(!hidden)} aria-pressed={hidden} aria-label={t('fb.hidden')} title={t('fb.hidden')}>
          {hidden ? <Eye size={17} /> : <EyeOff size={17} />}
        </button>
        <button type="button" className="btn btn-plain btn-icon" onClick={reload} aria-label={t('term.reload')} title={t('term.reload')}>
          <RefreshCw size={17} className={state.loading ? 'spin' : ''} />
        </button>
        <span className="fb-bar-sep" aria-hidden="true" />
        {/* 新建、上传是这一栏里最常用的两样：地方够时带上文字，不用悬停猜 */}
        <button type="button" className="btn btn-plain fb-act" disabled={!canWrite} onClick={mkdir} aria-label={t('fb.new_folder')} title={t('fb.new_folder')}>
          <FolderPlus size={17} aria-hidden="true" />
          <span className="fb-act-label">{t('fb.new_folder')}</span>
        </button>
        <button type="button" className="btn btn-plain fb-act" disabled={!canWrite} onClick={() => fileInput.current?.click()} aria-label={t('fb.upload')} title={t('fb.upload_hint')}>
          <Upload size={17} aria-hidden="true" />
          <span className="fb-act-label">{t('fb.upload_short')}</span>
        </button>
        <input ref={fileInput} type="file" multiple hidden onChange={(e) => { upload(e.target.files); e.target.value = ''; }} />
      </div>
      {(searching || filter) && (
        <div className="fb-search">
          <Search size={15} aria-hidden="true" className="muted" />
          <input
            ref={searchRef}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder={t('fb.search_ph')}
            spellCheck={false}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                setFilter('');
                setSearching(false);
                listRef.current?.focus();
              } else if (e.key === 'ArrowDown' || e.key === 'Enter') {
                e.preventDefault();
                setCursor(0);
                if (e.key === 'Enter' && entries[0]) activate(entries[0]);
                else listRef.current?.focus();
              }
            }}
          />
          {filter && !deepOn && <span className="muted small">{t('fb.search_count', { n: entries.length, total: data?.entries?.length ?? 0 })}</span>}
          {deepOn && found && <span className="muted small">{found.loading ? t('fb.deep_busy') : t('fb.deep_count', { n: found.items.length })}</span>}
          <button type="button" className={`btn btn-plain fb-deep ${deep ? 'on' : ''}`} onClick={() => setDeep(!deep)} aria-pressed={deep} title={t('fb.deep_hint')}>
            {t('fb.deep')}
          </button>
        </div>
      )}
      {clipHere && (
        <div className="fb-clipbar" role="status">
          {clipHere.op === 'move' ? <Scissors size={15} aria-hidden="true" /> : <CopyPlus size={15} aria-hidden="true" />}
          <span className="small grow ellipsis">{t(clipHere.op === 'move' ? 'fb.clip_move_bar' : 'fb.clip_copy_bar', { n: clipHere.items.length, names: names(clipHere.items) })}</span>
          <button type="button" className="btn btn-primary btn-sm" disabled={!canWrite} onClick={paste} title={t('fb.paste_hint')}>
            <ClipboardPaste size={15} aria-hidden="true" />
            {t('fb.paste_here')}
          </button>
          <button type="button" className="btn btn-plain btn-icon" onClick={() => setClip(null)} aria-label={t('fb.clip_cancel')} title={t('fb.clip_cancel')}>
            <X size={15} />
          </button>
        </div>
      )}
      {offline && data && <div className="fb-offline small">{t('fb.offline')}</div>}
      {busy.length > 0 && (
        <div className="fb-progress" role="status">
          <span className="small">{t('fb.uploading', { n: busy.length, pct })}</span>
          <div className="files-bar grow" aria-hidden="true">
            <i style={{ width: `${pct}%` }} />
          </div>
          <button type="button" className="btn btn-plain btn-icon" onClick={() => store.cancelWhere((x) => ACTIVE.includes(x.state))} aria-label={t('files.cancel')} title={t('files.cancel')}>
            <X size={15} />
          </button>
        </div>
      )}
      {batch.busy && (
        <div className="fb-progress" role="status">
          <Download size={15} aria-hidden="true" />
          <span className="small ellipsis grow">{t('fb.dl_progress', { n: batch.busy.n, total: batch.busy.total, name: batch.busy.name })}</span>
        </div>
      )}
      {!touchUi && pickbar}
      <div className="fb-main" ref={mainRef}>
        <div className="fb-list" role="grid" aria-busy={state.loading} aria-rowcount={entries.length} tabIndex={0} ref={listRef} onKeyDown={onKey}>
          {data && !data.denied && entries.length > 0 && (
            <div className="fb-head" role="row">
              <span className="fb-c-check" role="columnheader">
                <input type="checkbox" checked={allOn} ref={(el) => el && (el.indeterminate = picked.length > 0 && !allOn)} onChange={toggleAll} aria-label={t('fb.pick_all')} />
              </span>
              {sortBtn('name', t('fb.name'), 'fb-c-name')}
              {sortBtn('mtime', t('fb.mtime'), 'fb-c-time')}
              {sortBtn('size', t('fb.size'), 'fb-c-size')}
              <span className="fb-c-acts" aria-hidden="true" />
            </div>
          )}
          {state.error && <p className="fb-pad small" style={{ color: 'var(--danger)' }}>{errorText(t, state.error)}</p>}
          {data?.denied && <p className="fb-pad small muted">{t('fb.denied')}</p>}
          {data && !data.exists && <p className="fb-pad small muted">{t('fb.moved')}</p>}
          {data && !data.denied && data.entries.length === 0 && <p className="fb-pad small muted">{t('fb.empty')}</p>}
          {data && data.entries.length > 0 && entries.length === 0 && <p className="fb-pad small muted">{t('fb.search_none', { q: filter })}</p>}
          {deepOn && found && (
            <div className="fb-deep-list">
              {found.error && <p className="fb-pad small" style={{ color: 'var(--danger)' }}>{errorText(t, found.error.status === 404 ? { code: 'CLIENT_TOO_OLD' } : found.error)}</p>}
              {!found.loading && !found.error && !found.items.length && <p className="fb-pad small muted">{t('fb.search_none', { q: filter })}</p>}
              {found.items.map((h) => (
                <HitRow key={h.path} hit={h} now={now} sub={relDir(h.path, data?.path) || '.'} onPick={(x) => (x.is_dir ? go(x.path) : openAt(x.path))} />
              ))}
              {found.truncated && <p className="muted small fb-pad">{t('fb.deep_more')}</p>}
            </div>
          )}
          {!deepOn && entries.map((e, idx) => {
            const { kind, Icon } = fileKind(e.name, e.is_dir);
            const selected = open?.path === e.path;
            const on = checked.has(e.name);
            const time = fileTime(t, e.mtime_ms, now);
            const size = e.is_dir ? '' : formatBytes(e.size ?? NaN);
            const mark = marks.get(e.path);
            return (
              <div key={e.name} role="row" data-idx={idx} data-name={e.name} aria-selected={selected || on} className={`fb-row ${selected ? 'selected' : ''} ${on ? 'picked' : ''} ${cursor === idx ? 'cursor' : ''}`} {...longPress.bind(e)}>
                <span className="fb-c-check">
                  <input type="checkbox" checked={on} onChange={() => toggle(e.name)} aria-label={t('fb.pick', { name: e.name })} />
                </span>
                <button
                  type="button"
                  className="fb-name fb-c-name"
                  onClick={(ev) => {
                    // 长按刚勾上：这次松手不算点；手机多选模式里点一下是勾选 / 取消
                    if (longPress.fired.current) {
                      longPress.fired.current = false;
                      return;
                    }
                    setCursor(idx);
                    if (checked.size && (touchUi || ev.metaKey || ev.ctrlKey)) toggle(e.name);
                    else if (ev.shiftKey && cursor >= 0 && !touchUi) {
                      const [a, b] = [Math.min(cursor, idx), Math.max(cursor, idx)];
                      setChecked((cur) => new Set([...cur, ...entries.slice(a, b + 1).map((x) => x.name)]));
                    } else if ((ev.metaKey || ev.ctrlKey) && !touchUi) toggle(e.name);
                    else activate(e);
                  }}
                  title={e.path}
                >
                  <Icon size={18} aria-hidden="true" className={`fb-ico fb-ico-${kind}`} />
                  <span className="fb-name-text">
                    <span className="fb-name-main">
                      {e.name}
                      {e.link && <Link2 size={12} aria-label={t('fb.link')} className="muted fb-link" />}
                      {mark && (mark === 'dir' ? <i className="fb-git-dot" title={t('fb.git_dir')} /> : <b className={`fb-git fb-git-${mark === '!' ? 'x' : mark}`} title={t(`fb.git_${mark === '!' ? 'x' : mark}`, { defaultValue: t('fb.git_M') })}>{mark}</b>)}
                    </span>
                    {/* 窄的时候（开着预览、手机）时间和大小挪到名字下面 */}
                    <span className="fb-sub muted" title={fullTime(e.mtime_ms)}>{[time, size].filter(Boolean).join(' · ')}</span>
                  </span>
                </button>
                <span className="fb-c-time muted small" title={fullTime(e.mtime_ms)}>{time}</span>
                <span className="fb-c-size muted small">{size}</span>
                <span className="fb-acts fb-c-acts">
                  {insert && (
                    <button type="button" className="btn btn-plain btn-icon fb-more" onClick={() => insert(e.path)} aria-label={insertLabel} title={insertLabel}>
                      <TextCursorInput size={15} />
                    </button>
                  )}
                  {e.is_dir && onNewHere && (
                    <button type="button" className="btn btn-plain btn-icon fb-more" onClick={() => onNewHere(e.path)} aria-label={`${t('fb.new_here_dir')} ${e.name}`} title={t('fb.new_here_dir')}>
                      <MessageSquarePlus size={15} />
                    </button>
                  )}
                  {!e.is_dir && (
                    <button type="button" className="btn btn-plain btn-icon" onClick={() => batch.run([e])} aria-label={`${t('files.download')} ${e.name}`} title={t('files.download')}>
                      <Download size={15} />
                    </button>
                  )}
                  <button type="button" className="btn btn-plain btn-icon fb-more" disabled={offline} onClick={() => rename(e)} aria-label={`${t('fb.rename')} ${e.name}`} title={t('fb.rename')}>
                    <PenLine size={15} />
                  </button>
                  <button type="button" className="btn btn-plain btn-icon fb-more fb-danger" disabled={offline} onClick={() => remove([e])} aria-label={`${t('fb.delete')} ${e.name}`} title={t('fb.delete')}>
                    <Trash2 size={15} />
                  </button>
                </span>
              </div>
            );
          })}
        </div>
        {open && <Splitter box={mainRef} width={previewW} onWidth={setWidth} />}
        {open && (
          <div className="fb-preview-wrap" style={{ '--fb-pw': `${previewW}px` }}>
            <Preview
              key={open.path}
              ep={ep}
              entry={open}
              sep={sep}
              onClose={() => setOpen(null)}
              onInsert={insert}
              insertLabel={insertLabel}
              extra={prevNext}
              device={device}
              active={active}
              onAsk={insert && insertTo === 'chat' ? onInsert : openSession ? setChatPick : null}
            />
          </div>
        )}
      </div>
      {dropping && (
        <div className="fb-drop" aria-hidden="true">
          <Upload size={26} />
          <p>{t('fb.drop', { dir: data?.path ?? '' })}</p>
        </div>
      )}
      {touchUi && pickbar}
      <NameDialog dialog={dialog} onClose={() => setDialog(null)} />
      {chatPick && openSession && <ChatPick device={device} text={chatPick} onClose={() => setChatPick(null)} openSession={openSession} />}
    </div>
  );
}
