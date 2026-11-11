// AI 助手页 → 技能：这台电脑上各 AI 工具在用户级目录里装的技能（后端 src/skills.rs）。
// 列表只放能管的（用户装的），按工具筛选，多了才有搜索框；Codex 内置的、Claude Code 插件带的收在底下“只能看”。
// 点一行看详情（抽屉：说明、谁在用、SKILL.md、文件、路径）；删除先进回收站，提示里能撤销，7 天后自动清掉。
// 远程看别的电脑时（页头设备切换）列的就是那台电脑的，删除一样能用。
import { ChevronRight, FolderSymlink, RefreshCw, Sparkles, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { api } from '../shared/api.js';
import { errorText } from '../shared/i18n.js';
import Markdown from '../shared/Markdown.jsx';
import { More } from '../shared/page.jsx';
import { Button, Card, CopyButton, Drawer, Empty, ErrorNote, Skeleton, Time, useConfirm, useLoad, useToast } from '../shared/ui.jsx';
import { formatBytes } from '../shared/xfer.js';

export const TOOL_NAMES = { claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', dsh: 'DSH' };
const TOOL_ORDER = ['claude', 'codex', 'opencode', 'dsh'];
const SEARCH_FROM = 8;

const skillUrl = (id) => `/skills/${id.split('/').map(encodeURIComponent).join('/')}`;
const toolList = (tools) => tools.map((x) => TOOL_NAMES[x] ?? x);
const iso = (ms) => (ms ? new Date(ms).toISOString() : null);

/** 渲染时去掉开头的 frontmatter（说明已经在上面显示了） */
function stripFrontmatter(text) {
  const m = /^---\r?\n[\s\S]*?\r?\n---[^\n]*\n?/.exec(text);
  return m ? text.slice(m[0].length) : text;
}

function ToolTags({ tools }) {
  return toolList(tools).map((n) => (
    <span key={n} className="badge skill-tag">
      {n}
    </span>
  ));
}

function SkillRow({ item, onOpen, onDelete, gone = false }) {
  const { t } = useTranslation();
  return (
    <li className={`skill-row${gone ? ' skill-gone' : ''}`}>
      <button type="button" className="skill-main" onClick={() => onOpen(item)}>
        <span className="skill-name">
          <b className="ellipsis">{item.name}</b>
          {item.ours && <span className="badge skill-tag brand">{t('skills.ours')}</span>}
          {item.link && <span className="badge skill-tag">{t('skills.link')}</span>}
        </span>
        {item.description && <span className="skill-desc small muted">{item.description}</span>}
        <span className="skill-meta small muted">
          <ToolTags tools={item.tools} />
          {item.source === 'system' && <span>{t('skills.system')}</span>}
          {item.source === 'plugin' && <span className="ellipsis">{t('skills.plugin', { name: item.plugin })}</span>}
          {item.mtime && (
            <span>
              · <Time value={iso(item.mtime)} only="relative" />
            </span>
          )}
        </span>
      </button>
      {item.deletable ? (
        <Button variant="plain" icon={Trash2} className="skill-del" aria-label={t('skills.delete_named', { name: item.name })} title={t('skills.delete')} onClick={() => onDelete(item)} />
      ) : (
        <span className="small muted nowrap">{t('skills.readonly')}</span>
      )}
      <ChevronRight size={16} className="skill-chev" aria-hidden="true" />
    </li>
  );
}

function Detail({ item, onClose, onDelete }) {
  const { t } = useTranslation();
  const [data, error] = useLoad(() => api.get(skillUrl(item.id)), [item.id]);
  const [raw, setRaw] = useState(false);
  const it = data?.item ?? item;
  return (
    <Drawer open onClose={onClose} title={it.name}>
      <div className="stack skill-detail">
        {it.description && <p className="skill-lead">{it.description}</p>}
        <div className="skill-stats small muted">
          <ToolTags tools={it.tools} />
          <span>
            {t('skills.files_n', { count: it.files })} · {formatBytes(it.bytes)}
            {it.mtime && (
              <>
                {' · '}
                <Time value={iso(it.mtime)} only="relative" />
              </>
            )}
          </span>
        </div>
        {it.source !== 'user' && <p className="small muted">{it.source === 'system' ? t('skills.system_hint') : t('skills.plugin_hint', { name: it.plugin })}</p>}
        <div>
          <p className="skill-label">{t('skills.location')}</p>
          <div className="skill-path">
            {it.link && <FolderSymlink size={14} aria-label={t('skills.link')} />}
            <code className="mono small ellipsis">{it.path}</code>
            <CopyButton text={it.path} label={false} />
          </div>
        </div>
        <ErrorNote error={error} />
        {!data && !error && <Skeleton height={160} />}
        {data && (
          <section className="skill-md">
            <div className="between">
              <p className="skill-label">SKILL.md</p>
              <button type="button" className="link-btn small" onClick={() => setRaw(!raw)}>
                {raw ? t('skills.rendered') : t('skills.raw')}
              </button>
            </div>
            {raw ? <pre className="log-box skill-raw">{data.body}</pre> : <Markdown text={stripFrontmatter(data.body)} />}
            {data.truncated && <p className="small muted">{t('skills.truncated')}</p>}
          </section>
        )}
        {data && (
          <More summary={t('skills.files_title', { count: it.files })}>
            <ul className="skill-files">
              {data.files.map(([p, n]) => (
                <li key={p}>
                  <span className="mono small ellipsis">{p}</span>
                  <span className="small muted nowrap">{formatBytes(n)}</span>
                </li>
              ))}
            </ul>
            {data.more_files && <p className="small muted">{t('skills.more_files')}</p>}
          </More>
        )}
      </div>
      {it.deletable && (
        <footer className="skill-foot">
          <span className="small muted grow">{t('skills.delete_effect', { tools: toolList(it.tools).join(t('skills.and')) })}</span>
          <Button variant="danger" icon={Trash2} onClick={() => onDelete(it)}>
            {t('skills.delete_this')}
          </Button>
        </footer>
      )}
    </Drawer>
  );
}

export default function SkillsSection({ onChange }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const [data, error, reload, loading] = useLoad(() => api.get('/skills'));
  const [tool, setTool] = useState('all');
  const [q, setQ] = useState('');
  const [open, setOpen] = useState(null);
  const [gone, setGone] = useState(() => new Set());
  // 列表刷新后，淡出中的那行已经不在了
  useEffect(() => setGone(new Set()), [data]);

  const items = data?.items ?? [];
  const mine = items.filter((x) => x.source === 'user');
  const builtin = items.filter((x) => x.source !== 'user');
  const counts = useMemo(() => {
    const c = {};
    for (const x of mine) for (const k of x.tools) c[k] = (c[k] ?? 0) + 1;
    return c;
  }, [data]);
  const tools = TOOL_ORDER.filter((k) => counts[k]);
  const needle = q.trim().toLowerCase();
  const shown = mine.filter(
    (x) => (tool === 'all' || x.tools.includes(tool)) && (!needle || x.name.toLowerCase().includes(needle) || x.description.toLowerCase().includes(needle)),
  );

  const changed = () => {
    reload();
    onChange?.();
  };

  const remove = async (it) => {
    const names = toolList(it.tools).join(t('skills.and'));
    const lines = [
      it.link ? t('skills.confirm_link', { path: it.path }) : t('skills.confirm_tools', { tools: names }),
      t('skills.confirm_reopen'),
      it.ours && t('skills.confirm_ours'),
      !it.link && t('skills.confirm_trash'),
    ].filter(Boolean);
    const ok = await confirm({
      title: t('skills.confirm_title', { name: it.name }),
      body: (
        <span className="skill-confirm">
          {lines.map((l) => (
            <span key={l}>{l}</span>
          ))}
        </span>
      ),
      confirm: t('skills.delete'),
      danger: true,
    });
    if (!ok) return;
    try {
      const r = await api.del(skillUrl(it.id));
      setOpen(null);
      setGone((s) => new Set(s).add(it.id));
      setTimeout(changed, 220);
      toast(t('skills.deleted', { name: it.name }), 'ok', {
        label: t('skills.undo'),
        onClick: async () => {
          try {
            await api.post('/skills/restore', { trash: r.trash });
            toast(t('skills.restored', { name: it.name }));
            changed();
          } catch (e) {
            toast(errorText(t, e), 'error');
          }
        },
      });
    } catch (e) {
      toast(errorText(t, e), 'error');
      changed();
    }
  };

  return (
    <Card
      id="skills"
      title={t('skills.title')}
      sub={t('skills.sub')}
      actions={<Button variant="plain" icon={RefreshCw} busy={loading && Boolean(data)} aria-label={t('action.refresh')} title={t('action.refresh')} onClick={reload} />}
    >
      {error && (
        <div className="stack">
          <ErrorNote error={error} />
          <div>
            <Button variant="secondary" onClick={reload}>
              {t('skills.retry')}
            </Button>
          </div>
        </div>
      )}
      {!data && !error && <Skeleton height={120} />}
      {data && mine.length === 0 && (
        <Empty icon={Sparkles} title={t('skills.empty_title')}>
          <p className="muted small">{t('skills.empty_body')}</p>
        </Empty>
      )}
      {mine.length > 0 && (
        <div className="skill-tools">
          {tools.length > 1 && (
            <div className="skill-chips" role="group" aria-label={t('skills.filter')}>
              {[['all', t('skills.all'), mine.length], ...tools.map((k) => [k, TOOL_NAMES[k], counts[k]])].map(([k, label, n]) => (
                <button key={k} type="button" className="skill-chip" aria-pressed={tool === k} onClick={() => setTool(k)}>
                  {label}
                  <span className="skill-chip-n">{n}</span>
                </button>
              ))}
            </div>
          )}
          {mine.length > SEARCH_FROM && (
            <input type="search" className="skill-search" value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('skills.search')} aria-label={t('skills.search')} />
          )}
        </div>
      )}
      {mine.length > 0 && (
        <ul className="skill-list">
          {shown.map((it) => (
            <SkillRow key={it.id} item={it} onOpen={setOpen} onDelete={remove} gone={gone.has(it.id)} />
          ))}
          {shown.length === 0 && <li className="small muted skill-none">{t('skills.no_match')}</li>}
        </ul>
      )}
      {builtin.length > 0 && (
        <More summary={t('skills.builtin', { count: builtin.length })}>
          <ul className="skill-list">
            {builtin.map((it) => (
              <SkillRow key={it.id} item={it} onOpen={setOpen} onDelete={remove} />
            ))}
          </ul>
        </More>
      )}
      {open && <Detail item={open} onClose={() => setOpen(null)} onDelete={remove} />}
    </Card>
  );
}
