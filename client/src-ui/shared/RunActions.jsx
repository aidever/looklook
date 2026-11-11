// 项目的运行按钮（▶）：照 Codex 桌面版顶栏的“操作”按钮做。
// 每个项目可以有几条运行命令（npm run dev、cargo test…），点 ▶ 在这个项目里一个专门的终端会话（“▶ 名字”）里跑：
// 看得到在不在跑、能停、能重跑、能打开终端看完整输出。命令来自看看自己记的，以及项目里 Codex 的
// .codex/environments/*.toml（只读）。没配时按项目文件（package.json 的 scripts 等）列几条建议，点“添加”才记下，不会自己跑。
// 会话页顶栏（TerminalView.jsx 的 TermPane / 手机整页）和项目页（Project.jsx）各放一个按钮，点开是同一个面板。
import { Bug, ChevronDown, FlaskConical, Hammer, Pencil, Play, Plus, RotateCcw, Square, SquareTerminal, Trash2, Wrench } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useSessionUi } from '../pages/Sessions.jsx';
import { deviceApi, rememberTerminals } from './api.js';
import { errorText } from './i18n.js';
import { useRouter } from './router.jsx';
import { useSessions } from './sessions.jsx';
import { Button, Modal, Spinner, useToast } from './ui.jsx';

const enc = encodeURIComponent;

/** 命令现在的样子：running 在跑；done 跑完了（终端还开着，输出还在）；idle 没跑过或会话已停。 */
export function runState(a) {
  const i = a?.inst;
  if (!i || !i.task_alive) return 'idle';
  // 看不出有没有子进程（Windows）时看前台程序是不是 shell
  const jobs = i.jobs ?? i.busy;
  return jobs === false ? 'done' : 'running';
}

const ICONS = { test: FlaskConical, debug: Bug, build: Hammer, tool: Wrench, tools: Wrench };
const iconOf = (a) => ICONS[(a.icon || '').toLowerCase()] ?? Play;

