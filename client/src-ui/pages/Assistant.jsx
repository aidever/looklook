// 助理（docs/ASSISTANT.md，设计稿 docs/SINGLE_ENTRY_AGENT_DESIGN_2026-11-11.md 零期）：一个入口说话，助理自己判断是哪个项目、
// 新开会话还是接着原来的，把活派给 Claude Code / Codex；小事自己做。这一页 = 和助理的对话 + 它派出去在干的活 + 每次派到哪、为什么（去向条，能改）
// + 口头约定。助理是这台电脑上一个藏起来的对话模式 Claude 会话（不在会话列表里），对话直接用 ChatView。
import { ArrowRightLeft, Bot, Check, ChevronRight, ListChecks, Loader2, MessageCircleQuestion, PanelRight, Sparkles, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { api, rememberTerminals, selectedDevice, terminalDevice } from '../shared/api.js';
import { DeviceSwitcher, useDevices } from '../shared/devices.jsx';
import { errorText } from '../shared/i18n.js';
import { isTouch } from '../shared/KeyBar.jsx';
import { useRouter } from '../shared/router.jsx';
import Select from '../shared/Select.jsx';
import { AGENTS, baseName, sessionLaunch, sessionTitle, useSessions } from '../shared/sessions.jsx';
import { formatTime } from '../shared/time.js';
import { Button, Empty, Note, Skeleton, useToast } from '../shared/ui.jsx';
import ErrorBoundary from '../shared/ErrorBoundary.jsx';
import ChatView from './ChatView.jsx';

const POLL_MS = 4000;
// 最近一次派活（这么久以内、没改过的）在对话上方单独显示一条，方便马上改
const FRESH_MS = 15 * 60 * 1000;

function useAssistant() {
  const [state, setState] = useState(null);
  const [error, setError] = useState(null);
  const load = useCallback(
    () =>
      api.get('/assistant').then(
        (d) => {
          const dev = selectedDevice();
          // 助理和它派的会话都在这台电脑上：记下来，对话接口才会发到这台
          if (dev) rememberTerminals(dev, [d.brain, ...d.tasks.map((x) => x.session), ...d.dispatches.map((x) => x.session)].filter(Boolean).map((id) => ({ id })));
          setState(d);
          setError(null);
        },
        (e) => setError(e),
      ),
    [],
  );
  useEffect(() => {
    load();
    const id = setInterval(() => document.visibilityState === 'visible' && load(), POLL_MS);
    return () => clearInterval(id);
  }, [load]);
  return { state, error, load };
}

/**
 * 手机上软键盘弹着：iOS Safari / 新版 Android Chrome 只缩小可见区域，底部的输入框会被键盘盖住。
 * 键盘弹着时整页贴合可见区域（盖住顶栏和底部标签，收起后复原）。返回 style（没弹键盘时 undefined）。
 */
function useKeyboardFit(enabled) {
  const [fit, setFit] = useState(null);
  useEffect(() => {
    const vv = window.visualViewport;
    if (!enabled || !vv) return undefined;
    let full = 0;
    let w = 0;
    const on = () => {
      if (window.innerWidth !== w) {
        w = window.innerWidth;
        full = 0;
      }
      full = Math.max(full, vv.height, document.documentElement.clientHeight);
      setFit(vv.height < full - 120 ? { top: vv.offsetTop, height: vv.height } : null);
    };
    on();
    vv.addEventListener('resize', on);
    vv.addEventListener('scroll', on);
    return () => {
      vv.removeEventListener('resize', on);
      vv.removeEventListener('scroll', on);
    };
  }, [enabled]);
  return fit ? { position: 'fixed', left: 0, right: 0, top: fit.top, height: fit.height, zIndex: 45, background: 'var(--bg)' } : undefined;
}

export default function Assistant() {
  const { t } = useTranslation();
  const { state, error, load } = useAssistant();
  const [side, setSide] = useState(false);
  const [touch] = useState(isTouch);
  const devices = useDevices();
  const fit = useKeyboardFit(touch);
  if (!state) {
    return (
      <div className="asst asst-pad">
        {error ? <Note kind="error">{errorText(t, error)}</Note> : <Skeleton height={200} />}
      </div>
    );
  }
  if (!state.brain) return <Enable onDone={load} />;
  const waiting = state.tasks.filter((x) => x.state === 'waiting_for_user').length;
  return (
    <div className="asst" style={fit}>
      <div className="asst-main">
        <div className="asst-strip">
          {/* 每台电脑一个助理；多台时这里换（手机上在顶栏） */}
          {(devices.multi || devices.localOnly) && (
            <span className="asst-dev">
              <DeviceSwitcher perDevice />
            </span>
          )}
          <TaskChips tasks={state.tasks} onChange={load} />
          <button type="button" className="btn btn-plain btn-sm asst-side-btn" onClick={() => setSide(!side)} aria-pressed={side} aria-controls="asst-side">
            <PanelRight size={15} aria-hidden="true" />
            <span>{t('asst.side')}</span>
            {waiting > 0 && <span className="rail-count asst-count">{waiting}</span>}
          </button>
        </div>
        <FreshRoutes list={state.dispatches} onChange={load} />
        <div className="asst-chat">
          <ErrorBoundary>
            <ChatView id={state.brain} launch="claude" active touch={touch} label={t('asst.title')} />
          </ErrorBoundary>
        </div>
      </div>
      <aside id="asst-side" className="asst-side" data-open={side ? '' : undefined} aria-label={t('asst.side')}>
        <div className="asst-side-head">
          <strong>{t('asst.side')}</strong>
          <button type="button" className="btn btn-plain btn-icon asst-side-close" onClick={() => setSide(false)} aria-label={t('action.close')}>
            <X size={18} aria-hidden="true" />
          </button>
        </div>
        <SidePanel state={state} onChange={load} />
      </aside>
    </div>
  );
}

/** 还没开：说清楚它是什么、怎么用，点一下建好（建一个藏起来的对话模式 Claude 会话） */
function Enable({ onDone }) {
  const { t } = useTranslation();
  const devices = useDevices();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const go = () => {
    setBusy(true);
    api
      .post('/assistant/enable', {})
      .then(onDone, (e) => toast(e?.code === 'ASSISTANT_UNAVAILABLE' ? t('asst.unavailable') : errorText(t, e), 'error'))
      .finally(() => setBusy(false));
  };
  return (
    <div className="asst asst-pad">
      {(devices.multi || devices.localOnly) && (
        <span className="asst-dev">
          <DeviceSwitcher perDevice />
        </span>
      )}
      <Empty icon={Sparkles} title={t('asst.intro_title')}>
        <ul className="asst-intro">
          <li>{t('asst.intro_1')}</li>
          <li>{t('asst.intro_2')}</li>
          <li>{t('asst.intro_3')}</li>
          <li>{t('asst.intro_4')}</li>
        </ul>
        <Button onClick={go} busy={busy} icon={Sparkles}>
          {t('asst.enable')}
        </Button>
        <p className="muted asst-note">{t('asst.enable_note')}</p>
      </Empty>
    </div>
  );
}

const STATE_ICON = { working: Loader2, waiting_for_user: MessageCircleQuestion, turn_finished: Check };

/** 对话上方一排：它派出去、还盯着的活。点了打开那个会话 */
function TaskChips({ tasks, onChange }) {
  const { t } = useTranslation();
  const { navigate } = useRouter();
  if (tasks.length === 0) return <span className="muted asst-strip-empty">{t('asst.no_tasks')}</span>;
  // 等你的放最前面
  const order = { waiting_for_user: 0, working: 1 };
  const list = [...tasks].sort((a, b) => (order[a.state] ?? 2) - (order[b.state] ?? 2));
  const drop = (e, id) => {
    e.stopPropagation();
    api.del(`/assistant/watch/${id}`).then(onChange, () => {});
  };
  return (
    <div className="asst-chips">
      {list.map((x) => {
        const Icon = STATE_ICON[x.state] ?? Bot;
        return (
          <span key={x.session} className="asst-chip" data-state={x.state}>
            <button type="button" className="asst-chip-main" onClick={() => navigate(`/s/${x.session}`)} title={`${x.title} · ${x.project}${x.doing ? `\n${x.doing}` : ''}`}>
              <Icon size={13} aria-hidden="true" className={x.state === 'working' ? 'spin' : undefined} />
              <span className="ellipsis">{x.title}</span>
              <span className="asst-chip-state">{t(`asst.state.${x.state}`, { defaultValue: x.state })}</span>
            </button>
            {x.state !== 'working' && x.state !== 'waiting_for_user' && (
              <button type="button" className="asst-chip-x" onClick={(e) => drop(e, x.session)} aria-label={t('asst.unwatch')} title={t('asst.unwatch')}>
                <X size={12} aria-hidden="true" />
              </button>
            )}
          </span>
        );
      })}
    </div>
  );
}

/** 刚派出去的那几次：对话上方各一条去向条，写去了哪、为什么，能马上改 */
function FreshRoutes({ list, onChange }) {
  const [now, setNow] = useState(Date.now);
  const [hidden, setHidden] = useState(() => new Set());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(id);
  }, []);
  const fresh = list.filter((d) => !d.fixed && now - d.at < FRESH_MS && !hidden.has(d.id)).slice(0, 1);
  if (fresh.length === 0) return null;
  return (
    <div className="asst-fresh">
      {fresh.map((d) => (
        <Route key={d.id} d={d} onChange={onChange} onHide={() => setHidden(new Set([...hidden, d.id]))} compact />
      ))}
    </div>
  );
}

