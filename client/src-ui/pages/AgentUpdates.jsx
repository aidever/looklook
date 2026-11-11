// AI 工具版本：Claude Code、Codex、DSH 现在的版本、有没有新版本（“当前 → 新版本”）、一键更新，每个一个“自动更新”开关。
// 放在 AI 助手页“账号与模型”最上面（设置 → 关于里也有入口）。后端见 src/agent_update.rs：
// 按用户实际的安装方式更新（npm / pnpm / bun、Claude Code 安装脚本的 `claude update`、Homebrew）；
// 自动更新等到没有会话正在用这个工具时才装，不打断在跑的会话。装的进度和“一键安装”共用 `GET /tools/install/{tool}`。
import { RefreshCw } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { LaunchIcon } from '../shared/brands.jsx';
import { api } from '../shared/api.js';
import { errorText } from '../shared/i18n.js';
import { Button, Card, CopyButton, ErrorNote, Note, Skeleton, Switch, Time, useConfirm, useLoad, useToast } from '../shared/ui.jsx';

const NAMES = { claude: 'Claude Code', codex: 'Codex', dsh: 'DSH' };

/** 上次更新的结果：成功、失败（能照着做的提示） */
function LastOutcome({ item }) {
  const { t } = useTranslation();
  const o = item.last;
  // 只说和现在有关的：成功的只在装上的就是那个版本时说；失败的只在还有这个新版本时说
  if (!o) return null;
  if (o.ok) {
    if (o.to !== item.version) return null;
    return (
      <p className="small muted">
        {t(o.auto ? 'aup.done_auto' : 'aup.done', { from: o.from ?? '?', to: o.to })} · <Time value={new Date(o.at).toISOString()} only="relative" />
      </p>
    );
  }
  // 装失败时 npm 可能已经把旧版删了（显示成“没装”），这时也要说清楚是更新失败
  if (!(item.update || !item.installed) || o.target !== item.latest) return null;
  return (
    <Note kind={o.error === 'unchanged' ? 'info' : 'warn'}>
      {t(`aup.err.${o.error ?? 'failed'}`, { name: NAMES[item.tool], version: o.target })}
      {item.command && ['perm', 'busy', 'failed', 'node'].includes(o.error) && (
        <div className="between aup-cmd">
          <code className="mono small ellipsis">{item.command}</code>
          <CopyButton text={item.command} />
        </div>
      )}
    </Note>
  );
}