/** 取这个项目的运行命令；`every` 毫秒取一次（0：只取一次）。 */
function useRunData(device, workdir, every, enabled = true) {
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const alive = useRef(true);
  const load = () =>
    deviceApi(device)
      .get(`/run-actions?workdir=${enc(workdir)}`)
      .then(
        (r) => {
          if (!alive.current) return;
          setData(r);
          setError(null);
        },
        (e) => alive.current && setError(e),
      );
  useEffect(() => {
    alive.current = true;
    if (!enabled || !workdir) return undefined;
    load();
    if (!every) return () => (alive.current = false);
    const iv = setInterval(() => document.visibilityState === 'visible' && load(), every);
    return () => {
      alive.current = false;
      clearInterval(iv);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [device, workdir, every, enabled]);
  return { data, error, reload: load, setData };
}

/**
 * ▶ 按钮：点开运行面板。有命令在跑时右上角一个绿点。`text` 时带文字（项目页），否则只有图标（会话顶栏）。
 * `active`：所在的会话正显示着（看不见的不去轮询）。
 */
export function RunButton({ device = null, workdir, active = true, text = false, size = 16, className }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  // 只为了绿点：15 秒看一次；面板开着时面板自己取
  const { data } = useRunData(device, workdir, 15000, active && !open && Boolean(workdir));
  const running = (data?.actions ?? []).filter((a) => runState(a) === 'running');
  const label = running.length ? t('run.button_running', { count: running.length }) : t('run.button');
  if (!workdir) return null;
  return (
    <>
      <button
        type="button"
        className={className ?? (text ? 'btn btn-ghost run-btn' : 'btn btn-plain btn-icon run-btn')}
        onClick={() => setOpen(true)}
        aria-label={label}
        title={label}
        aria-haspopup="dialog"
      >
        <Play size={size} aria-hidden="true" />
        {text && t('run.title')}
        {running.length > 0 && <span className="run-dot" aria-hidden="true" />}
      </button>
      {open && <RunPanel device={device} workdir={workdir} onClose={() => setOpen(false)} />}
    </>
  );
}

/** 运行面板（手机上是底部弹出的一张卡）：命令列表，或者编辑命令。 */
export function RunPanel({ device = null, workdir, onClose }) {
  const { t } = useTranslation();
  const { data, error, reload, setData } = useRunData(device, workdir, 2500);
  const [editing, setEditing] = useState(false);
  const actions = data?.actions ?? [];
  const mine = actions.filter((a) => a.source === 'looklook');
  const empty = data && actions.length === 0;
  const name = workdir.split(/[\\/]/).filter(Boolean).pop() || workdir;
  return (
    <Modal open onClose={onClose} title={t('run.panel_title', { name })} className="run-modal" wide>
      {!data && !error && (
        <div className="run-loading">
          <Spinner />
        </div>
      )}
      {error && !data && <p className="field-error">{errorText(t, error)}</p>}
      {data && (editing || empty) ? (
        <RunEditor
          device={device}
          workdir={workdir}
          mine={mine}
          suggestions={data.suggestions ?? []}
          codex={actions.filter((a) => a.source === 'codex').length}
          onCancel={empty ? onClose : () => setEditing(false)}
          cancelLabel={empty ? t('action.close') : t('action.cancel')}
          onSaved={(list) => {
            setData({ ...data, actions: [...list.map((a) => ({ ...a, source: 'looklook', inst: actions.find((x) => x.key === a.key)?.inst ?? null })), ...actions.filter((a) => a.source === 'codex')] });
            setEditing(false);
            reload();
          }}
        />
      ) : (
        data && (
          <>
            <p className="small muted run-where">{t('run.where')}</p>
            <ul className="run-list">
              {actions.map((a) => (
                <RunRow key={a.key} a={a} device={device} workdir={workdir} onChanged={reload} onOpen={onClose} />
              ))}
            </ul>
            {data.codex_files?.length > 0 && <p className="small muted run-codex-note">{t('run.codex_note', { file: data.codex_files.map((f) => f.split(/[\\/]/).slice(-3).join('/')).join('、') })}</p>}
            <div className="modal-actions">
              <Button variant="ghost" icon={Pencil} onClick={() => setEditing(true)}>
                {t(mine.length ? 'run.edit' : 'run.add_own')}
              </Button>
              <Button variant="ghost" onClick={onClose}>
                {t('action.close')}
              </Button>
            </div>
          </>
        )
      )}
    </Modal>
  );
}

/** 一条命令：名字、命令、状态；运行 / 停止 / 重新运行 / 打开终端；最近的输出。 */
function RunRow({ a, device, workdir, onChanged, onOpen }) {
  const { t } = useTranslation();
  const toast = useToast();
  const ui = useSessionUi();
  const s = useSessions();
  const { navigate } = useRouter();
  const [busy, setBusy] = useState(null);
  const [showOut, setShowOut] = useState(false);
  const state = runState(a);
  const Icon = iconOf(a);
  const client = deviceApi(device);
  const run = async () => {
    setBusy('run');
    try {
      const r = await client.post('/run-actions/run', { workdir, key: a.key });
      if (device && r?.id) rememberTerminals(device, [{ id: r.id }]);
      setShowOut(true);
      toast(t('run.started', { name: a.name }), 'ok');
      s?.reload?.();
    } catch (e) {
      toast(errorText(t, e), 'error');
    } finally {
      setBusy(null);
      onChanged();
    }
  };
  const stop = async () => {
    setBusy('stop');
    try {
      await client.post('/run-actions/stop', { workdir, key: a.key });
      toast(t('run.stopped', { name: a.name }));
      s?.reload?.();
    } catch (e) {
      toast(errorText(t, e), 'error');
    } finally {
      setBusy(null);
      onChanged();
    }
  };
  const openTerm = async () => {
    const id = a.inst?.id;
    if (!id) return;
    if (device) rememberTerminals(device, [{ id }]);
    onOpen();
    if (ui) {
      // 先刷新会话列表：新建的运行会话还不在列表里时，标签会先显示“会话不在了”
      await s?.reload?.();
      ui.openSession(id);
    } else navigate(`/t/${id}`);
  };
  const badge = { running: 'badge-brand', done: 'badge-ok' }[state];
  return (
    <li className={`run-row run-${state}`}>
      <div className="run-head">
        <span className="run-ico" aria-hidden="true">
          <Icon size={16} />
        </span>
        <span className="run-name ellipsis">{a.name}</span>
        {a.source === 'codex' && <span className="badge run-src">Codex</span>}
        <span className="grow" />
        {badge && <span className={`badge ${badge}`}>{t(`run.state_${state}`)}</span>}
      </div>
      <code className="run-cmd" title={a.command}>
        {a.command}
      </code>
      <div className="run-acts">
        {state === 'running' ? (
          <>
            <Button variant="ghost" icon={Square} busy={busy === 'stop'} disabled={Boolean(busy)} onClick={stop}>
              {t('run.stop')}
            </Button>
            <Button variant="ghost" icon={RotateCcw} busy={busy === 'run'} disabled={Boolean(busy)} onClick={run}>
              {t('run.rerun')}
            </Button>
          </>
        ) : (
          <Button variant="primary" icon={Play} busy={busy === 'run'} disabled={Boolean(busy)} onClick={run}>
            {t(state === 'done' ? 'run.rerun' : 'run.run')}
          </Button>
        )}
        {a.inst?.task_alive && (
          <Button variant="plain" icon={SquareTerminal} onClick={openTerm}>
            {t('run.open')}
          </Button>
        )}
        {a.inst?.task_alive && (
          <button type="button" className="btn btn-plain run-out-toggle" aria-expanded={showOut} onClick={() => setShowOut(!showOut)}>
            <ChevronDown size={15} className="prow-chev" aria-hidden="true" />
            {t('run.output')}
          </button>
        )}
      </div>
      {showOut && a.inst?.task_alive && <RunOutput device={device} id={a.inst.id} live={state === 'running'} />}
    </li>
  );
}

/** 终端里最近的输出（最后十几行），在跑时两秒刷新一次。完整的在终端里看。 */
function RunOutput({ device, id, live }) {
  const { t } = useTranslation();
  const [text, setText] = useState(null);
  const box = useRef(null);
  useEffect(() => {
    let alive = true;
    const load = () =>
      deviceApi(device)
        .get(`/instances/${id}/logs`)
        .then(
          (r) => alive && setText((r?.output ?? '').split('\n').slice(-40).join('\n')),
          () => alive && setText(''),
        );
    load();
    const iv = live ? setInterval(load, 2000) : null;
    return () => {
      alive = false;
      if (iv) clearInterval(iv);
    };
  }, [device, id, live]);
  useEffect(() => {
    if (box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [text]);
  return (
    <pre className="run-out" ref={box} aria-live="off">
      {text === null ? <Spinner size={14} /> : text.trim() ? text : t('run.no_output')}
    </pre>
  );
}

/** 编辑看看自己的命令；下面是按项目文件猜的建议（点了只是加进列表，保存后点 ▶ 才运行）。 */
function RunEditor({ device, workdir, mine, suggestions, codex, onCancel, cancelLabel, onSaved }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [list, setList] = useState(() => mine.map(({ key, name, command, icon }) => ({ key, name, command, icon: icon ?? '' })));
  const [saving, setSaving] = useState(false);
  const first = useRef(null);
  const commands = new Set(list.map((a) => a.command.trim()));
  const left = suggestions.filter((x) => !commands.has(x.command));
  const set = (i, patch) => setList(list.map((a, j) => (j === i ? { ...a, ...patch } : a)));
  const add = (a = { name: '', command: '' }) => setList([...list, { key: '', icon: '', ...a }]);
  const ready = list.every((a) => a.name.trim() && a.command.trim());
  const dirty = JSON.stringify(list) !== JSON.stringify(mine.map(({ key, name, command, icon }) => ({ key, name, command, icon: icon ?? '' })));
  const save = async () => {
    setSaving(true);
    try {
      const r = await deviceApi(device).put('/run-actions', { workdir, actions: list });
      toast(t('run.saved'), 'ok');
      onSaved(r?.actions ?? []);
    } catch (e) {
      toast(errorText(t, e), 'error');
    } finally {
      setSaving(false);
    }
  };
  useEffect(() => {
    if (list.length && !list[list.length - 1].command) first.current?.focus();
  }, [list.length]);
  return (
    <div className="run-edit">
      {list.length === 0 && (
        <>
          <p className="run-empty-title">{t('run.empty_title')}</p>
          <p className="small muted">{t('run.empty_body')}</p>
        </>
      )}
      {list.length > 0 && (
        <ul className="run-edit-list">
          {list.map((a, i) => (
            <li key={i} className="run-edit-row">
              <div className="run-edit-line">
                <input
                  ref={i === list.length - 1 ? first : undefined}
                  className="run-edit-name"
                  value={a.name}
                  maxLength={30}
                  onChange={(e) => set(i, { name: e.target.value })}
                  placeholder={t('run.name_ph')}
                  aria-label={t('run.name')}
                />
                <button type="button" className="btn btn-plain btn-icon" onClick={() => setList(list.filter((_, j) => j !== i))} aria-label={t('action.delete')} title={t('action.delete')}>
                  <Trash2 size={16} />
                </button>
              </div>
              <textarea
                className="run-edit-cmd mono"
                value={a.command}
                rows={Math.min(6, Math.max(1, a.command.split('\n').length))}
                onChange={(e) => set(i, { command: e.target.value })}
                placeholder={t('run.command_ph')}
                aria-label={t('run.command')}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
              />
            </li>
          ))}
        </ul>
      )}
      <p className="small muted">{t('run.command_hint')}</p>
      <Button variant="ghost" icon={Plus} onClick={() => add()}>
        {t('run.add_own')}
      </Button>
      {left.length > 0 && (
        <section className="run-suggest" aria-label={t('run.suggest_title')}>
          <h3 className="small">{t('run.suggest_title')}</h3>
          <p className="small muted">{t('run.suggest_hint')}</p>
          <ul>
            {left.map((x) => (
              <li key={x.command}>
                <button type="button" className="run-chip" onClick={() => add({ name: x.name, command: x.command })} title={t('run.from', { file: x.from })}>
                  <Plus size={14} aria-hidden="true" />
                  <code>{x.command}</code>
                  <span className="muted small">{x.from}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
      {codex > 0 && <p className="small muted">{t('run.codex_count', { count: codex })}</p>}
      <div className="modal-actions">
        <Button busy={saving} disabled={!ready || !dirty} onClick={save}>
          {t('action.save')}
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          {cancelLabel}
        </Button>
      </div>
    </div>
  );
}