/** 一条去向：去了哪个会话（新开 / 接着）、哪个项目、为什么；“不对”可以改成新开或者改发给另一个会话 */
function Route({ d, onChange, onHide, compact = false }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { navigate } = useRouter();
  const sessions = useSessions();
  const [fixing, setFixing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [target, setTarget] = useState('');
  const dev = selectedDevice();
  const choices = useMemo(
    () =>
      (sessions.items ?? [])
        .filter((i) => i.id !== d.session && AGENTS.includes(sessionLaunch(i)) && (!sessions.multi || terminalDevice(i.id) === dev))
        .slice(0, 60)
        .map((i) => [i.id, `${sessionTitle(i)} · ${baseName(i.workdir)}`]),
    [sessions.items, sessions.multi, d.session, dev],
  );
  const fix = (mode) => {
    setBusy(true);
    api
      .post(`/assistant/dispatches/${d.id}/fix`, mode === 'into' ? { mode, session: target } : { mode })
      .then(
        () => {
          setFixing(false);
          toast(t('asst.fixed'), 'success');
          onChange();
        },
        (e) => toast(errorText(t, e), 'error'),
      )
      .finally(() => setBusy(false));
  };
  const queued = !d.session && d.queued_after;
  const where = d.kind === 'start' ? (queued ? 'asst.route.queued' : 'asst.route.new') : 'asst.route.into';
  return (
    <div className="asst-route" data-compact={compact ? '' : undefined} data-fixed={d.fixed ? '' : undefined}>
      <div className="asst-route-line">
        <ArrowRightLeft size={14} aria-hidden="true" className="asst-route-ico" />
        <span className="asst-route-text">
          {t(where)}{' '}
          {d.session ? (
            <button type="button" className="link-btn asst-link" onClick={() => navigate(`/s/${d.session}`)}>
              {d.title}
            </button>
          ) : (
            <strong>{d.title}</strong>
          )}
          <span className="muted"> · {baseName(d.project) || d.project}</span>
          {!compact && <span className="muted"> · {formatTime(d.at)}</span>}
        </span>
        {!d.fixed && !fixing && (
          <button type="button" className="btn btn-plain btn-sm" onClick={() => setFixing(true)}>
            {t('asst.route.wrong')}
          </button>
        )}
        {onHide && !fixing && (
          <button type="button" className="btn btn-plain btn-icon btn-sm" onClick={onHide} aria-label={t('asst.route.ok')} title={t('asst.route.ok')}>
            <Check size={15} aria-hidden="true" />
          </button>
        )}
      </div>
      {d.reason && <p className="asst-route-why">{t('asst.route.why', { reason: d.reason })}</p>}
      {d.fixed && <p className="asst-route-why">{t(d.fixed.mode === 'new' ? 'asst.route.fixed_new' : 'asst.route.fixed_into')}</p>}
      {fixing && (
        <div className="asst-route-fix">
          {d.kind === 'send' && (
            <Button className="btn-sm" variant="secondary" busy={busy} onClick={() => fix('new')}>
              {t('asst.route.to_new')}
            </Button>
          )}
          <span className="asst-route-into">
            <Select value={target} onChange={setTarget} options={choices} placeholder={t('asst.route.pick')} aria-label={t('asst.route.pick')} />
            <Button className="btn-sm" variant="secondary" busy={busy} disabled={!target} onClick={() => fix('into')}>
              {t('asst.route.to_this')}
            </Button>
          </span>
          <button type="button" className="btn btn-plain btn-sm" onClick={() => setFixing(false)}>
            {t('action.cancel')}
          </button>
          <p className="muted asst-route-why">{t(d.kind === 'start' ? 'asst.route.fix_note_start' : 'asst.route.fix_note_send')}</p>
        </div>
      )}
    </div>
  );
}

/** 右边（手机上是盖上来的一层）：分派记录、口头约定、改正次数 */
function SidePanel({ state, onChange }) {
  const { t } = useTranslation();
  const toast = useToast();
  const drop = (id) => api.post('/assistant/agreements/remove', { id }).then(onChange, (e) => toast(errorText(t, e), 'error'));
  const { fixed, of } = state.stats;
  return (
    <div className="asst-side-body">
      <section>
        <h3 className="asst-h">
          <ListChecks size={15} aria-hidden="true" />
          {t('asst.agreements')}
        </h3>
        {state.agreements.length === 0 ? (
          <p className="muted asst-small">{t('asst.agreements_empty')}</p>
        ) : (
          <ul className="asst-agree">
            {state.agreements.map((a) => (
              <li key={a.id}>
                <span>
                  {a.text}
                  {a.until && <span className="muted"> · {t('asst.until', { until: a.until })}</span>}
                </span>
                <button type="button" className="btn btn-plain btn-icon btn-sm" onClick={() => drop(a.id)} aria-label={t('asst.agreement_done')} title={t('asst.agreement_done')}>
                  <X size={14} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section>
        <h3 className="asst-h">
          <ArrowRightLeft size={15} aria-hidden="true" />
          {t('asst.routes')}
        </h3>
        {of > 0 && <p className="muted asst-small">{t('asst.stats', { of, fixed })}</p>}
        {state.dispatches.length === 0 ? (
          <p className="muted asst-small">{t('asst.routes_empty')}</p>
        ) : (
          <div className="asst-routes">
            {state.dispatches.slice(0, 50).map((d) => (
              <Route key={d.id} d={d} onChange={onChange} />
            ))}
          </div>
        )}
      </section>
      <p className="muted asst-small asst-foot">
        <ChevronRight size={12} aria-hidden="true" />
        {t('asst.foot', { model: state.default_model })}
      </p>
    </div>
  );
}
