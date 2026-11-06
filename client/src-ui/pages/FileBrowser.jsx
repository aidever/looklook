// 文件浏览：终端页的“文件”标签（传 id：从终端的当前目录开始）和管理台的“文件”页（不传 id：从主目录开始）。
// 浏览这台电脑上的文件，预览文本、图片、Markdown 和办公文件（PDF、Word、Excel、CSV，OfficePreview.jsx），下载、把路径填进终端；新建文件夹、重命名、删除、上传（拖进列表也行）。
// 终端标签的浏览、预览、下载挂在 /api/instances/{id}/… 下面，多台电脑时按终端路由到它所在的电脑（服务端 edge/route.rs）；
// “文件”页与文件管理用 /api/fs/…，多台电脑时发给选中的那台（X-LL-Device）。
import { ArrowUp, ArrowUpDown, BookOpen, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, Code2, Copy, Download, Eye, EyeOff, File, FileArchive, FileAudio, FileCode, FileSpreadsheet, FileText, FileVideo, Folder, FolderPlus, Home, Image as ImageIcon, Link2, LocateFixed, PenLine, RefreshCw, Search, TextCursorInput, Trash2, Upload, WrapText, X } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';

import { deviceApi, fetchRaw, plainLinksWork, rtcMode, saveViaFrame, selectedDevice, terminalDevice } from '../shared/api.js';
import { errorText } from '../shared/i18n.js';
import Markdown, { isMarkdown } from '../shared/Markdown.jsx';
import { OFFICE_MAX, officeKind, parseCsv, WHOLE } from '../shared/office.js';
import { relativeTime } from '../shared/time.js';
import { Button, ErrorNote, Field, Modal, useConfirm, useToast } from '../shared/ui.jsx';
import { dirUploadStore } from '../shared/uploader.js';
import { formatBytes, quotePath } from '../shared/xfer.js';
import { renderOffice, SheetView } from './OfficePreview.jsx';

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|ico|avif)$/i;
const PREVIEW_MAX = 1024 * 1024; // 与 transfer::PREVIEW_MAX 一致
const ACTIVE = ['queued', 'uploading', 'retrying'];

