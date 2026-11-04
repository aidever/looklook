// AI 助手 → 账号 → 用量统计：Claude Code、Codex 的会话记录里的 token 用量，按 models.dev 的 API 价格估算费用
// （usage_stats.rs，后台每 10 分钟扫一遍；价格每天取一次）。订阅用户看到的是“按 API 价格折算”的数。
// 只在这台电脑上统计，不上传。
//
// 参考了 ccusage（按天 / 会话 / 项目、按工具分）、OpenRouter Activity（缓存命中率、按维度切、导出）、
// Claude Code Usage Monitor（今天按小时看消耗速度）：上面是合计和环比，中间是按天（今天按小时）的柱子，
// 下面按 工具 / 账号 / 模型 / 项目 / 会话 分开看。
import { Download, RefreshCw } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { api } from '../shared/api.js';
import { formatTime } from '../shared/time.js';
import { Button, Card, ErrorNote, Seg, Skeleton, useAction, useLoad } from '../shared/ui.jsx';

const RANGES = [1, 7, 30, 90, 365];
const AGENT_NAME = { claude: 'Claude Code', codex: 'Codex' };
const AGENT_COLOR = { claude: 'var(--brand)', codex: 'var(--brand-2)' };
const agentName = (a) => AGENT_NAME[a] ?? a;
const agentColor = (a) => AGENT_COLOR[a] ?? 'var(--info)';
const VIEWS = ['agents', 'accounts', 'models', 'projects', 'sessions'];

