// AI 页“远程与通知”（docs/TASK_BOARD.md §6.1、§6.5，按 docs/DONT_MAKE_ME_THINK_2026-11.md 的规矩）：
// - FeishuCard：没配时一个按钮“用飞书扫码创建机器人”，手动配置收起来；配好后一行状态 + “发一条测试” + ⋯（修改、补权限、解绑、断开）；
// - AgentCard：飞书机器人（在飞书里和 AI 对话、自动建项目群、群里 @），用法和少用的选项收起来；
// - RulesCard：什么时候通知；勿扰、合并、卡住提醒、静音项目收进“少打扰”；
// - HooksCard：只通知的渠道（企业微信、钉钉、Server酱、PushPlus、Bark、ntfy），每行“发一条测试” + ⋯。
// 配置只在这台电脑上（不在电脑之间同步）；连接状态也是这台电脑的。
import { CheckCircle2, ExternalLink, KeyRound, Link2Off, Pencil, Power, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { api } from '../shared/api.js';
import { isTouch } from '../shared/KeyBar.jsx';
import PopMenu from '../shared/PopMenu.jsx';
import { useSessions } from '../shared/sessions.jsx';
import { Button, Card, CopyButton, ErrorNote, Field, More, Note, PasswordInput, QrCode, Row, Switch, useAction, useLoad } from '../shared/ui.jsx';
import Select from '../shared/Select.jsx';

const CONSOLE = {
  feishu: 'https://open.feishu.cn/app',
  lark: 'https://open.larksuite.com/app',
};
// 和 src/feishu/register.rs 里扫码时带上的一致
const SCOPES = [
  'im:message:send_as_bot',
  'im:message.p2p_msg:readonly',
  'cardkit:card:write',
  'im:resource',
  'im:message.group_at_msg:readonly',
  'im:message.reactions:write_only',
  'im:message:readonly',
  'im:chat:create',
];
const EVENTS = ['im.message.receive_v1', 'application.bot.menu_v6'];
// 机器人菜单（推送事件）的 key：新会话、正在跑的会话、任务页（src/feishu_agent.rs on_menu）
const MENU_KEYS = ['new', 'sessions', 'tasks'];
const CALLBACK = 'card.action.trigger';
// 常见错误码 → 怎么办（notify.feishu_hint_*）
const HINTS = [10003, 10014, 230013, 99991672, 1000040346, 1000040350];
// 只通知的渠道：有没有第二个输入框（钉钉加签、ntfy 令牌）、在哪拿地址
const KINDS = {
  wecom: {
    secret: false,
    help: 'https://developer.work.weixin.qq.com/document/path/91770',
  },
  serverchan: { secret: false, help: 'https://sct.ftqq.com/sendkey' },
  pushplus: { secret: false, help: 'https://www.pushplus.plus/push1.html' },
  dingtalk: {
    secret: true,
    help: 'https://open.dingtalk.com/document/robots/custom-robot-access',
  },
  bark: { secret: false, help: 'https://bark.day.app/' },
  ntfy: { secret: true, help: 'https://docs.ntfy.sh/' },
};

function Toggle({ r, k, title, hint, busy, save, disabled, checked, onChange }) {
  return (
    <Row label={title} hint={hint}>
      <Switch checked={checked ?? Boolean(r[k])} disabled={busy || disabled} onChange={onChange ?? ((v) => save({ [k]: v }))} label={title} />
    </Row>
  );
}

/** 保存规则（带上界面语言：卡片跟着界面语言） */
function useRules(data, reload) {
  const { i18n } = useTranslation();
  const { busy, error, run } = useAction();
  const r = data?.rules;
  const lang = i18n.language?.startsWith('en') ? 'en-US' : 'zh-CN';
  const save = async (patch) => {
    if (await run(() => api.put('/notify/rules', { ...r, ...patch, lang }))) reload();
  };
  return { r, lang, save, busy, error };
}

function Code({ text }) {
  return (
    <span className="nt-code">
      <code>{text}</code>
      <CopyButton text={text} label={false} />
    </span>
  );
}

function FeishuError({ error }) {
  const { t } = useTranslation();
  if (!error) return null;
  const code = error.params?.code ?? error.code;
  if (error.code === 'NOTIFY_FEISHU' || typeof code === 'number') {
    const hint = HINTS.includes(code) ? t(`notify.feishu_hint_${code}`) : '';
    return (
      <Note kind="error">
        {t('notify.feishu_error', {
          code,
          msg: error.params?.msg ?? error.msg ?? '',
        })}
        {hint && <span style={{ display: 'block' }}>{hint}</span>}
      </Note>
    );
  }
  return <ErrorNote error={error} />;
}

/** 填 App ID / Secret（向导第 3 步，或者“修改”） */
function CredsForm({ data, onSaved, onCancel }) {
  const { t } = useTranslation();
  const f = data?.feishu;
  const [appId, setAppId] = useState(f?.app_id ?? '');
  const [secret, setSecret] = useState('');
  const [domain, setDomain] = useState(f?.domain ?? 'feishu');
  const { busy, error, run } = useAction();
  const save = async (e) => {
    e.preventDefault();
    const r = await run(() =>
      api.put('/notify/feishu', {
        app_id: appId.trim(),
        app_secret: secret.trim() || undefined,
        domain,
      }),
    );
    if (r) onSaved(r);
  };
  return (
    <form className="stack" style={{ gap: 12 }} onSubmit={save}>
      <Field label={t('notify.domain')}>
        <Select value={domain} onChange={(e) => setDomain(e.target.value)}>
          <option value="feishu">{t('notify.domain_feishu')}</option>
          <option value="lark">{t('notify.domain_lark')}</option>
        </Select>
      </Field>
      <Field label="App ID">
        <input value={appId} onChange={(e) => setAppId(e.target.value)} placeholder="cli_…" autoComplete="off" spellCheck={false} />
      </Field>
      <Field label="App Secret" hint={f ? t('notify.secret_keep') : t('notify.secret_hint')}>
        <PasswordInput value={secret} onChange={(e) => setSecret(e.target.value)} autoComplete="off" spellCheck={false} />
      </Field>
      <FeishuError error={error} />
      <div className="nt-actions">
        <Button type="submit" busy={busy} disabled={!appId.trim() || (!f && !secret.trim())}>
          {t('notify.save_creds')}
        </Button>
        {onCancel && (
          <Button variant="ghost" type="button" onClick={onCancel}>
            {t('action.cancel')}
          </Button>
        )}
      </div>
    </form>
  );
}

function Step({ n, done, title, children }) {
  return (
    <li className={`nt-step${done ? ' done' : ''}`}>
      <span className="nt-step-n" aria-hidden="true">
        {done ? <CheckCircle2 size={18} /> : n}
      </span>
      <div className="nt-step-body">
        <b>{title}</b>
        {/* 做完的步骤只留标题 */}
        {!done && children}
      </div>
    </li>
  );
}

function PairCode({ pair }) {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const iv = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(iv);
  }, []);
  if (!pair) return null;
  const left = Math.max(0, Math.round((pair.expires_at - now) / 60000));
  return (
    <div className="nt-pair">
      <span className="nt-pair-code mono">{pair.code}</span>
      <CopyButton text={pair.code} />
      <span className="small muted">{t('notify.pair_left', { n: left })}</span>
    </div>
  );
}