function AgentRow({ item, onChanged }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const [busy, setBusy] = useState('');
  const [job, setJob] = useState(null);
  const running = item.running || job?.state === 'running';
  const name = NAMES[item.tool];

  // 在装：看进度；装完等后端记下结果再刷新（后端每 2 秒看一次任务、再查一遍版本）
  useEffect(() => {
    if (!running) return undefined;
    let done = false;
    const iv = setInterval(async () => {
      const j = await api.get(`/tools/install/${item.tool}`).catch(() => null);
      if (j) setJob(j);
      if (j && j.state !== 'running' && !done) {
        done = true;
        setTimeout(onChanged, 4000);
      }
    }, 1500);
    return () => clearInterval(iv);
  }, [running, item.tool]);

  const setAuto = async (on) => {
    setBusy('auto');
    try {
      await api.put(`/agent-updates/${item.tool}`, { auto: on });
      onChanged();
    } catch (e) {
      toast(errorText(t, e), 'error');
    } finally {
      setBusy('');
    }
  };

  const update = async () => {
    // 有会话正在用：说清楚现在更新会怎样，让用户选
    const using = item.busy || item.procs;
    if (using && !(await confirm({ title: t('aup.busy_title', { name }), body: t(item.os === 'windows' ? 'aup.busy_body_win' : 'aup.busy_body', { name, count: using }), confirm: t('aup.update_anyway') }))) return;
    setBusy('update');
    try {
      setJob(await api.post(`/agent-updates/${item.tool}/update`));
    } catch (e) {
      toast(errorText(t, e), 'error');
      onChanged();
    } finally {
      setBusy('');
    }
  };

  let badge = null;
  if (!item.installed) badge = <span className="badge">{t('aup.not_installed')}</span>;
  else if (running) badge = <span className="badge badge-warn">{t('aup.updating')}</span>;
  else if (item.update) badge = <span className="badge badge-warn">{t('aup.new')}</span>;
  else if (item.latest) badge = <span className="badge badge-ok">{t('aup.latest')}</span>;

  const version = item.installed && (
    <span className="mono">
      {item.version ?? t('aup.unknown_version')}
      {item.update && <> → <b>{item.latest}</b></>}
    </span>
  );

  // 自动更新开着、有新版本、在等会话空闲
  const waiting = item.auto && item.update && item.can_update && item.waiting && !running;

  return (
    <div className="srow2 srow2-stack aup-row">
      <div className="srow2-text">
        <div className="srow2-label between">
          <span className="inline">
            <LaunchIcon launch={item.tool} size={18} /> {name}
          </span>
          {badge}
        </div>
        {item.installed ? (
          <div className="srow2-hint">
            {version} · {t(`aup.method.${item.method}`)}
          </div>
        ) : (
          <div className="srow2-hint">{t('aup.not_installed_hint')}</div>
        )}
      </div>
      {item.installed && item.can_update && (
        <div className="aup-ctl">
          <label className="aup-auto">
            <Switch checked={item.auto} disabled={busy === 'auto'} label={t('aup.auto_label', { name })} onChange={setAuto} />
            <span className="small">{t('aup.auto')}</span>
          </label>
          {item.update && (
            <Button className="btn-sm" busy={busy === 'update' || running} onClick={update}>
              {running ? t('aup.updating') : t('aup.update_to', { version: item.latest })}
            </Button>
          )}
        </div>
      )}
      {item.installed && !item.can_update && <p className="small muted">{t(item.method === 'desktop' ? 'aup.desktop' : 'aup.manual', { name })}</p>}
      {waiting && <p className="small muted">{t(item.busy ? 'aup.waiting_busy' : 'aup.waiting_procs', { count: item.busy || item.procs, name })}</p>}
      {item.installed && item.can_update && item.auto && !item.update && <p className="small muted">{t('aup.auto_on_hint')}</p>}
      {!item.installed && item.last && !item.last.ok && <p className="small muted">{t('aup.gone_after_fail', { name })}</p>}
      {!running && <LastOutcome item={item} />}
      {job && job.state !== 'ok' && job.output && <pre className="mono small ai-job">{job.output.slice(-2000)}</pre>}
    </div>
  );
}

export default function AgentUpdatesCard() {
  const { t } = useTranslation();
  const [data, error, reload] = useLoad(() => api.get('/agent-updates'), []);
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState(null);
  const check = async () => {
    setChecking(true);
    setCheckError(null);
    try {
      await api.post('/agent-updates/check');
      reload();
    } catch (e) {
      setCheckError(e);
    } finally {
      setChecking(false);
    }
  };
  const updates = data?.items?.filter((i) => i.update).length ?? 0;
  return (
    <Card
      id="agent-updates"
      title={t('aup.title')}
      sub={t('aup.sub')}
      actions={
        <Button variant="ghost" className="btn-sm" icon={RefreshCw} busy={checking} onClick={check}>
          {t('aup.check')}
        </Button>
      }
    >
      {error && <ErrorNote error={error} />}
      {!data && !error && <Skeleton height={120} />}
      {data && (
        <>
          {data.items.map((item) => (
            <AgentRow key={item.tool} item={{ ...item, os: data.os }} onChanged={reload} />
          ))}
          <p className="small muted aup-foot">
            {data.checked_at ? (
              <>
                {t('aup.checked')} <Time value={new Date(data.checked_at).toISOString()} only="relative" />
                {updates > 0 && <> · {t('aup.n_new', { count: updates })}</>}
              </>
            ) : (
              t('aup.never_checked')
            )}
          </p>
        </>
      )}
      <ErrorNote error={checkError} />
    </Card>
  );
}