function money(n) {
  if (!n) return '$0';
  return n < 0.01 ? '<$0.01' : `$${n.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

function tokens(n) {
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(n ?? 0);
}

const pct = (x) => `${Math.round(x * 100)}%`;

/** 缓存命中率：读缓存 / 全部输入（不含缓存的输入 + 读缓存 + 写缓存） */
function hitRate(s) {
  const all = (s?.input ?? 0) + (s?.cache_read ?? 0) + (s?.cache_write ?? 0);
  return all ? s.cache_read / all : null;
}

const baseName = (dir) => (dir ? dir.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || dir : '');

/** 环比：比上一个同样长的时间段多了 / 少了多少 */
function Delta({ now, prev }) {
  const { t } = useTranslation();
  if (!prev) return now ? <span className="small muted">{t('usage.delta_new')}</span> : null;
  const d = (now - prev) / prev;
  if (Math.abs(d) < 0.005) return <span className="small muted">{t('usage.delta_same')}</span>;
  return <span className={`small usage-delta ${d > 0 ? 'up' : 'down'}`}>{t(d > 0 ? 'usage.delta_up' : 'usage.delta_down', { p: pct(Math.abs(d)) })}</span>;
}

/** 柱子：一根一天（或今天的一个小时），按工具叠起来；悬停看明细 */
function Bars({ items, metric, label }) {
  const { t } = useTranslation();
  const val = (x) => (metric === 'cost' ? x.cost : x.tokens);
  const fmt = metric === 'cost' ? money : (n) => `${tokens(n)} tokens`;
  const max = Math.max(...items.map(val), 0);
  if (!max) return <p className="small muted">{t(metric === 'cost' ? 'usage.no_cost' : 'usage.no_usage')}</p>;
  const order = [...new Set(items.flatMap((d) => Object.keys(d.agents ?? {})))].sort();
  const tip = (d) =>
    [
      `${d.label} · ${money(d.cost)} · ${tokens(d.tokens)} tokens · ${t('usage.requests', { n: d.requests })}`,
      ...Object.entries(d.agents ?? {}).map(([a, v]) => `${agentName(a)}: ${money(v.cost)} · ${tokens(v.tokens)}`),
    ].join('\n');
  const mid = items[Math.floor((items.length - 1) / 2)];
  return (
    <div className="usage-chart" role="img" aria-label={label}>
      <span className="usage-max small muted">{fmt(max)}</span>
      <div className="usage-bars">
        {items.map((d) => (
          <span key={d.label} className="usage-bar-hit" title={tip(d)}>
            <span className="usage-bar" style={{ height: `${Math.max((val(d) / max) * 100, val(d) ? 2 : 0)}%` }}>
              {order.map((a) => {
                const v = d.agents?.[a];
                return v && val(v) ? <i key={a} style={{ flexGrow: val(v), background: agentColor(a) }} /> : null;
              })}
            </span>
          </span>
        ))}
      </div>
      <div className="usage-axis small muted">
        <span>{items[0]?.short}</span>
        {items.length > 4 && <span>{mid?.short}</span>}
        <span>{items[items.length - 1]?.short}</span>
      </div>
      {order.length > 1 && (
        <div className="usage-legend small muted">
          {order.map((a) => (
            <span key={a}>
              <i style={{ background: agentColor(a) }} aria-hidden="true" />
              {agentName(a)}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/** 占比：一根细条 + 百分比 */
function Share({ v, total }) {
  const p = total ? v / total : 0;
  return (
    <span className="usage-share">
      <span className="usage-share-bar" aria-hidden="true">
        <i style={{ width: `${Math.min(p * 100, 100)}%` }} />
      </span>
      <span className="small muted">{pct(p)}</span>
    </span>
  );
}

function csvCell(v) {
  const s = String(v ?? '');
  return /[",\n]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s;
}

/** 导出：每天（今天是每小时）每个工具一行 */
function exportCsv(data, days) {
  const head = ['date', 'hour', 'tool', 'input', 'output', 'cache_read', 'cache_write', 'tokens', 'requests', 'cost_usd'];
  const today = data.days[data.days.length - 1]?.date ?? '';
  const buckets = days === 1 ? data.hours.map((h) => [today, h.hour, h]) : data.days.map((d) => [d.date, '', d]);
  const lines = [head.join(',')];
  for (const [date, hour, b] of buckets) {
    for (const [a, v] of Object.entries(b.agents ?? {})) {
      lines.push([date, hour, agentName(a), v.input, v.output, v.cache_read, v.cache_write, v.tokens, v.requests, v.cost].map(csvCell).join(','));
    }
  }
  const url = URL.createObjectURL(new Blob([`﻿${lines.join('\n')}\n`], { type: 'text/csv;charset=utf-8' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = `looklook-usage-${today}-${days}d.csv`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** 一张明细表：第一列名字，后面 Token / 请求 / 占比 / 费用；`extra` 是名字后面多出来的列 */
function Breakdown({ rows, metric, total, name, extra = [] }) {
  const { t } = useTranslation();
  const val = (x) => (metric === 'cost' ? x.cost : x.tokens);
  return (
    <div className="table-wrap">
      <table className="table usage-table">
        <thead>
          <tr>
            <th>{name}</th>
            {extra.map((c) => (
              <th key={c.head} className="num usage-col-extra">
                {c.head}
              </th>
            ))}
            <th className="num">{t('usage.col_tokens')}</th>
            <th className="num usage-col-share">{t('usage.col_share')}</th>
            <th className="num">{t('usage.col_cost')}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key}>
              <td>{r.label}</td>
              {extra.map((c) => (
                <td key={c.head} className="num usage-col-extra" title={c.title?.(r.raw)}>
                  {c.cell(r.raw)}
                </td>
              ))}
              <td className="num">{tokens(r.raw.tokens)}</td>
              <td className="num usage-col-share">
                <Share v={val(r.raw)} total={total} />
              </td>
              <td className="num" title={r.costTitle}>
                {r.raw.priced === false ? <span className="muted">{t('usage.no_price')}</span> : money(r.raw.cost)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function UsageStatsCard() {
  const { t } = useTranslation();
  const [days, setDays] = useState(30);
  const [view, setView] = useState('agents');
  const [metricPick, setMetric] = useState(null);
  // 按界面所在时区分天
  const tz = -new Date().getTimezoneOffset();
  const [data, error, reload] = useLoad(() => api.get(`/usage-stats?days=${days}&tz=${tz}`), [days]);
  const { busy, error: scanErr, run } = useAction();
  const rescan = async () => {
    if (await run(() => api.post(`/usage-stats/scan?days=${days}&tz=${tz}`, {}))) reload();
  };
  const ready = data?.ready;
  // 没选时：算得出费用就按费用看，都是 0（套餐、本机模型）就按 token 看
  const metric = metricPick ?? (ready && !data.total.cost && data.total.tokens ? 'tokens' : 'cost');

  const bars = useMemo(() => {
    if (!ready) return [];
    if (days === 1) return (data.hours ?? []).map((h) => ({ ...h, label: `${String(h.hour).padStart(2, '0')}:00`, short: `${h.hour}:00` }));
    return data.days.map((d) => ({ ...d, label: d.date, short: d.date.slice(5) }));
  }, [data, days, ready]);

  const totalVal = ready ? (metric === 'cost' ? data.total.cost : data.total.tokens) : 0;
  const hit = ready ? hitRate(data.total) : null;
  const lastHour = days === 1 && bars.length ? bars[bars.length - 1] : null;

  const tables = ready && {
    agents: {
      name: t('usage.col_tool'),
      extra: [
        { head: t('usage.col_sessions'), cell: (r) => r.sessions },
        { head: t('usage.col_hit'), cell: (r) => (hitRate(r) == null ? '—' : pct(hitRate(r))) },
      ],
      rows: (data.agents ?? []).map((a) => ({
        key: a.agent,
        raw: a,
        label: (
          <span className="usage-name">
            <i className="usage-dot" style={{ background: agentColor(a.agent) }} aria-hidden="true" />
            {agentName(a.agent)}
          </span>
        ),
      })),
    },
    accounts: {
      name: t('usage.col_account'),
      rows: (data.accounts ?? []).map((a) => ({
        key: `${a.agent}/${a.account}`,
        raw: a,
        label: (
          <>
            {a.name || t(a.kind === 'official' ? 'usage.account_default' : 'usage.account_api')}
            <span className="muted">
              {' · '}
              {agentName(a.agent)} · {t(`usage.kind_${a.kind || 'official'}`)}
            </span>
          </>
        ),
      })),
    },
    models: {
      name: t('usage.col_model'),
      extra: [
        { head: t('usage.col_input'), cell: (m) => tokens(m.input) },
        { head: t('usage.col_cache'), cell: (m) => tokens(m.cache_read + m.cache_write), title: (m) => t('usage.cache_hint', { read: tokens(m.cache_read), write: tokens(m.cache_write) }) },
        { head: t('usage.col_output'), cell: (m) => tokens(m.output) },
      ],
      rows: data.models.map((m) => ({
        key: `${m.agent}/${m.model}`,
        raw: m,
        costTitle: m.price_from ? t('usage.price_from', { provider: m.price_from }) : undefined,
        label: (
          <>
            <span className="mono">{m.model}</span>
            <span className="muted"> · {agentName(m.agent)}</span>
          </>
        ),
      })),
    },
    projects: {
      name: t('usage.col_project'),
      extra: [{ head: t('usage.col_sessions'), cell: (p) => p.sessions }],
      rows: (data.projects ?? []).slice(0, 30).map((p) => ({
        key: p.dir || '-',
        raw: p,
        label: p.dir ? (
          <span className="usage-proj" title={p.dir}>
            <span>{baseName(p.dir)}</span>
            <span className="muted mono usage-path ellipsis">{p.dir}</span>
          </span>
        ) : (
          <span className="muted">{t('usage.project_unknown')}</span>
        ),
      })),
    },
    sessions: {
      name: t('usage.col_session'),
      extra: [{ head: t('usage.col_requests'), cell: (s) => s.requests }],
      rows: (data.sessions ?? []).map((s, i) => ({
        key: `${s.agent}/${s.id ?? i}`,
        raw: s,
        label: (
          <span className="usage-sess">
            <span className="ellipsis">{s.title || <span className="muted">{t('usage.untitled')}</span>}</span>
            <span className="small muted ellipsis">
              {[agentName(s.agent), baseName(s.dir), s.model, s.last && formatTime(s.last)].filter(Boolean).join(' · ')}
            </span>
          </span>
        ),
      })),
    },
  };
  const tbl = tables?.[view];

  return (
    <div id="usage" className="anchor">
      <Card
        title={t('usage.title')}
        actions={
          <>
            {ready && (
              <Button variant="ghost" icon={Download} onClick={() => exportCsv(data, days)} title={t('usage.export_hint')}>
                {t('usage.export')}
              </Button>
            )}
            <Button variant="ghost" icon={RefreshCw} busy={busy} onClick={rescan} title={t('usage.rescan_hint')}>
              {t('usage.rescan')}
            </Button>
          </>
        }
      >
        <p className="muted small">{t('usage.intro')}</p>
        <ErrorNote error={error ?? scanErr} />
        {!data && !error && <Skeleton height={160} />}
        {data && !ready && <p className="small muted">{t('usage.not_ready')}</p>}
        {ready && (
          <>
            <div className="usage-controls">
              <Seg value={days} label={t('usage.range')} onChange={setDays} options={RANGES.map((d) => [d, d === 1 ? t('usage.today') : t('usage.days', { n: d })])} />
              <Seg value={metric} label={t('usage.metric')} onChange={setMetric} options={[['cost', t('usage.metric_cost')], ['tokens', 'Token']]} />
            </div>
            <div className="usage-tiles">
              <div className="usage-tile">
                <span className="small muted">{days === 1 ? t('usage.today') : t('usage.period', { n: days })}</span>
                <b>{money(data.total.cost)}</b>
                <Delta now={data.total.cost || data.total.tokens} prev={data.total.cost ? data.prev?.cost : data.prev?.tokens} />
              </div>
              <div className="usage-tile">
                <span className="small muted">Token</span>
                <b>{tokens(data.total.tokens)}</b>
                <span className="small muted" title={t('usage.hit_hint')}>
                  {hit == null ? t('usage.requests', { n: data.total.requests }) : t('usage.hit', { p: pct(hit) })}
                </span>
              </div>
              {days === 1 ? (
                <div className="usage-tile">
                  <span className="small muted">{t('usage.this_hour')}</span>
                  <b>{money(lastHour?.cost)}</b>
                  <span className="small muted">{tokens(lastHour?.tokens ?? 0)} tokens</span>
                </div>
              ) : (
                <div className="usage-tile">
                  <span className="small muted">{t('usage.today')}</span>
                  <b>{money(data.today.cost)}</b>
                  <span className="small muted">{t('usage.avg', { v: money(data.total.cost / days) })}</span>
                </div>
              )}
              <div className="usage-tile">
                <span className="small muted">{t('usage.sessions_tile')}</span>
                <b>{data.session_count ?? 0}</b>
                <span className="small muted">{t('usage.requests', { n: data.total.requests })}</span>
              </div>
            </div>
            <Bars items={bars} metric={metric} label={t(days === 1 ? 'usage.chart_label_hour' : 'usage.chart_label')} />
            {tbl && (
              <>
                <div className="usage-controls">
                  <Seg value={view} label={t('usage.by')} onChange={setView} options={VIEWS.map((v) => [v, t(`usage.by_${v}`)])} />
                </div>
                {tbl.rows.length ? <Breakdown rows={tbl.rows} metric={metric} total={totalVal} name={tbl.name} extra={tbl.extra} /> : <p className="small muted">{t('usage.no_usage')}</p>}
                {view === 'sessions' && data.session_count > tbl.rows.length && <p className="small muted">{t('usage.sessions_top', { n: tbl.rows.length, total: data.session_count })}</p>}
              </>
            )}
            <p className="muted small" style={{ margin: '12px 0 0' }}>
              {t('usage.note')} {view === 'accounts' && data.accounts?.length > 1 ? t('usage.accounts_note') : ''}
              {data.pricing ? ` ${t('usage.pricing_at', { at: formatTime(data.pricing.fetched_at) })}` : ` ${t('usage.pricing_none')}`}
              {data.scanned_at && ` ${t('usage.scanned_at', { at: formatTime(data.scanned_at) })}`}
            </p>
          </>
        )}
      </Card>
    </div>
  );
}