/** 扫码建应用（或给已有的应用补权限）：二维码 + 在飞书里打开的链接，结果由设置页轮询 `register` */
function ScanSetup({ data, reload, update = false }) {
  const { t } = useTranslation();
  const { busy, error, run } = useAction();
  const [now, setNow] = useState(Date.now());
  const reg = data?.register?.update === update ? data.register : null;
  const pending = reg?.state === 'pending' && reg.expires_at > now;
  useEffect(() => {
    if (!pending) return undefined;
    const iv = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(iv);
  }, [pending]);
  const start = async () => {
    if (await run(() => api.post('/notify/feishu/register', { update }))) reload();
  };
  const cancel = async () => {
    if (await run(() => api.delete('/notify/feishu/register'))) reload();
  };
  // 还是 pending 但已过期：当成过期
  const state = reg?.state === 'pending' ? 'expired_token' : reg?.state;
  const failed = !reg || pending || state === 'done' ? '' : ['access_denied', 'expired_token'].includes(state) ? t(`notify.scan_${state}`) : t('notify.scan_failed', { code: state });
  if (pending) {
    // 手机上扫不了自己屏幕上的码：只给链接
    const touch = isTouch();
    return (
      <div className="nt-scan">
        {!touch && <QrCode text={reg.url} size={168} label={t('notify.scan_qr')} />}
        <div className="stack" style={{ gap: 8 }}>
          <b>{t(update ? 'notify.scan_update_title' : touch ? 'notify.link_title' : 'notify.scan_title')}</b>
          <span className="small muted">{t(update ? 'notify.scan_update_body' : 'notify.scan_body')}</span>
          <span className="small muted">{t(touch ? 'notify.link_hint_touch' : 'notify.link_hint')}</span>
          <div className="nt-actions">
            <a className={`btn btn-sm${touch ? ' btn-primary' : ' btn-ghost'}`} href={reg.url} target="_blank" rel="noopener noreferrer">
              {t('notify.link_open')} <ExternalLink size={12} aria-hidden="true" />
            </a>
            <CopyButton text={reg.url} label={t('notify.link_copy')} />
          </div>
          <span className="small muted">{t('notify.scan_waiting', { n: Math.max(1, Math.ceil((reg.expires_at - now) / 60000)) })}</span>
          <div className="nt-actions">
            <Button variant="ghost" className="btn-sm" disabled={busy} onClick={cancel}>
              {t('action.cancel')}
            </Button>
          </div>
        </div>
      </div>
    );
  }
  // 补权限：从 ⋯ 菜单开始，这里只显示结果
  if (update) {
    if (!failed && reg?.state !== 'done') return null;
    return failed ? <Note kind="warn">{failed}</Note> : <Note kind="ok">{t('notify.scan_update_done')}</Note>;
  }
  return (
    <div className="stack" style={{ gap: 8 }}>
      {failed && <Note kind="warn">{failed}</Note>}
      <div className="nt-actions">
        <Button busy={busy} onClick={start}>
          {t('notify.scan_start')}
        </Button>
      </div>
      <FeishuError error={error} />
    </div>
  );
}

