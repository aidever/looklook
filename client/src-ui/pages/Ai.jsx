// AI 助手：和 AI 有关的设置都在这一页，按“想做什么”分五块，顶上五张卡片就是目录，各写着现在的状态：
// - 账号与模型：AI 工具的版本与更新（AgentUpdates.jsx）、Claude Code / Codex 账号（换账号、第三方模型服务）、用量；
// - 离开电脑也能用：AI 做完、要确认时通知你（飞书、Webhook），飞书机器人里直接和会话聊；
// - 工具与权限：MCP 接口（令牌、一键装进 Codex / Claude Code / OpenCode / DSH）和它给 AI 助手的工具，自动审批；
// - 宠物：对话界面角落的宠物。
// 以前在“设置”里的 #ai、#notify、#pet、#claude-accounts 等链接都转到这里（Settings.jsx）。
// MCP 开关远程也能改；令牌和装进 AI 助手只能直接在这台电脑上看、做（见 PagesMcp.jsx）。
import { Bell, Globe, MessageSquare, PawPrint, Server, ShieldCheck, Sparkles, UserCog } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { api } from '../shared/api.js';
import { errorText } from '../shared/i18n.js';
import { Link, useRouter } from '../shared/router.jsx';
import { Button, Card, CopyButton, ErrorNote, Skeleton, Switch, useAction, useLoad, useToast } from '../shared/ui.jsx';
import { More, PageHead, Row } from '../shared/page.jsx';
import AgentUpdatesCard from './AgentUpdates.jsx';
import ClaudeProfilesCard, { CodexProfilesCard } from './ClaudeProfiles.jsx';
import GuardCard from './Guard.jsx';
import NotifySection from './Notify.jsx';
import McpCard from './PagesMcp.jsx';
import PetSection from './PetSettings.jsx';
import SkillsSection from './Skills.jsx';
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
        // 先说能做什么；工具名是给想对照日志的人看的，放后面、淡一点
        <li key={n} className={on ? '' : 'off'}>
          <span className="small">{t(`ai.tool.${n}`)}</span>
          <code className="mono">{n}</code>
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
    <div className="srow2 srow2-stack">
      <div className="srow2-text">
        <div className="srow2-label between">
          <span className="inline">
            <MessageSquare size={16} aria-hidden="true" /> {t('ai.group_feishu')}
          </span>
          {tools && <span className={`badge ${path ? 'badge-ok' : ''}`}>{t(path ? 'ai.lark_badge_ok' : 'ai.lark_badge_none')}</span>}
        </div>
        <div className="srow2-hint">{t('ai.lark_intro')}</div>
      </div>
      {body}
      {job && job.state !== 'ok' && <pre className="mono small ai-job">{job.output.slice(-2000)}</pre>}
      <ErrorNote error={actError} />
    </div>
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
  // 每样工具一行：能做什么、现在能不能用。逐台主机的开关在 SSH 主机页，工具的函数名收在最下面
  return (
    <Card title={t('ai.tools_title')} sub={mcp ? null : t('ai.tools_off')}>
      <Row label={<span className="inline"><Globe size={16} aria-hidden="true" /> {t('ai.group_pages')}</span>} hint={t('ai.pages_hint')}>
        <span className={`badge ${mcp ? 'badge-ok' : ''}`}>{t(mcp ? 'ai.on' : 'ai.off')}</span>
      </Row>
      <Row
        label={<span className="inline"><Server size={16} aria-hidden="true" /> {t('ai.group_ssh')}</span>}
        hint={
          data.items.length === 0 ? (
            <>
              {t('ai.no_hosts')} <Link to="/ssh">{t('nav.ssh')}</Link>
            </>
          ) : (
            <>
              {t('ai.hosts_count', { n: allowed, total: data.items.length })} · <Link to="/ssh">{t('ai.hosts_manage')}</Link>
            </>
          )
        }
      >
        <Switch checked={ssh} disabled={busy === 'ssh'} label={t('ai.ssh_switch')} onChange={(on) => run('ssh', () => api.put('/mcp', { ssh: on }))} />
      </Row>
      <LarkGroup />
      <More summary={t('ai.tool_names')}>
        <ToolList names={PAGE_TOOLS} on={mcp} />
        <ToolList names={SSH_TOOLS} on={mcp && ssh} />
      </More>
    </Card>
  );
}

const HUB = [
  { id: 'accounts', icon: UserCog },
  { id: 'remote', icon: Bell },
  { id: 'tools', icon: ShieldCheck },
  { id: 'skills', icon: Sparkles },
  { id: 'pet', icon: PawPrint },
];
// 别处（包括以前设置页里）的锚点落到哪一块
const HUB_ALIAS = {
  ai: 'accounts',
  model: 'accounts',
  'claude-accounts': 'accounts',
  'codex-profiles': 'accounts',
  usage: 'accounts',
  updates: 'accounts',
  'agent-updates': 'accounts',
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

/** 顶上五张卡片的状态：取不到（旧版客户端没有接口等）就不写 */
function useHubStatus(version) {
  const { t } = useTranslation();
  const [st, setSt] = useState({});
  useEffect(() => {
    let alive = true;
    const get = (u) => api.get(u).catch(() => null);
    Promise.all([get('/claude-profiles'), get('/codex-profiles'), get('/notify'), get('/mcp'), get('/pets/settings'), get('/pets'), get('/skills')]).then(([cp, xp, n, m, ps, pets, sk]) => {
      if (!alive) return;
      const c = cp?.profiles?.length ?? 0;
      const x = xp?.profiles?.length ?? 0;
      const feishu = n?.feishu;
      const hooks = n?.hooks?.length ?? 0;
      const petCount = pets?.items?.length;
      const skillCount = sk?.items?.filter((x) => x.source === 'user').length ?? 0;
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
        skills: sk ? (skillCount ? { text: t('ai.hub.skills_n', { count: skillCount }), ok: true } : { text: t('ai.hub.skills_none') }) : null,
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
  // 从设置 → 关于的“AI 工具的更新”过来：滚到版本卡片
  useEffect(() => {
    if (hash === '#updates' || hash === '#agent-updates') setTimeout(() => document.getElementById('agent-updates')?.scrollIntoView({ behavior: 'smooth', block: 'start' }), 50);
  }, [hash]);
  return (
    <>
      <PageHead title={t('ai.title')} sub={t('ai.hub.intro')} help="ai" perDevice />
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
          <AgentUpdatesCard />
          <ClaudeProfilesCard />
          <CodexProfilesCard />
          <UsageStatsCard />
        </>
      )}
      {hub === 'remote' && <NotifySection />}
      {hub === 'tools' && (
        <>
          <McpCard onChange={() => setVersion((v) => v + 1)} />
          <ToolsCard version={version} />
          <GuardCard />
        </>
      )}
      {hub === 'skills' && <SkillsSection onChange={() => setVersion((v) => v + 1)} />}
      {hub === 'pet' && <PetSection />}
    </>
  );
}
