// AI 助手：和 AI 有关的设置都在这一页，按“想做什么”分四块，顶上四张卡片就是目录，各写着现在的状态：
// - 账号与模型：Claude Code / Codex 账号（换账号、第三方模型服务）、用量；
// - 离开电脑也能用：AI 做完、要确认时通知你（飞书、Webhook），飞书机器人里直接和会话聊；
// - 工具与权限：MCP 接口（令牌、一键装进 Codex / Claude Code / OpenCode / DSH）和它给 AI 助手的工具，自动审批；
// - 宠物：对话界面角落的宠物。
// 以前在“设置”里的 #ai、#notify、#pet、#claude-accounts 等链接都转到这里（Settings.jsx）。
// MCP 开关远程也能改；令牌和装进 AI 助手只能直接在这台电脑上看、做（见 PagesMcp.jsx）。
import { Bell, Bot, Globe, MessageSquare, PawPrint, Server, ShieldCheck, UserCog, Wrench } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { api } from '../shared/api.js';
import { errorText } from '../shared/i18n.js';
import { Link, useRouter } from '../shared/router.jsx';
import { Button, Card, CopyButton, ErrorNote, Skeleton, Switch, useAction, useLoad, useToast } from '../shared/ui.jsx';
import { HelpLink } from '../shared/help.jsx';
import ClaudeProfilesCard, { CodexProfilesCard } from './ClaudeProfiles.jsx';
import GuardCard from './Guard.jsx';
import NotifySection from './Notify.jsx';
import McpCard from './PagesMcp.jsx';
import PetSection from './PetSettings.jsx';
import UsageStatsCard from './UsageStats.jsx';

const PAGE_TOOLS = ['list_mappings', 'find_mapping', 'ensure_mapping'];
const SSH_TOOLS = ['list_ssh_hosts', 'ssh_run'];
// 飞书 CLI：装（和 src/tools.rs 里一键安装的一样）、第一次登录
const LARK_INSTALL = 'npx @larksuite/cli@latest install';
const LARK_LOGIN = ['lark-cli config init --new', 'lark-cli auth login --recommend'];

function ToolList({ names, on }) {
  const { t } = useTranslation();
  return (
    <ul className="ai-tools">
      {names.map((n) => (
        <li key={n} className={on ? '' : 'off'}>
          <code className="mono small">{n}</code>
          <span className="small muted">{t(`ai.tool.${n}`)}</span>
        </li>
      ))}
    </ul>
  );
}

// 飞书：不给看看的应用申请用户权限，引导装飞书官方的 lark-cli（带技能，Claude Code、Codex 装好就会用），
// 以你本人的身份读群聊和私聊、找人、读写云文档、发消息都交给它
function LarkGroup() {
  const { t } = useTranslation();
  const [tools, error, reload] = useLoad(() => api.get('/tools'), []);
  const { busy, error: actError, run } = useAction();
  const [job, setJob] = useState(null);
  const running = job?.state === 'running';
  useEffect(() => {
    if (!running) return undefined;
    const iv = setInterval(async () => {
      const j = await api.get('/tools/install/lark-cli').catch(() => null);
      if (j) setJob(j);
      if (j?.state !== 'running') reload();
    }, 1500);
    return () => clearInterval(iv);
  }, [running]);
  const install = async () => {
    const j = await run(() => api.post('/tools/install', { tool: 'lark-cli', mirror: 'npmmirror' }));
    if (j) setJob(j);
  };
  const path = tools?.tools?.['lark-cli'];
  let body;
  if (error) body = <ErrorNote error={error} />;
  else if (!tools) body = <Skeleton height={40} />;
  else if (path)
    body = (
      <>
        <p className="small">{t('ai.lark_installed')}</p>
        <p className="small muted">{t('ai.lark_login_hint')}</p>
        {LARK_LOGIN.map((c) => (
          <div key={c} className="between" style={{ gap: 8 }}>
            <code className="mono small ellipsis">{c}</code>
            <CopyButton text={c} />
          </div>
        ))}
      </>
    );
  else
    body = (
      <>
        <div className="nt-actions">
          <Button className="btn-sm" busy={busy || running} disabled={!tools.tools?.npm} onClick={install}>
            {t('ai.lark_install')}
          </Button>
          <CopyButton text={LARK_INSTALL} label={t('ai.lark_copy')} />
        </div>
        {!tools.tools?.npm && <p className="small muted">{t('ai.lark_need_node')}</p>}
      </>
    );
  return (
    <section className="ai-group">
      <div className="ai-group-head">
        <MessageSquare size={16} aria-hidden="true" />
        <b className="grow">{t('ai.group_feishu')}</b>
        {tools && <span className={`badge ${path ? 'badge-ok' : ''}`}>{t(path ? 'ai.lark_badge_ok' : 'ai.lark_badge_none')}</span>}
      </div>
      <p className="small muted">{t('ai.lark_intro')}</p>
      {body}
      {job && job.state !== 'ok' && <pre className="mono small ai-job">{job.output.slice(-2000)}</pre>}
      <ErrorNote error={actError} />
    </section>
  );
}