export function FeishuCard({ data, reload }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const { busy, error, run } = useAction();
  const [tested, setTested] = useState(false);
  const f = data?.feishu;
  const st = data?.status ?? {};
  const consoleUrl = CONSOLE[f?.domain ?? 'feishu'];

  const scanning = data?.register?.state === 'pending';
  // 等扫码、等绑定、等回调验证时勾得快一点
  useEffect(() => {
    if (!scanning && f && f.bound && f.callback_ok) return undefined;
    const iv = setInterval(reload, scanning ? 2000 : 3000);
    return () => clearInterval(iv);
  }, [f?.bound, f?.callback_ok, Boolean(f), scanning]);

  const saved = () => {
    setEditing(false);
    reload();
  };
  const test = async () => {
    if ((await run(() => api.post('/notify/test'))) !== undefined) setTested(true);
  };
  const unbind = async () => {
    if (await run(() => api.post('/notify/unbind'))) reload();
  };
  const disconnect = async () => {
    if (await run(() => api.delete('/notify/feishu'))) reload();
  };
  const grant = async () => {
    if (await run(() => api.post('/notify/feishu/register', { update: true }))) reload();
  };

  const steps = (
    <ol className="nt-steps">
      <Step n={1} done={Boolean(f)} title={t('notify.step_app')}>
        <p className="small muted">{t('notify.step_app_body')}</p>
        <a className="small" href={consoleUrl} target="_blank" rel="noopener noreferrer">
          {t('notify.open_console')} <ExternalLink size={12} aria-hidden="true" />
        </a>
        <p className="small muted">{t('notify.step_app_team')}</p>
      </Step>
      <Step n={2} done={Boolean(f)} title={t('notify.step_scopes')}>
        <p className="small muted">{t('notify.step_scopes_body')}</p>
        {SCOPES.map((s) => (
          <Code key={s} text={s} />
        ))}
      </Step>
      <Step n={3} done={Boolean(f)} title={t('notify.step_creds')}>
        <p className="small muted">{t('notify.step_creds_body')}</p>
        {!f && <CredsForm data={data} onSaved={saved} />}
      </Step>
      <Step n={4} done={Boolean(f?.callback_ok)} title={t('notify.step_events')}>
        <p className="small muted">{t('notify.step_events_body')}</p>
        <span className="small">{t('notify.step_events_event')}</span>
        {EVENTS.map((e) => (
          <Code key={e} text={e} />
        ))}
        <span className="small">{t('notify.step_events_callback')}</span>
        <Code text={CALLBACK} />
        <Note kind="warn">{t('notify.step_events_warn')}</Note>
      </Step>
      <Step n={5} done={Boolean(f?.bound)} title={t('notify.step_publish')}>
        <p className="small muted">{t('notify.step_publish_body')}</p>
      </Step>
      <Step n={6} done={Boolean(f?.bound)} title={t('notify.step_bind')}>
        {f && !f.bound ? (
          <>
            <p className="small muted">{t('notify.step_bind_body')}</p>
            <PairCode pair={data.pair} />
          </>
        ) : (
          <p className="small muted">{t(f?.bound ? 'notify.bound' : 'notify.step_bind_later')}</p>
        )}
      </Step>
    </ol>
  );

  // 没配：一个按钮，手动配置收起来
  if (!f) {
    return (
      <Card id="nt-feishu" title={t('notify.feishu_title')} sub={t('notify.feishu_lead')}>
        <ScanSetup data={data} reload={reload} />
        <More summary={t('notify.manual')}>{steps}</More>
      </Card>
    );
  }

  const menu = [
    { icon: Pencil, label: t('notify.edit_creds'), onClick: () => setEditing(true) },
    f.bound && { icon: KeyRound, label: t('notify.scan_update'), hint: t('notify.scan_update_hint'), onClick: grant },
    f.bound && { icon: Link2Off, label: t('notify.unbind'), onClick: unbind },
    'sep',
    { icon: Power, label: t('notify.disconnect'), onClick: disconnect, danger: true },
  ];
  const label = !st.connected ? t('notify.disconnected') : f.bound ? t('notify.connected') : t('notify.connected_unbound');
  const hint = f.bound ? t('notify.bound_hint', { app: f.app_id }) : t('notify.unbound_hint', { app: f.app_id });
  return (
    <Card id="nt-feishu" title={t('notify.feishu_title')}>
      <Row
        label={
          <>
            <span className={`nt-dot ${st.connected ? 'ok' : 'bad'}`} aria-hidden="true" />
            {label}
          </>
        }
        hint={hint}
      >
        {f.bound && st.connected && (
          <Button variant="secondary" className="btn-sm" busy={busy} onClick={test}>
            {t('notify.test')}
          </Button>
        )}
        <PopMenu items={menu} label={t('action.more')} className="btn btn-plain btn-icon" />
      </Row>
      <div className="stack" style={{ gap: 8 }}>
        {st.error && <FeishuError error={{ code: 'NOTIFY_FEISHU', params: st.error }} />}
        {/* 一次只一条：连不上时只说连不上 */}
        {f.bound && st.connected && !st.error && !f.callback_ok && <Note>{t(tested ? 'notify.test_sent' : 'notify.callback_pending')}</Note>}
        {editing && <CredsForm data={data} onSaved={saved} onCancel={() => setEditing(false)} />}
        <ScanSetup data={data} reload={reload} update />
        <ErrorNote error={error && error.code !== 'NOTIFY_FEISHU' ? error : null} />
        {error?.code === 'NOTIFY_FEISHU' && <FeishuError error={error} />}
        {/* 手动连上了还没绑定：展开步骤，显示绑定码 */}
        {!f.bound && (
          <More summary={t('notify.manual_rest')} open>
            {steps}
          </More>
        )}
      </div>
    </Card>
  );
}