/** 终端标签（id）或“文件”页（无 id）用的接口地址。 */
export function endpoints(id) {
  const q = (path) => `?path=${encodeURIComponent(path)}`;
  return id
    ? { list: (path, hidden) => `/instances/${id}/fs${q(path)}&hidden=${hidden ? 1 : 0}`, preview: (p) => `/instances/${id}/preview${q(p)}`, download: (p) => `/instances/${id}/download${q(p)}`, plain: true }
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

/** 带行号显示的最多这么多行（再多一行一个元素太慢，照旧整段显示） */
const NUMBER_MAX = 20000;

/** 带行号的文字：`focus`（{ from, to }，从 1 开始）那几行标出来，打开时滚到那里 */
function NumberedText({ text, wrap, focus }) {
  const box = useRef(null);
  const lines = useMemo(() => {
    const l = text.split('\n');
    if (l.length > 1 && l[l.length - 1] === '') l.pop();
    return l;
  }, [text]);
  const from = focus?.from ?? 0;
  const to = focus?.to ?? from;
  useEffect(() => {
    if (!from) return;
    const el = box.current?.children[from - 1];
    // 标出的几行放在上面三分之一，前面留几行看上下文
    el?.scrollIntoView({ block: 'start' });
    const body = box.current?.closest('.fb-preview-body');
    if (el && body) body.scrollTop = Math.max(0, body.scrollTop - body.clientHeight / 4);
  }, [text, from]);
  return (
    <div ref={box} className={`fb-text fb-lines ${wrap ? 'fb-wrap' : ''}`}>
      {lines.map((l, n) => (
        // eslint-disable-next-line react/no-array-index-key -- 行没有别的标识
        <div key={n} className={from && n + 1 >= from && n + 1 <= to ? 'on' : undefined}>
          <span className="fb-no" aria-hidden="true">{n + 1}</span>
          <span>{l || ' '}</span>
        </div>
      ))}
    </div>
  );
}

/**
 * 一个文件的预览（“文件”页右边；对话页里点到的文件也用它，见 ChatView 的 FileViewer）。
 * 对话页另外传：numbered（文字带行号）、focus（{ from, to }：标出这几行并滚过去）、extra（标题栏关闭按钮前的按钮）。
 */
export function Preview({ ep, entry, onClose, onInsert, sep, numbered = false, focus = null, extra = null }) {
  const { t } = useTranslation();
  const [state, setState] = useState({ loading: true });
  const [wrap, setWrap] = useState(true);
  const md = isMarkdown(entry.name);
  const office = officeKind(entry.name);
  const csv = office === 'csv';
  // Markdown 看排版效果 / 源码，CSV 看表格 / 源码；要标出某几行时先看源码
  const [rendered, setRendered] = useState(!focus?.from);
  const image = IMAGE.test(entry.name);
  const whole = WHOLE.includes(office);
  useEffect(() => {
    let alive = true;
    let url = null;
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
        const text = decodeText(await res.arrayBuffer());
        if (alive) setState({ text, truncated: size > PREVIEW_MAX });
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
  return (
    <div className="fb-preview" role="region" aria-label={entry.name}>
      <div className="fb-preview-head">
        {image ? <ImageIcon size={16} aria-hidden="true" /> : office === 'xlsx' || csv ? <FileSpreadsheet size={16} aria-hidden="true" /> : <FileText size={16} aria-hidden="true" />}
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
        {!image && !showMd && !showSheet && state.text != null && (
          <button type="button" className={`btn btn-plain btn-icon ${wrap ? 'on' : ''}`} onClick={() => setWrap(!wrap)} aria-pressed={wrap} aria-label={t('fb.wrap')} title={t('fb.wrap')}>
            <WrapText size={16} />
          </button>
        )}
        {onInsert && (
          <button type="button" className="btn btn-plain btn-icon" onClick={() => onInsert(entry.path)} aria-label={t('fb.insert')} title={t('fb.insert')}>
            <TextCursorInput size={16} />
          </button>
        )}
        {extra}
        <DownloadLink ep={ep} entry={entry} size={16} />
        <button type="button" className="btn btn-plain btn-icon" onClick={onClose} aria-label={t('fb.close_preview')} title={t('fb.close_preview')}>
          <X size={17} />
        </button>
      </div>
      <div className="fb-preview-body">
        {state.loading ? (
          <p className="muted small fb-pad">{t('fb.loading')}</p>
        ) : state.error ? (
          <p className="fb-pad small" style={{ color: 'var(--danger)' }}>{t(`files.err.${state.error.code}`, { defaultValue: errorText(t, state.error) })}</p>
        ) : image ? (
          <img src={state.url} alt={entry.name} />
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
            ) : numbered && state.text && state.text.length < NUMBER_MAX * 200 && state.text.split('\n', NUMBER_MAX + 1).length <= NUMBER_MAX ? (
              <NumberedText text={state.text} wrap={wrap} focus={focus} />
            ) : (
              <pre className={`fb-text ${wrap ? 'fb-wrap' : ''}`}>{state.text || t('fb.empty_file')}</pre>
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
 * `onInsert(text)` 把路径填进终端（没有时不显示这个按钮）。
 */
export default function FileBrowser({ id, active = true, onInsert }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const [path, setPath] = useState(''); // 空 = 终端的当前目录（“文件”页：主目录）
  const [hidden, setHidden] = useState(false);
  const [tick, setTick] = useState(0);
  const [state, setState] = useState({ loading: true });
  const [open, setOpen] = useState(null);
  const [dialog, setDialog] = useState(null);
  const [dropping, setDropping] = useState(false);
  const [sort, setSortState] = useState(() => readStore(SORT_KEY, { key: 'name', dir: 1 }));
  const [filter, setFilter] = useState('');
  const [searching, setSearching] = useState(false);
  const [checked, setChecked] = useState(() => new Set());
  const [cursor, setCursor] = useState(-1);
  const [previewW, setPreviewW] = useState(() => readStore(WIDTH_KEY, 460));
  const fileInput = useRef(null);
  const listRef = useRef(null);
  const mainRef = useRef(null);
  const searchRef = useRef(null);
  const ep = endpoints(id);
  const device = id ? terminalDevice(id) : selectedDevice();
  const ops = deviceApi(device ?? undefined);
  const store = dirUploadStore(device);
  const tasks = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const batch = useBatchDownload(ep);
  const reload = () => setTick((n) => n + 1);
  useEffect(() => {
    if (!active) return undefined;
    let alive = true;
    setState((s) => ({ ...s, loading: true }));
    ops
      .get(ep.list(path, hidden))
      .then((data) => alive && setState({ data }))
      .catch((error) => alive && setState((s) => ({ data: s.data, error })));
    return () => {
      alive = false;
    };
  }, [id, device, path, hidden, tick, active]);
  const data = state.data;
  const sep = data?.os === 'windows' ? '\\' : '/';
  // 换了文件夹：清掉勾选、筛选、光标
  useEffect(() => {
    setChecked(new Set());
    setFilter('');
    setCursor(-1);
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
  const insert = onInsert && ((p) => onInsert(`${quotePath(p, data?.os)} `));
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
  const remove = async (list) => {
    const one = list.length === 1 ? list[0] : null;
    const ok = await confirm(
      one
        ? { title: t(one.is_dir ? 'fb.delete_dir_title' : 'fb.delete_title', { name: one.name }), body: t(one.is_dir ? 'fb.delete_dir_body' : 'fb.delete_body', { path: one.path }), confirm: t('fb.delete'), danger: true }
        : { title: t('fb.delete_n_title', { n: list.length }), body: t('fb.delete_n_body', { names: list.slice(0, 5).map((e) => e.name).join('、') + (list.length > 5 ? '…' : '') }), confirm: t('fb.delete'), danger: true },
    );
    if (!ok) return;
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
    if (done) toast(one ? t('fb.deleted', { name: one.name }) : t('fb.deleted_n', { n: done }));
    setChecked(new Set());
    reload();
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
  const canWrite = Boolean(data?.path && data.exists !== false && !data.denied);

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
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
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
    } else if (e.key === 'Backspace' && data?.parent) {
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
        <button type="button" className="btn btn-plain btn-icon" disabled={!data?.parent} onClick={() => go(data.parent)} aria-label={t('fb.up')} title={t('fb.up')}>
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
          {filter && <span className="muted small">{t('fb.search_count', { n: entries.length, total: data?.entries?.length ?? 0 })}</span>}
        </div>
      )}
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
      {checked.size > 0 && (
        <div className="fb-pickbar" role="toolbar" aria-label={t('fb.picked_actions')}>
          <button type="button" className="btn btn-plain btn-icon" onClick={() => setChecked(new Set())} aria-label={t('fb.pick_clear')} title={t('fb.pick_clear')}>
            <X size={16} />
          </button>
          <span className="small grow ellipsis">
            <b>{t('fb.picked', { n: picked.length })}</b>
            {pickedBytes > 0 && <span className="muted"> · {formatBytes(pickedBytes)}</span>}
          </span>
          <button type="button" className="btn btn-plain fb-act" onClick={() => batch.run(picked)} disabled={!!batch.busy} title={t('fb.dl_hint', { n: BATCH_FILES, size: formatBytes(BATCH_BYTES) })}>
            <Download size={16} aria-hidden="true" />
            <span className="fb-act-label">{t('fb.download')}</span>
          </button>
          <button type="button" className="btn btn-plain fb-act" onClick={() => copyPaths(picked.map((e) => e.path))} title={t('fb.copy_paths')}>
            <Copy size={16} aria-hidden="true" />
            <span className="fb-act-label">{t('fb.copy_paths_short')}</span>
          </button>
          {insert && (
            <button type="button" className="btn btn-plain fb-act" onClick={() => onInsert(`${picked.map((e) => quotePath(e.path, data?.os)).join(' ')} `)} title={t('fb.insert')}>
              <TextCursorInput size={16} aria-hidden="true" />
              <span className="fb-act-label">{t('fb.insert_short')}</span>
            </button>
          )}
          <button type="button" className="btn btn-plain fb-act fb-danger" onClick={() => remove(picked)} title={t('fb.delete')}>
            <Trash2 size={16} aria-hidden="true" />
            <span className="fb-act-label">{t('fb.delete')}</span>
          </button>
        </div>
      )}
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
          {entries.map((e, idx) => {
            const { kind, Icon } = fileKind(e.name, e.is_dir);
            const selected = open?.path === e.path;
            const on = checked.has(e.name);
            const time = fileTime(t, e.mtime_ms, now);
            const size = e.is_dir ? '' : formatBytes(e.size ?? NaN);
            return (
              <div key={e.name} role="row" data-idx={idx} aria-selected={selected || on} className={`fb-row ${selected ? 'selected' : ''} ${on ? 'picked' : ''} ${cursor === idx ? 'cursor' : ''}`}>
                <span className="fb-c-check">
                  <input type="checkbox" checked={on} onChange={() => toggle(e.name)} aria-label={t('fb.pick', { name: e.name })} />
                </span>
                <button
                  type="button"
                  className="fb-name fb-c-name"
                  onClick={() => {
                    setCursor(idx);
                    activate(e);
                  }}
                  title={e.path}
                >
                  <Icon size={18} aria-hidden="true" className={`fb-ico fb-ico-${kind}`} />
                  <span className="fb-name-text">
                    <span className="fb-name-main">
                      {e.name}
                      {e.link && <Link2 size={12} aria-label={t('fb.link')} className="muted fb-link" />}
                    </span>
                    {/* 窄的时候（开着预览、手机）时间和大小挪到名字下面 */}
                    <span className="fb-sub muted" title={fullTime(e.mtime_ms)}>{[time, size].filter(Boolean).join(' · ')}</span>
                  </span>
                </button>
                <span className="fb-c-time muted small" title={fullTime(e.mtime_ms)}>{time}</span>
                <span className="fb-c-size muted small">{size}</span>
                <span className="fb-acts fb-c-acts">
                  {insert && (
                    <button type="button" className="btn btn-plain btn-icon fb-more" onClick={() => insert(e.path)} aria-label={t('fb.insert')} title={t('fb.insert')}>
                      <TextCursorInput size={15} />
                    </button>
                  )}
                  {!e.is_dir && (
                    <button type="button" className="btn btn-plain btn-icon" onClick={() => batch.run([e])} aria-label={`${t('files.download')} ${e.name}`} title={t('files.download')}>
                      <Download size={15} />
                    </button>
                  )}
                  <button type="button" className="btn btn-plain btn-icon fb-more" onClick={() => rename(e)} aria-label={`${t('fb.rename')} ${e.name}`} title={t('fb.rename')}>
                    <PenLine size={15} />
                  </button>
                  <button type="button" className="btn btn-plain btn-icon fb-more fb-danger" onClick={() => remove([e])} aria-label={`${t('fb.delete')} ${e.name}`} title={t('fb.delete')}>
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
            <Preview ep={ep} entry={open} sep={sep} onClose={() => setOpen(null)} onInsert={insert} extra={prevNext} />
          </div>
        )}
      </div>
      {dropping && (
        <div className="fb-drop" aria-hidden="true">
          <Upload size={26} />
          <p>{t('fb.drop', { dir: data?.path ?? '' })}</p>
        </div>
      )}
      <NameDialog dialog={dialog} onClose={() => setDialog(null)} />
    </div>
  );
}