function ToolsCard({ version }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [data, error, reload] = useLoad(() => api.get('/ssh/hosts'), [version]);
  const [busy, setBusy] = useState(null);
  if (error) return <ErrorNote error={error} />;
  if (!data) return <Skeleton height={200} />;
  const { mcp, ssh } = data.ai;
  const run = async (key, fn) => {
    setBusy(key);
    try {
      await fn();
      reload();
    } catch (e) {
      toast(errorText(t, e), 'error');
    } finally {
      setBusy(null);
    }
  };
  const allowed = data.items.filter((h) => h.allow_ai).length;
  return (
    <Card
      title={
        <span className="inline">
          <Wrench size={18} aria-hidden="true" /> {t('ai.tools_title')}
        </span>
      }
    >
      <p className="muted">{mcp ? t('ai.tools_intro') : t('ai.tools_off')}</p>
      <section className="ai-group">
        <div className="ai-group-head">
          <Globe size={16} aria-hidden="true" />
          <b className="grow">{t('ai.group_pages')}</b>
          <span className={`badge ${mcp ? 'badge-ok' : ''}`}>{t(mcp ? 'ai.on' : 'ai.off')}</span>
        </div>
        <ToolList names={PAGE_TOOLS} on={mcp} />
      </section>
      <section className="ai-group">
        <div className="ai-group-head">
          <Server size={16} aria-hidden="true" />
          <b className="grow">{t('ai.group_ssh')}</b>
          <Switch checked={ssh} disabled={busy === 'ssh'} label={t('ai.ssh_switch')} onChange={(on) => run('ssh', () => api.put('/mcp', { ssh: on }))} />
        </div>
        <p className="small muted">{t(ssh ? 'ai.ssh_on_hint' : 'ai.ssh_off_hint')}</p>
        <ToolList names={SSH_TOOLS} on={mcp && ssh} />
        {data.items.length === 0 ? (
          <p className="small muted">
            {t('ai.no_hosts')} <Link to="/ssh">{t('nav.ssh')}</Link>
          </p>
        ) : (
          <>
            <p className="small">{t('ai.hosts_count', { n: allowed, total: data.items.length })}</p>
            <ul className="list ai-hosts">
              {data.items.map((h) => (
                <li key={h.id} className={ssh ? '' : 'off'}>
                  <span className="grow" style={{ minWidth: 0 }}>
                    <b className="ellipsis">{h.name}</b>
                    <span className="mono small muted ellipsis">
                      {h.user}@{h.host}
                      {h.port !== 22 && `:${h.port}`}
                    </span>
                  </span>
                  <Switch
                    checked={h.allow_ai}
                    disabled={busy === h.id}
                    label={`${t('ssh.allow_ai')} · ${h.name}`}
                    onChange={(allow) => run(h.id, () => api.put(`/ssh/hosts/${h.id}/ai`, { allow }))}
                  />
                </li>
              ))}
            </ul>
          </>
        )}
      </section>
      <LarkGroup />
    </Card>
  );
}

const HUB = [
  { id: 'accounts', icon: UserCog },
  { id: 'remote', icon: Bell },
  { id: 'tools', icon: ShieldCheck },
  { id: 'pet', icon: PawPrint },
];
// 别处（包括以前设置页里）的锚点落到哪一块
const HUB_ALIAS = {
  ai: 'accounts',
  model: 'accounts',
  'claude-accounts': 'accounts',
  'codex-profiles': 'accounts',
  usage: 'accounts',
  notify: 'remote',
  feishu: 'remote',
  mcp: 'tools',
  guard: 'tools',
};
/** 这些锚点以前在设置页，现在在这一页（Settings.jsx 收到了就转过来） */
export const MOVED_TO_AI = new Set([...Object.keys(HUB_ALIAS), 'pet']);