/** 加一个渠道，或者改已有的（改的时候地址留空 = 不变） */
function HookForm({ hook, onSaved, onCancel }) {
  const { t } = useTranslation();
  const [kind, setKind] = useState(hook?.kind ?? 'wecom');
  const [target, setTarget] = useState('');
  const [secret, setSecret] = useState('');
  const [name, setName] = useState(hook?.name ?? '');
  const { busy, error, run } = useAction();
  const editing = Boolean(hook);
  const keep = editing && hook.kind === kind;
  const save = async (e) => {
    e.preventDefault();
    const body = {
      id: hook?.id,
      kind,
      name: name.trim(),
      target: target.trim() || undefined,
      secret: KINDS[kind].secret && (secret.trim() || !keep) ? secret.trim() : undefined,
    };
    const r = await run(() => api.put('/notify/hooks', body));
    if (r) onSaved(r);
  };
  return (
    <form className="stack" style={{ gap: 12 }} onSubmit={save}>
      <Field label={t('notify.hook_kind')}>
        <Select value={kind} onChange={(e) => setKind(e.target.value)}>
          {Object.keys(KINDS).map((k) => (
            <option key={k} value={k}>
              {t(`notify.hook_${k}`)}
            </option>
          ))}
        </Select>
      </Field>
      <p className="small muted" style={{ margin: 0 }}>
        {t(`notify.hook_${kind}_how`)}{' '}
        <a href={KINDS[kind].help} target="_blank" rel="noopener noreferrer">
          {t('notify.hook_help')} <ExternalLink size={12} aria-hidden="true" />
        </a>
      </p>
      <Field label={t(`notify.hook_${kind}_target`)} hint={keep ? t('notify.hook_keep') : undefined}>
        <PasswordInput value={target} onChange={(e) => setTarget(e.target.value)} placeholder={keep ? hook.target : t(`notify.hook_${kind}_ph`)} autoComplete="off" spellCheck={false} />
      </Field>
      {KINDS[kind].secret && (
        <Field label={t(`notify.hook_${kind}_secret`)} hint={keep && hook.has_secret ? t('notify.hook_keep') : t('notify.hook_optional')}>
          <PasswordInput value={secret} onChange={(e) => setSecret(e.target.value)} autoComplete="off" spellCheck={false} />
        </Field>
      )}
      <Field label={t('notify.hook_name')} hint={t('notify.hook_optional')}>
        <input value={name} onChange={(e) => setName(e.target.value)} maxLength={40} placeholder={t(`notify.hook_${kind}`)} />
      </Field>
      <ErrorNote error={error} />
      <div className="nt-actions">
        <Button type="submit" busy={busy} disabled={!keep && !target.trim()}>
          {t('action.save')}
        </Button>
        <Button variant="ghost" type="button" onClick={onCancel}>
          {t('action.cancel')}
        </Button>
      </div>
    </form>
  );
}