function hubOf(hash) {
  const h = hash.replace(/^#/, '');
  const id = HUB_ALIAS[h] ?? h;
  return HUB.some((x) => x.id === id) ? id : 'accounts';
}

/** 顶上四张卡片的状态：取不到（旧版客户端没有接口等）就不写 */
function useHubStatus(version) {
  const { t } = useTranslation();
  const [st, setSt] = useState({});
  useEffect(() => {
    let alive = true;
    const get = (u) => api.get(u).catch(() => null);
    Promise.all([get('/claude-profiles'), get('/codex-profiles'), get('/notify'), get('/mcp'), get('/pets/settings'), get('/pets')]).then(([cp, xp, n, m, ps, pets]) => {
      if (!alive) return;
      const c = cp?.profiles?.length ?? 0;
      const x = xp?.profiles?.length ?? 0;
      const feishu = n?.feishu;
      const hooks = n?.hooks?.length ?? 0;
      const petCount = pets?.items?.length;
      setSt({
        accounts: cp || xp ? { text: c + x ? t('ai.hub.accounts_n', { claude: c, codex: x }) : t('ai.hub.accounts_none'), ok: c + x > 0 } : null,
        remote: n
          ? feishu?.bound
            ? n.status?.connected === false && n.status?.error
              ? { text: t('ai.hub.feishu_down'), warn: true }
              : { text: t('ai.hub.feishu_on'), ok: true }
            : hooks
              ? { text: t('ai.hub.hooks_n', { count: hooks }), ok: true }
              : { text: t('ai.hub.remote_none') }
          : null,
        tools: m ? (m.enabled ? { text: t('ai.hub.mcp_on'), ok: true } : { text: t('ai.hub.mcp_off') }) : null,
        pet: ps
          ? ps.enabled && petCount !== 0
            ? { text: t('ai.hub.pet_on'), ok: true }
            : { text: petCount === 0 ? t('ai.hub.pet_none') : t('ai.hub.pet_off') }
          : null,
      });
    });
    return () => {
      alive = false;
    };
  }, [version, t]);
  return st;
}

export default function AiPage() {
  const { t } = useTranslation();
  const { hash, navigate } = useRouter();
  const hub = hubOf(hash);
  // MCP 开关一变，工具卡片跟着刷新；换一块时顶上的状态也重新取
  const [version, setVersion] = useState(0);
  const status = useHubStatus(`${hub}:${version}`);
  const go = (id) => navigate(`/ai#${id}`, { replace: true });
  return (
    <>
      <div className="page-head">
        <h1 className="inline">
          <Bot size={24} aria-hidden="true" /> {t('ai.title')} <HelpLink page="ai" />
        </h1>
      </div>
      <p className="muted">{t('ai.hub.intro')}</p>
      <nav className="ai-hub" aria-label={t('ai.title')}>
        {HUB.map(({ id, icon: Icon }) => {
          const s = status[id];
          return (
            <a
              key={id}
              href={`/ai#${id}`}
              className={`ai-hub-item${hub === id ? ' active' : ''}`}
              aria-current={hub === id ? 'page' : undefined}
              onClick={(e) => {
                e.preventDefault();
                go(id);
              }}
            >
              <span className="ai-hub-icon" aria-hidden="true">
                <Icon size={18} />
              </span>
              <span className="ai-hub-text">
                <b>{t(`ai.hub.${id}`)}</b>
                <span className="small muted">{t(`ai.hub.${id}_desc`)}</span>
                {s && (
                  <span className={`ai-hub-state small${s.ok ? ' ok' : ''}${s.warn ? ' warn' : ''}`}>
                    <i aria-hidden="true" />
                    {s.text}
                  </span>
                )}
              </span>
            </a>
          );
        })}
      </nav>
      {hub === 'accounts' && (
        <>
          <ClaudeProfilesCard />
          <CodexProfilesCard />
          <UsageStatsCard />
        </>
      )}
      {hub === 'remote' && <NotifySection />}
      {hub === 'tools' && (
        <>
          <p className="muted">{t('ai.intro')}</p>
          <McpCard onChange={() => setVersion((v) => v + 1)} />
          <ToolsCard version={version} />
          <GuardCard />
        </>
      )}
      {hub === 'pet' && <PetSection />}
    </>
  );
}