function HookRow({ hook, onChanged }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [tested, setTested] = useState(false);
  const { busy, error, run } = useAction();
  const st = hook.status ?? {};
  const test = async () => {
    setTested(false);
    if ((await run(() => api.post(`/notify/hooks/${hook.id}/test`))) !== undefined) setTested(true);
    onChanged();
  };
  const remove = async () => {
    if (await run(() => api.delete(`/notify/hooks/${hook.id}`))) onChanged();
  };
  if (editing) {
    return (
      <HookForm
        hook={hook}
        onSaved={() => {
          setEditing(false);
          onChanged();
        }}
        onCancel={() => setEditing(false)}
      />
    );
  }
  const menu = [
    { icon: Pencil, label: t('notify.edit'), onClick: () => setEditing(true) },
    { icon: Trash2, label: t('notify.hook_remove'), onClick: remove, danger: true },
  ];
  return (
    <>
      <Row
        label={
          <>
            <span className={`nt-dot ${st.error ? 'bad' : st.sent_at ? 'ok' : ''}`} aria-hidden="true" />
            {hook.name || t(`notify.hook_${hook.kind}`)}
          </>
        }
        hint={hook.target}
      >
        <Button variant="secondary" className="btn-sm" busy={busy} onClick={test}>
          {t('notify.test')}
        </Button>
        <PopMenu items={menu} label={t('action.more')} className="btn btn-plain btn-icon" />
      </Row>
      {tested && !error && <Note kind="ok">{t('notify.hook_test_ok')}</Note>}
      {st.error && !error && <Note kind="error">{t('notify.hook_error', { msg: st.error.msg })}</Note>}
      <ErrorNote error={error} />
    </>
  );
}

export function HooksCard({ data, reload }) {
  const { t } = useTranslation();
  const [adding, setAdding] = useState(false);
  const hooks = data?.hooks ?? [];
  if (!data) return null;
  return (
    <Card id="nt-hooks" title={t('notify.hooks_title')} sub={t('notify.hooks_lead')}>
      {hooks.map((h) => (
        <HookRow key={h.id} hook={h} onChanged={reload} />
      ))}
      <div className="stack" style={{ gap: 12, marginTop: hooks.length ? 8 : 0 }}>
        {adding ? (
          <>
            <p className="small muted" style={{ margin: 0 }}>
              {t('notify.hooks_privacy')}
            </p>
            <HookForm
              onSaved={() => {
                setAdding(false);
                reload();
              }}
              onCancel={() => setAdding(false)}
            />
          </>
        ) : (
          hooks.length < 10 && (
            <div className="nt-actions">
              <Button variant={hooks.length || data.feishu ? 'ghost' : 'primary'} onClick={() => setAdding(true)}>
                {t('notify.hook_add')}
              </Button>
            </div>
          )
        )}
      </div>
    </Card>
  );
}

const QUIET_DEFAULT = { start: '23:00', end: '08:00' };

export function RulesCard({ data, reload }) {
  const { t } = useTranslation();
  const sessions = useSessions();
  const { r, lang, save, busy, error } = useRules(data, reload);
  const [adding, setAdding] = useState('');
  // 卡片跟着界面语言：换了语言后下次打开这里时顺手更新
  useEffect(() => {
    if (r && r.lang !== lang && (data?.feishu || data?.hooks?.length)) save({});
  }, [lang, Boolean(r)]);
  const dirs = useMemo(() => [...new Set((sessions.items ?? []).map((i) => i.workdir).filter(Boolean))].sort(), [sessions.items]);
  if (!r) return null;
  const toggle = (k, title, hint) => <Toggle r={r} k={k} title={title} hint={hint} busy={busy} save={save} />;
  const name = (d) => d.split(/[\\/]/).filter(Boolean).pop() || d;
  const pick = (k, title, hint, options) => (
    <Row label={title} hint={hint}>
      <Select value={r[k] ?? 0} disabled={busy} onChange={(e) => save({ [k]: Number(e.target.value) })} aria-label={title} style={{ width: 'auto', minWidth: 120 }}>
        {options.map(([v, label]) => (
          <option key={v} value={v}>
            {label}
          </option>
        ))}
      </Select>
    </Row>
  );
  const quiet = r.quiet;
  const setQuiet = (patch) => {
    const q = { ...QUIET_DEFAULT, ...quiet, ...patch };
    if (/^\d{2}:\d{2}$/.test(q.start) && /^\d{2}:\d{2}$/.test(q.end) && q.start !== q.end) save({ quiet: q });
  };
  // 改过“少打扰”里的哪一项就默认展开
  const calmSet = Boolean(quiet) || r.done_batch > 0 || r.stall_min !== 20 || r.muted.length > 0;
  return (
    <Card title={t('notify.rules_title')} sub={t('notify.rules_lead')}>
      {toggle('done', t('notify.rule_done'), t('notify.rule_done_hint'))}
      {toggle('failed', t('notify.rule_failed'))}
      {toggle('fg_quiet', t('notify.rule_fg_quiet'), t('notify.rule_fg_quiet_hint'))}
      {data.feishu && toggle('actions', t('notify.rule_actions'), t('notify.rule_actions_hint'))}
      {toggle('content', t('notify.rule_content'), t('notify.rule_content_hint'))}
      <More summary={t('notify.calm_title')} open={calmSet}>
        <Row label={t('notify.quiet')} hint={t('notify.quiet_hint')}>
          <Switch checked={Boolean(quiet)} disabled={busy} onChange={(v) => (v ? setQuiet({}) : save({ quiet: null }))} label={t('notify.quiet')} />
        </Row>
        {quiet && (
          <div className="nt-actions nt-quiet" style={{ marginBottom: 8 }}>
            <input type="time" value={quiet.start} disabled={busy} aria-label={t('notify.quiet_start')} onChange={(e) => setQuiet({ start: e.target.value })} />
            <span className="muted">–</span>
            <input type="time" value={quiet.end} disabled={busy} aria-label={t('notify.quiet_end')} onChange={(e) => setQuiet({ end: e.target.value })} />
          </div>
        )}
        {pick('done_batch', t('notify.batch'), t('notify.batch_hint'), [
          [0, t('notify.batch_off')],
          [10, t('notify.every_min', { n: 10 })],
          [30, t('notify.every_min', { n: 30 })],
          [60, t('notify.every_hour')],
        ])}
        {pick('stall_min', t('notify.stall'), t('notify.stall_hint'), [
          [0, t('notify.stall_off')],
          [10, t('notify.after_min', { n: 10 })],
          [20, t('notify.after_min', { n: 20 })],
          [30, t('notify.after_min', { n: 30 })],
          [60, t('notify.after_min', { n: 60 })],
        ])}
        <Row label={t('notify.muted')} hint={t('notify.muted_hint')} stack>
          {r.muted.length > 0 && (
            <div className="nt-chips">
              {r.muted.map((m) => (
                <span key={m} className="nt-chip" title={m}>
                  {name(m)}
                  <button type="button" aria-label={t('notify.unmute', { name: name(m) })} disabled={busy} onClick={() => save({ muted: r.muted.filter((x) => x !== m) })}>
                    ×
                  </button>
                </span>
              ))}
            </div>
          )}
          <div className="nt-actions nt-mute-add">
            <Select value={adding} onChange={(e) => setAdding(e.target.value)} aria-label={t('notify.muted')} style={{ minWidth: 0, flex: 1 }}>
              <option value="">{t('notify.mute_pick')}</option>
              {dirs
                .filter((d) => !r.muted.includes(d))
                .map((d) => (
                  <option key={d} value={d}>
                    {name(d)} · {d}
                  </option>
                ))}
            </Select>
            <Button
              variant="ghost"
              disabled={!adding || busy}
              onClick={async () => {
                await save({ muted: [...r.muted, adding] });
                setAdding('');
              }}
            >
              {t('notify.mute')}
            </Button>
          </div>
        </Row>
      </More>
      <ErrorNote error={error} />
    </Card>
  );
}

const HOW = ['dm', 'group', 'reply', 'new', 'stop', 'forward', 'help'];

/** 飞书机器人：在飞书里和 AI 对话（docs/FEISHU_AGENT.md）。绑定好飞书以后才有 */
export function AgentCard({ data, reload }) {
  const { t } = useTranslation();
  const { r, save, busy, error } = useRules(data, reload);
  if (!r || !data.feishu?.bound) return null;
  const toggle = (k, title, hint) => <Toggle r={r} k={k} title={title} hint={hint} busy={busy} save={save} />;
  // 和 AI 对话要能在飞书里处理（审批）：打开时一起打开
  const on = r.agent && r.actions;
  return (
    <Card id="nt-agent" title={t('notify.agent_title')} sub={on ? undefined : t('notify.agent_lead')}>
      <Toggle r={r} title={t('notify.rule_agent')} hint={t('notify.rule_agent_hint')} busy={busy} checked={on} onChange={(v) => save(v ? { agent: true, actions: true } : { agent: false })} />
      {on && (
        <>
          {toggle('auto_groups', t('notify.rule_auto_groups'), t('notify.rule_auto_groups_hint'))}
          {toggle('agent_groups', t('notify.rule_agent_groups'), t('notify.rule_agent_groups_hint'))}
          {/* 打开了“回答直接发在群里”（有风险）时露在外面，否则收进“怎么用和更多选项” */}
          {r.agent_groups && r.agent_group_public && (
            <>
              {toggle('agent_group_public', t('notify.rule_agent_group_public'), t('notify.rule_agent_group_public_hint'))}
              <Note kind="warn">{t('notify.agent_group_public_warn')}</Note>
            </>
          )}
          <More summary={t('notify.agent_how_title')}>
            {r.agent_groups && !r.agent_group_public && toggle('agent_group_public', t('notify.rule_agent_group_public'), t('notify.rule_agent_group_public_hint'))}
            <ul className="nt-intro">
              {HOW.map((k) => (
                <li key={k}>{t(`notify.agent_how_${k}`)}</li>
              ))}
            </ul>
            <p className="small muted">{t('notify.rule_agent_note')}</p>
            <div className="stack" style={{ gap: 6 }}>
              <b className="small">{t('notify.agent_menu_title')}</b>
              <span className="small muted">{t('notify.agent_menu')}</span>
              {MENU_KEYS.map((k) => (
                <span key={k} className="small">
                  {t(`notify.agent_menu_${k}`)} <Code text={k} />
                </span>
              ))}
            </div>
          </More>
        </>
      )}
      <ErrorNote error={error} />
    </Card>
  );
}

/** AI 页“远程与通知”：先连飞书，再是飞书机器人、什么时候通知，最后是只通知的渠道 */
export default function NotifySection() {
  const [data, error, reload] = useLoad(() => api.get('/notify'));
  const configured = data?.feishu || data?.hooks?.length > 0;
  return (
    <>
      <ErrorNote error={error} />
      <FeishuCard data={data} reload={reload} />
      <AgentCard data={data} reload={reload} />
      {configured && <RulesCard data={data} reload={reload} />}
      <HooksCard data={data} reload={reload} />
    </>
  );
}
