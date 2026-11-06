// 设置 → 宠物（docs/PET_PACK.md §6.3）：给每种对话配一只宠物、大小、气泡；列出找得到的宠物包，导入、试播、删除。
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Download, FolderOpen, Github, Heart, MessageCircle, Monitor, Play, RefreshCw, Trash2, Upload } from 'lucide-react';
import { api, postFile } from '../shared/api.js';
import { deviceName } from '../shared/devices.jsx';
import { errorText } from '../shared/i18n.js';
import { PETS_CHANGED, inDesktopMain, usePetsLive } from '../shared/ChatPet.jsx';
import { levelOf } from '../shared/pet/bond.js';
import PetPlayer from '../shared/pet/PetPlayer.jsx';
import { loadBlob, videoFormat } from '../shared/pet/media.js';
import { textOf } from '../shared/pet/brain.js';
import { Button, Card, CopyButton, ErrorNote, Field, Modal, Seg, Spinner, useConfirm, useLoad, useToast } from '../shared/ui.jsx';
import { FolderPicker } from './Terminals.jsx';
import Select from '../shared/Select.jsx';

const GUIDE_URL = 'https://github.com/aidever/looklook/blob/main/client/docs/PET_PACK_GUIDE.md';
const enc = encodeURIComponent;
const fileUrlOf = (p) => (key) => `/pets/${enc(p.source)}/${enc(p.id)}/file/${key}`;
const ghLink = (o) => `https://github.com/${o.github}${o.path || o.ref ? `/tree/${o.ref || 'HEAD'}/${o.path || ''}` : ''}`;
const changed = () => window.dispatchEvent(new Event(PETS_CHANGED));

/** 宠物包有问题时的说明 */
export function petError(t, e) {
  if (!e) return '';
  return t(`error:reason.pet_${e.code}`, { detail: e.detail, defaultValue: `${e.code} ${e.detail}` });
}

export default function PetSection() {
  const { t } = useTranslation();
  const [fresh, setFresh] = useState(0);
  const [list, listErr, reloadList] = useLoad(() => api.get(`/pets${fresh ? '?fresh=1' : ''}`), [fresh]);
  const [saved, setErr, reloadSettings] = useLoad(() => api.get('/pets/settings'));
  const [s, setS] = useState(null);
  const [saveErr, setSaveErr] = useState(null);
  useEffect(() => {
    if (saved) setS(saved);
  }, [saved]);

  const save = async (next) => {
    setS(next);
    setSaveErr(null);
    try {
      setS(await api.put('/pets/settings', next));
      changed();
    } catch (e) {
      setSaveErr(e);
    }
  };
  const items = list?.items ?? [];
  const usable = items.filter((p) => !p.error);
  const reload = () => {
    setFresh((n) => n + 1);
    reloadSettings();
  };
  // 别处改了（另一个标签页、后台导入 / 生成台词做完了）：跟着刷新
  usePetsLive();
  useEffect(() => {
    const on = () => {
      reloadList();
      reloadSettings();
    };
    window.addEventListener(PETS_CHANGED, on);
    return () => window.removeEventListener(PETS_CHANGED, on);
  }, []);
  const onInstalled = (p) => {
    reload();
    changed();
    // 第一只宠物：直接用它
    if (p && s && !s.default) save({ ...s, default: p.key });
  };

  return (
    <>
      <SettingsCard s={s} usable={usable} save={save} error={saveErr ?? setErr} />
      <Card
        title={t('pet.packs')}
        actions={
          <>
            <ImportButton onDone={onInstalled} />
            <PickButton onDone={onInstalled} />
            <GithubButton onDone={onInstalled} />
            <Button variant="ghost" icon={RefreshCw} onClick={reload} title={t('pet.rescan')} aria-label={t('pet.rescan')} />
          </>
        }
      >
        <p className="muted small">
          {t('pet.packs_hint')}{' '}
          <a href={GUIDE_URL} target="_blank" rel="noopener noreferrer">
            {t('pet.guide')}
          </a>
        </p>
        <ErrorNote error={listErr} />
        {list && !items.length && <p className="muted">{t('pet.none')}</p>}
        <div className="pet-cards">
          {items.map((p) => (
            <PetCard
              key={p.key}
              p={p}
              onRemoved={() => {
                reloadList();
                changed();
              }}
            />
          ))}
        </div>
        {list?.dirs && (
          <dl className="kv small pet-dirs">
            <dt>{t('pet.dir_local')}</dt>
            <dd className="mono">
              {list.dirs.local} <CopyButton text={list.dirs.local} />
            </dd>
          </dl>
        )}
      </Card>
      <FromDevices mine={items} onDone={onInstalled} />
    </>
  );
}

/**
 * 从同一账户的其他电脑导入（多台电脑时才有）：点开一台看它装了哪些宠物，点“导入”才开始；
 * 下载、校验、安装都在这台电脑的后台进行，离开这个页面也照样装完。
 */
function FromDevices({ mine, onDone }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const [open, setOpen] = useState(null);
  const [jobs, setJobs] = useState([]);
  const seen = useRef(new Set());
  // 自己问同一账户有哪些电脑：“只看本机”时界面上的电脑列表是空的，导入宠物照样要能用
  const [devs] = useLoad(() => api.get('/pets/devices').catch(() => ({ items: [] })));
  const others = devs?.items ?? [];
  const running = jobs.some((j) => j.state === 'running');
  // 第一次打开时看看有没有还在进行的（之前点过、离开了页面）；已经做完的不再提示
  useEffect(() => {
    if (!others.length) return;
    api.get('/pets/from-device').then(
      (r) => {
        const items = r?.items ?? [];
        for (const j of items) if (j.state !== 'running') seen.current.add(j.key + j.state);
        setJobs(items);
      },
      () => {},
    );
  }, [others.length > 0]);
  useEffect(() => {
    if (!running) return undefined;
    const poll = () =>
      api.get('/pets/from-device').then(
        (r) => {
          const items = r?.items ?? [];
          setJobs(items);
          for (const j of items) {
            if (j.state === 'running' || seen.current.has(j.key + j.state)) continue;
            seen.current.add(j.key + j.state);
            if (j.state === 'done') {
              toast?.(t('pet.imported', { name: textOf(j.pack?.name, i18n.language) || j.pet }));
              onDone(j.pack);
            } else toast?.(errorText(t, j.error ?? {}), 'error');
          }
        },
        () => {},
      );
    const id = setInterval(poll, 1000);
    return () => clearInterval(id);
  }, [running]);
  if (!others.length) return null;
  const start = async (dev, p) => {
    try {
      const j = await api.post('/pets/from-device', { device: dev.id, id: p.id });
      seen.current.delete(j.key + 'done');
      seen.current.delete(j.key + 'failed');
      setJobs((old) => [...old.filter((x) => x.key !== j.key), j]);
    } catch (e) {
      toast?.(errorText(t, e), 'error');
    }
  };
  return (
    <Card title={t('pet.from_devices')}>
      <p className="muted small">{t('pet.from_devices_hint')}</p>
      <div className="pet-devs">
        {others.map((dev) => {
          const online = dev.online !== false;
          return (
            <div key={dev.id} className="pet-dev">
              <Button variant="secondary" icon={Monitor} disabled={!online} aria-expanded={open === dev.id} onClick={() => setOpen(open === dev.id ? null : dev.id)}>
                {deviceName(t, dev)}
                {!online && <span className="muted small"> · {t('pet.dev_offline')}</span>}
              </Button>
              {open === dev.id && online && <DevicePets dev={dev} mine={mine} jobs={jobs} onImport={(p) => start(dev, p)} />}
            </div>
          );
        })}
      </div>
    </Card>
  );
}

function DevicePets({ dev, mine, jobs, onImport }) {
  const { t, i18n } = useTranslation();
  const [list, err, reload] = useLoad(() => api.get(`/pets/devices/${enc(dev.id)}`), [dev.id]);
  if (err) {
    return (
      <div className="pet-dev-list">
        <ErrorNote error={err} />
        <Button variant="ghost" icon={RefreshCw} onClick={reload}>
          {t('devices.retry')}
        </Button>
      </div>
    );
  }
  if (!list) return <p className="muted small pet-dev-list">{t('pet.dev_loading')}</p>;
  const items = (list.items ?? []).filter((p) => p.source === 'local' && !p.error);
  if (!items.length) return <p className="muted small pet-dev-list">{t('pet.dev_none')}</p>;
  const mb = (n) => (n / (1 << 20)).toFixed(n < 10 << 20 ? 1 : 0);
  return (
    <ul className="pet-dev-list">
      {items.map((p) => {
        const j = jobs.find((x) => x.key === `${dev.id}:${p.id}`);
        const have = mine.some((m) => m.source === 'local' && m.id === p.id);
        const busy = j?.state === 'running';
        return (
          <li key={p.id}>
            <span aria-hidden="true">🐾</span>
            <b className="ellipsis">{textOf(p.name, i18n.language) || p.id}</b>
            <span className="muted small mono ellipsis">{p.id}</span>
            <span className="grow" />
            {busy && <span className="muted small">{j.total ? t('pet.dev_progress', { pct: Math.floor((j.got / j.total) * 100) }) : t('pet.dev_got', { mb: mb(j.got) })}</span>}
            {!busy && have && <span className="badge">{t('pet.dev_have')}</span>}
            <Button variant={have ? 'ghost' : 'secondary'} icon={Download} busy={busy} disabled={busy} onClick={() => onImport(p)}>
              {t(have ? 'pet.dev_reimport' : 'pet.dev_import')}
            </Button>
          </li>
        );
      })}
    </ul>
  );
}

function SettingsCard({ s, usable, save, error }) {
  const { t, i18n } = useTranslation();
  if (!s) return <Card title={t('pet.title')}>{error ? <ErrorNote error={error} /> : <p className="muted">{t('common.loading')}</p>}</Card>;
  const opts = usable.map((p) => [p.key, textOf(p.name, i18n.language) || p.id]);
  const onOff = [
    [true, t('pet.on')],
    [false, t('pet.off')],
  ];
  const sw = (k, label, hint, def = true) => (
    <Field label={label} hint={hint}>
      <Seg value={def ? s[k] !== false : Boolean(s[k])} label={label} onChange={(v) => save({ ...s, [k]: v })} options={onOff} />
    </Field>
  );
  return (
    <Card title={t('pet.title')}>
      <p className="muted small">{t('pet.intro')}</p>
      {sw('enabled', t('pet.enabled'), undefined, false)}
      {s.enabled && (
        <>
          <Field label={t('pet.which')} hint={!usable.length ? t('pet.default_none') : t('pet.which_hint')}>
            <Select value={s.default ?? ''} aria-label={t('pet.which')} placeholder={t('pet.which_none')} onChange={(e) => save({ ...s, default: e.target.value || null })}>
              {opts.map(([k, n]) => (
                <option key={k} value={k}>
                  {n}
                </option>
              ))}
            </Select>
          </Field>
          <Field label={t('pet.size')}>
            <Seg value={Number(s.size)} label={t('pet.size')} onChange={(v) => save({ ...s, size: v })} options={[0.75, 1, 1.25, 1.5].map((n) => [n, `${Math.round(n * 100)}%`])} />
          </Field>
          {sw('lines', t('pet.lines'))}
          {s.lines !== false && sw('chatter', t('pet.chatter'), t('pet.chatter_hint'))}
          {s.lines !== false && sw('notify', t('pet.notify'), t('pet.notify_hint'))}
          {inDesktopMain() && sw('desktop', t('pet.desktop'), t('pet.desktop_hint'))}
        </>
      )}
      <ErrorNote error={error} />
    </Card>
  );
}

/** 好感度：一起几天、做完几轮、Lv */
function BondLine({ p }) {
  const { t } = useTranslation();
  const [b, setB] = useState(null);
  useEffect(() => {
    let dead = false;
    api.post(`/pets/${enc(p.source)}/${enc(p.id)}/bond`, { add: {} }).then((r) => !dead && setB(r), () => {});
    return () => {
      dead = true;
    };
  }, [p.key]);
  if (!b || !b.met) return null;
  const lv = levelOf(b.xp);
  return (
    <p className="small pet-bond">
      <Heart size={13} aria-hidden="true" /> {t('pet.bond', { level: lv.level, days: b.days, turns: b.turns, pets: b.pets, tests: b.tests })}
      <span className="pet-bond-bar" title={`${b.xp} / ${lv.to}`}>
        <span style={{ width: `${Math.round(lv.progress * 100)}%` }} />
      </span>
    </p>
  );
}

const count = (m) => (m ? Object.values(m).reduce((a, l) => a + l.length, 0) : 0);

/** 风格预设（填进“风格”框，可以再改） */
const STYLES = ['tsundere', 'genki', 'gentle', 'snarky', 'chuuni', 'classic', 'cat', 'butler'];
const API_EFFORTS = ['', 'minimal', 'low', 'medium', 'high', 'xhigh'];

/** 生成台词的进度（卡片上、弹窗里共用）：`job` 是后台任务 `{ state, via, model, at, error }` */
function useGenLines(p) {
  const url = `/pets/${enc(p.source)}/${enc(p.id)}/lines`;
  const [data, err, reload] = useLoad(() => api.get(url), [p.key]);
  const running = data?.job?.state === 'running';
  // 后台在生成：隔一会儿问一次（做完时版本号也会变，usePetsLive 会让整页刷新）
  useEffect(() => {
    if (!running) return undefined;
    const id = setInterval(reload, 2000);
    return () => clearInterval(id);
  }, [running]);
  useEffect(() => {
    window.addEventListener(PETS_CHANGED, reload);
    return () => window.removeEventListener(PETS_CHANGED, reload);
  }, []);
  return { url, data, err, reload, running };
}

/** 卡片上的生成状态：在生成时显示，做完 / 失败时提示一次 */
function GenStatus({ p, name }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { data, running } = useGenLines(p);
  const was = useRef(null);
  useEffect(() => {
    const st = data?.job?.state;
    if (was.current === 'running' && st === 'done') toast?.(t('pet.gen_done_name', { name }));
    if (was.current === 'running' && st === 'failed') toast?.(errorText(t, data.job.error ?? {}), 'error');
    if (st) was.current = st;
  }, [data?.job?.state]);
  if (!running) return null;
  return (
    <p className="small pet-gen-status">
      <Spinner size={12} /> {t('pet.gen_running')}
    </p>
  );
}

/** 按人设生成一批台词（中英两套）：花一次 token，之后宠物说话只是挑一句填数据。在后台生成，关掉窗口也照样做完、马上生效 */
function LinesModal({ p, name, onClose }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const { url, data, err, reload, running } = useGenLines(p);
  const [settings] = useLoad(() => api.get('/pets/settings'));
  const [g, setG] = useState(null);
  const [a, setA] = useState(null);
  const [key, setKey] = useState('');
  const [genErr, setGenErr] = useState(null);
  const [starting, setStarting] = useState(false);
  useEffect(() => {
    if (!settings || g) return;
    const x = settings.gen ?? {};
    setG({ via: x.via || 'auto', model: x.model ?? '', effort: x.effort ?? '', style: x.style ?? '' });
    const y = settings.api ?? {};
    setA({ url: y.url ?? '', model: y.model ?? '', protocol: y.protocol || 'responses', effort: y.effort ?? '', keySet: Boolean(y.keySet) });
  }, [settings]);
  const lang = i18n.language?.startsWith('en') ? 'en-US' : 'zh-CN';
  const mine = data?.[lang];
  const job = data?.job;
  const set = (patch) => setG((x) => ({ ...x, ...patch }));
  const run = async () => {
    setStarting(true);
    setGenErr(null);
    try {
      // 自配接口：先存起来（密钥不填就是不改）
      if (g.via === 'api') {
        const blank = !a.url.trim() && !a.model.trim();
        await api.put('/pets/settings', { ...settings, api: blank ? null : { url: a.url.trim(), model: a.model.trim(), protocol: a.protocol, effort: a.effort, key } });
        setKey('');
      }
      await api.post(url, { via: g.via, model: g.model, effort: g.effort, style: g.style });
      reload();
    } catch (e) {
      setGenErr(e);
    } finally {
      setStarting(false);
    }
  };
  const clear = async () => {
    await api.del(url).catch(() => {});
    reload();
    changed();
  };
  const was = useRef(null);
  useEffect(() => {
    if (was.current === 'running' && job?.state === 'done') toast?.(t('pet.gen_done'));
    if (job?.state) was.current = job.state;
  }, [job?.state]);
  return (
    <Modal open onClose={onClose} title={t('pet.gen_title', { name })} wide>
      <p className="muted small">{t('pet.gen_intro')}</p>
      {!g ? (
        <p className="muted">{t('common.loading')}</p>
      ) : (
        <>
          <Field label={t('pet.gen_via')}>
            <Seg
              value={g.via}
              label={t('pet.gen_via')}
              onChange={(v) => set({ via: v, model: '', effort: '' })}
              options={[
                ['auto', t('pet.gen_via_auto')],
                ['claude', 'Claude Code'],
                ['codex', 'Codex'],
                ['api', t('pet.gen_via_api')],
              ]}
            />
          </Field>
          {(g.via === 'claude' || g.via === 'codex') && <ToolModel via={g.via} model={g.model} effort={g.effort} onChange={set} />}
          {g.via === 'api' && a && <ApiFields a={a} setA={setA} keyVal={key} setKey={setKey} />}
          <Field label={t('pet.gen_style')} hint={t('pet.gen_style_hint')}>
            <div className="pet-style-presets">
              {STYLES.map((k) => (
                <button key={k} type="button" className="chip" aria-pressed={g.style === t(`pet.style.${k}_text`)} onClick={() => set({ style: t(`pet.style.${k}_text`) })}>
                  {t(`pet.style.${k}`)}
                </button>
              ))}
            </div>
            <textarea className="pet-style" rows={2} maxLength={500} value={g.style} placeholder={t('pet.gen_style_ph')} onChange={(e) => set({ style: e.target.value })} />
          </Field>
        </>
      )}
      <div className="pet-gen-actions">
        <Button busy={starting || running} disabled={!g || running} icon={MessageCircle} onClick={run}>
          {mine ? t('pet.gen_again') : t('pet.gen_run')}
        </Button>
        {data?.at && !running && (
          <Button variant="ghost" onClick={clear}>
            {t('pet.gen_clear')}
          </Button>
        )}
      </div>
      {running && <p className="muted small">{t('pet.gen_bg')}</p>}
      {job?.state === 'failed' && !running && <ErrorNote error={{ code: job.error?.code, params: job.error?.params }} />}
      <ErrorNote error={genErr ?? err} />
      {mine && (
        <div className="pet-gen-list">
          <p className="small muted">{t('pet.gen_have', { zh: count(data['zh-CN']), en: count(data['en-US']), via: data.via })}</p>
          <dl className="kv small">
            {Object.entries(mine).map(([k, l]) => (
              <div key={k} className="pet-gen-row">
                <dt className="mono">{k}</dt>
                <dd>{l.join(' · ')}</dd>
              </div>
            ))}
          </dl>
        </div>
      )}
    </Modal>
  );
}

/** Claude Code / Codex：选模型和思考等级（列表现取；没装时提示） */
function ToolModel({ via, model, effort, onChange }) {
  const { t } = useTranslation();
  const [list, err, reload] = useLoad(() => api.get(`/pets/gen-models?via=${via}`), [via]);
  const tool = via === 'claude' ? 'Claude Code' : 'Codex';
  if (err) return <ErrorNote error={err} />;
  if (!list) {
    return (
      <p className="muted small">
        <Spinner size={12} /> {t('pet.gen_models_loading', { tool })}
      </p>
    );
  }
  if (!list.installed) {
    return (
      <div className="pet-gen-warn small">
        {t('pet.gen_not_installed', { tool })}{' '}
        <Button variant="ghost" icon={RefreshCw} onClick={reload}>
          {t('tools.recheck')}
        </Button>
      </div>
    );
  }
  const opts = (list.options ?? []).filter((o) => o.value && o.value !== 'default');
  const cur = opts.find((o) => o.value === model);
  const efforts = (cur ?? opts.find((o) => o.default) ?? (via === 'claude' ? opts[0] : null))?.efforts ?? [];
  const defLabel = via === 'claude' ? t('pet.gen_model_default_claude') : t('pet.gen_model_default');
  return (
    <div className="pet-gen-model">
      <Field label={t('pet.gen_model')}>
        <Select value={model} aria-label={t('pet.gen_model')} onChange={(e) => onChange({ model: e.target.value, effort: '' })}>
          <option value="">{defLabel}</option>
          {opts.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
              {o.description ? ` · ${o.description}` : ''}
            </option>
          ))}
        </Select>
      </Field>
      <Field label={t('pet.gen_effort')}>
        <Select value={effort} aria-label={t('pet.gen_effort')} disabled={!efforts.length} onChange={(e) => onChange({ effort: e.target.value })}>
          <option value="">{t('pet.gen_effort_default')}</option>
          {efforts.map((x) => (
            <option key={x} value={x}>
              {t(`pet.effort.${x}`, { defaultValue: x })}
            </option>
          ))}
        </Select>
      </Field>
      {list.error && <ErrorNote error={list.error} />}
    </div>
  );
}

/** 自配接口：协议、地址、密钥、模型、思考等级 */
function ApiFields({ a, setA, keyVal, setKey }) {
  const { t } = useTranslation();
  const set = (patch) => setA((x) => ({ ...x, ...patch }));
  const ph = a.protocol === 'anthropic' ? ['https://api.anthropic.com', 'claude-haiku-4-5'] : a.protocol === 'chat' ? ['https://api.deepseek.com/v1', 'deepseek-chat'] : ['https://api.openai.com/v1', 'gpt-5-mini'];
  return (
    <div className="pet-gen-api">
      <Field label={t('pet.gen_api_protocol')}>
        <Select value={a.protocol} aria-label={t('pet.gen_api_protocol')} onChange={(e) => set({ protocol: e.target.value })}>
          <option value="responses">{t('pet.proto_responses')}</option>
          <option value="anthropic">{t('pet.proto_anthropic')}</option>
          <option value="chat">{t('pet.proto_chat')}</option>
        </Select>
      </Field>
      <Field label={t('pet.gen_api_url')}>
        <input value={a.url} onChange={(e) => set({ url: e.target.value })} placeholder={ph[0]} spellCheck={false} autoCapitalize="off" />
      </Field>
      <Field label={t('pet.gen_api_key')} hint={a.keySet ? t('pet.gen_api_key_set') : undefined}>
        <input type="password" value={keyVal} onChange={(e) => setKey(e.target.value)} autoComplete="off" placeholder={a.keySet ? '••••••••' : ''} />
      </Field>
      <div className="pet-gen-model">
        <Field label={t('pet.gen_api_model')}>
          <input value={a.model} onChange={(e) => set({ model: e.target.value })} placeholder={ph[1]} spellCheck={false} autoCapitalize="off" />
        </Field>
        <Field label={t('pet.gen_effort')}>
          <Select value={a.effort} aria-label={t('pet.gen_effort')} onChange={(e) => set({ effort: e.target.value })}>
            {API_EFFORTS.map((x) => (
              <option key={x} value={x}>
                {x ? t(`pet.effort.${x}`, { defaultValue: x }) : t('pet.gen_effort_none')}
              </option>
            ))}
          </Select>
        </Field>
      </div>
    </div>
  );
}

function ImportButton({ onDone }) {
  const { t } = useTranslation();
  const toast = useToast();
  const input = useRef(null);
  const [busy, setBusy] = useState(false);
  const pick = async (file) => {
    if (!file) return;
    setBusy(true);
    try {
      const p = await postFile('/pets/import', file);
      toast?.(t('pet.imported', { name: textOf(p.name, 'zh-CN') || p.id }));
      onDone(p);
    } catch (e) {
      toast?.(errorText(t, e), 'error');
    } finally {
      setBusy(false);
      if (input.current) input.current.value = '';
    }
  };
  return (
    <>
      {/* 不加 accept：手机上系统不认识 .llpet，加了过滤反而选不中；不是宠物包的由客户端报错 */}
      <input ref={input} type="file" hidden onChange={(e) => pick(e.target.files?.[0])} />
      <Button variant="secondary" icon={Upload} busy={busy} onClick={() => input.current?.click()}>
        {t('pet.import')}
      </Button>
    </>
  );
}

/** 从电脑里选：浏览运行看看的这台电脑上的文件，选 .llpet / .zip，或者解开的宠物文件夹里的 pet.json（文件不经浏览器上传，远程打开时也能装） */
function PickButton({ onDone }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const pick = async (path) => {
    setOpen(false);
    setBusy(true);
    try {
      const p = await api.post('/pets/import-path', { path });
      toast?.(t('pet.imported', { name: textOf(p.name, i18n.language) || p.id }));
      onDone(p);
    } catch (e) {
      toast?.(errorText(t, e), 'error');
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Button variant="secondary" icon={FolderOpen} busy={busy} onClick={() => setOpen(true)} title={t('pet.pick_hint')}>
        {t('pet.pick')}
      </Button>
      <FolderPicker open={open} files onClose={() => setOpen(false)} onPick={pick} title={t('pet.pick_title')} pickLabel={t('pet.pick_ok')} />
    </>
  );
}

/** 从 GitHub 仓库装：填仓库 → 找出里面的宠物 → 挑几只装（只有一只也给看一眼名字和许可再装） */
function GithubButton({ onDone }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant="secondary" icon={Github} onClick={() => setOpen(true)}>
        {t('pet.gh_button')}
      </Button>
      {open && <GithubDialog onClose={() => setOpen(false)} onDone={onDone} />}
    </>
  );
}

function GithubDialog({ onClose, onDone }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState(null);
  const [scan, setScan] = useState(null);
  const [picked, setPicked] = useState(() => new Set());
  const ok = (scan?.items ?? []).filter((x) => !x.error);

  const find = async (e) => {
    e?.preventDefault();
    if (!url.trim() || busy) return;
    setBusy(true);
    setErr(null);
    setScan(null);
    try {
      const r = await api.post('/pets/from-github', { url: url.trim() });
      setScan(r);
      setPicked(new Set(r.items.filter((x) => !x.error).map((x) => x.index)));
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  const install = async () => {
    setBusy(true);
    setErr(null);
    try {
      const r = await api.post('/pets/from-github/install', { token: scan.token, indexes: [...picked] });
      const names = r.items.map((p) => textOf(p.name, i18n.language) || p.id).join('、');
      toast?.(t('pet.imported', { name: names }));
      onDone(r.items[0]);
      onClose();
    } catch (e2) {
      setErr(e2);
      // 令牌用过或过期了：回到第一步重新找
      setScan(null);
    } finally {
      setBusy(false);
    }
  };
  const toggle = (i) =>
    setPicked((old) => {
      const next = new Set(old);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });

  return (
    <Modal open onClose={onClose} title={t('pet.gh_title')} wide>
      <form onSubmit={find}>
        <Field label={t('pet.gh_repo')} hint={t('pet.gh_repo_hint')}>
          <div className="pet-gh-row">
            <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="owner/repo" spellCheck={false} autoCapitalize="off" autoCorrect="off" />
            <Button type="submit" variant={scan ? 'secondary' : 'primary'} busy={busy && !scan} disabled={!url.trim()}>
              {t('pet.gh_find')}
            </Button>
          </div>
        </Field>
      </form>
      {busy && !scan && <p className="muted small">{t('pet.gh_downloading')}</p>}
      <ErrorNote error={err} />
      {scan && (
        <>
          <p className="small muted">{t(ok.length > 1 ? 'pet.gh_found_many' : 'pet.gh_found', { n: scan.items.length, repo: scan.repo.owner + '/' + scan.repo.name })}</p>
          <div className="pet-cards">
            {scan.items.map((x) => (
              <label key={x.index} className="pet-card pet-gh-item" data-error={x.error ? '' : undefined}>
                <GhThumb token={scan.token} x={x} />
                <div className="pet-card-body">
                  <div className="pet-card-title">
                    <b>{textOf(x.name, i18n.language) || x.id}</b>
                    {x.renderer === 'sprite' && <span className="badge">{t('pet.sprite')}</span>}
                    {x.license?.commercial === false && <span className="badge badge-warn">{t('pet.noncommercial')}</span>}
                  </div>
                  {x.error ? (
                    <p className="field-error">{petError(t, x.error)}</p>
                  ) : (
                    <p className="small muted">
                      {x.author?.name && <>{t('pet.by', { name: x.author.name })} · </>}
                      {x.license?.spdx ?? t('pet.license_unknown')}
                    </p>
                  )}
                  <p className="small muted mono ellipsis">{x.path || '/'}</p>
                </div>
                <div className="pet-card-actions">
                  <input type="checkbox" disabled={Boolean(x.error)} checked={picked.has(x.index)} onChange={() => toggle(x.index)} aria-label={textOf(x.name, i18n.language) || x.id} />
                </div>
              </label>
            ))}
          </div>
          <p className="small muted">{t('pet.gh_license_note')}</p>
        </>
      )}
      <div className="modal-actions">
        <Button variant="ghost" onClick={onClose}>
          {t('action.cancel')}
        </Button>
        {scan && (
          <Button variant="primary" busy={busy} disabled={!picked.size} onClick={install}>
            {t('pet.gh_install', { n: picked.size })}
          </Button>
        )}
      </div>
    </Modal>
  );
}

function GhThumb({ token, x }) {
  const [url, setUrl] = useState(null);
  useEffect(() => {
    let dead = false;
    if (!x.error) loadBlob(`/pets/from-github/preview/${token}/${x.index}`).then((u) => !dead && setUrl(u), () => {});
    return () => {
      dead = true;
    };
  }, [token, x.index]);
  return <div className="pet-thumb">{url ? <img src={url} alt="" /> : <span aria-hidden="true">🐾</span>}</div>;
}

function Thumb({ p }) {
  const [url, setUrl] = useState(null);
  useEffect(() => {
    let dead = false;
    if (p.preview) loadBlob(fileUrlOf(p)(p.preview)).then((u) => !dead && setUrl(u), () => {});
    return () => {
      dead = true;
    };
  }, [p.key, p.preview]);
  return <div className="pet-thumb">{url ? <img src={url} alt="" /> : <span aria-hidden="true">🐾</span>}</div>;
}

function PetCard({ p, onRemoved }) {
  const { t, i18n } = useTranslation();
  const confirm = useConfirm();
  const toast = useToast();
  const [trial, setTrial] = useState(false);
  const [lines, setLines] = useState(false);
  const name = textOf(p.name, i18n.language) || p.id;
  const lic = p.license;
  const apple = videoFormat() === 'mov';
  const remove = async () => {
    if (!(await confirm({ title: t('pet.remove_title', { name }), body: t('pet.remove_body'), confirm: t('action.delete'), danger: true }))) return;
    try {
      await api.del(`/pets/local/${enc(p.id)}`);
      onRemoved();
    } catch (e) {
      toast?.(errorText(t, e), 'error');
    }
  };
  return (
    <div className="pet-card" data-error={p.error ? '' : undefined}>
      <Thumb p={p} />
      <div className="pet-card-body">
        <div className="pet-card-title">
          <b>{name}</b>
          {p.origin?.github ? (
            <a className="badge" href={ghLink(p.origin)} target="_blank" rel="noopener noreferrer" title={p.origin.github}>
              {t('pet.from_github')}
            </a>
          ) : (
            <span className="badge">{t(`pet.src_${p.source}`)}</span>
          )}
          {p.renderer === 'sprite' && <span className="badge">{t('pet.sprite')}</span>}
          {lic?.commercial === false && <span className="badge badge-warn">{t('pet.noncommercial')}</span>}
        </div>
        {p.error ? (
          <p className="field-error">{petError(t, p.error)}</p>
        ) : (
          <>
            {p.description && <p className="muted small">{textOf(p.description, i18n.language)}</p>}
            <p className="small muted">
              {p.author?.name && <>{t('pet.by', { name: p.author.name })} · </>}
              {lic ? (lic.url ? <a href={lic.url} target="_blank" rel="noopener noreferrer">{lic.spdx}</a> : lic.spdx) : t('pet.license_unknown')}
              {lic?.note && <> · {lic.note}</>}
            </p>
            {p.credits?.length > 0 && (
              <p className="small muted">
                {p.credits.map((c, i) => (
                  <span key={i}>
                    {i > 0 && ' → '}
                    {c.role}：{c.url ? <a href={c.url} target="_blank" rel="noopener noreferrer">{c.name}</a> : c.name}
                  </span>
                ))}
              </p>
            )}
            {p.renderer === 'video' && !p.mov && <p className="small muted">{t(apple ? 'pet.no_mov_here' : 'pet.no_mov')}</p>}
            <BondLine p={p} />
            <GenStatus p={p} name={name} />
          </>
        )}
        <p className="small muted mono ellipsis" title={p.dir}>
          {p.dir}
        </p>
      </div>
      <div className="pet-card-actions">
        {!p.error && (
          <Button variant="ghost" icon={Play} onClick={() => setTrial(true)}>
            {t('pet.trial')}
          </Button>
        )}
        {!p.error && (
          <Button variant="ghost" icon={MessageCircle} onClick={() => setLines(true)}>
            {t('pet.gen_button')}
          </Button>
        )}
        {p.source === 'local' && <Button variant="ghost" icon={Trash2} onClick={remove} title={t('action.delete')} aria-label={t('action.delete')} />}
      </div>
      {trial && <Trial p={p} name={name} onClose={() => setTrial(false)} />}
      {lines && <LinesModal p={p} name={name} onClose={() => setLines(false)} />}
    </div>
  );
}

const STATES = ['idle', 'thinking', 'tool', 'waiting', 'terminal', 'offline'];
const ONCE = ['toolDone', 'done', 'error'];
/** 试播里的“看得懂”事件：[台词键, 动作池] */
const EVENTS = [
  ['testPass', 'testPass'],
  ['testFail', 'testFail'],
  ['commit', 'commit'],
  ['recapLong', null],
  ['danger', null],
  ['levelUp', 'levelUp'],
];

/** 试播：挨个播各个状态、动作，不用等 AI 真的跑起来 */
function Trial({ p, name, onClose }) {
  const { t, i18n } = useTranslation();
  const [data, err] = useLoad(() => api.get(`/pets/${enc(p.source)}/${enc(p.id)}`), [p.key]);
  const [gen] = useLoad(() => api.get(`/pets/${enc(p.source)}/${enc(p.id)}/lines`), [p.key]);
  const player = useRef(null);
  const [agent, setAgent] = useState({ state: 'idle', once: null, n: 0 });
  const m = data?.manifest;
  const fileUrl = useMemo(() => fileUrlOf(p), [p.key]);
  const clips = m ? Object.keys(m.clips) : [];
  const trialAct = (kind) => {
    player.current?.play(kind);
    player.current?.say(kind, null, { force: true });
  };
  return (
    <Modal open onClose={onClose} title={t('pet.trial_title', { name })} wide>
      <ErrorNote error={err} />
      <div className="pet-trial-stage">
        {m && <PetPlayer ref={player} manifest={m} fileUrl={fileUrl} agent={agent} height={150} lang={i18n.language} label={name} gen={gen} />}
      </div>
      {m && (
        <div className="pet-trial">
          <p className="small muted">{t('pet.trial_hint')}</p>
          <div className="pet-trial-row">
            <span className="small">{t('pet.trial_states')}</span>
            {STATES.map((k) => (
              <button key={k} type="button" className="chip" aria-pressed={agent.state === k} onClick={() => setAgent((a) => ({ state: k, once: null, n: a.n + 1 }))}>
                {t(`pet.state_${k}`)}
              </button>
            ))}
          </div>
          <div className="pet-trial-row">
            <span className="small">{t('pet.trial_once')}</span>
            {ONCE.map((k) => (
              <button key={k} type="button" className="chip" onClick={() => setAgent((a) => ({ state: a.state, once: k, n: a.n + 1 }))}>
                {t(`pet.state_${k}`)}
              </button>
            ))}
            <button type="button" className="chip" onClick={() => player.current?.play('click')}>
              {t('pet.act_click')}
            </button>
            <button type="button" className="chip" onClick={() => player.current?.play('sleep')}>
              {t('pet.act_sleep')}
            </button>
            <button type="button" className="chip" onClick={() => trialAct('petted')}>
              {t('pet.act_pet')}
            </button>
            <button type="button" className="chip" onClick={() => trialAct('annoyed')}>
              {t('pet.act_annoyed')}
            </button>
          </div>
          <div className="pet-trial-row">
            <span className="small">{t('pet.trial_events')}</span>
            {EVENTS.map(([k, st]) => (
              <button key={k} type="button" className="chip" onClick={() => player.current?.say(k, { dur: '6m 12s', files: 4, cmds: 9, level: 3, n: 42 }, { state: st, force: true })}>
                {t(`pet.ev_${k}`)}
              </button>
            ))}
          </div>
          <details>
            <summary className="small">{t('pet.trial_clips', { n: clips.length })}</summary>
            <div className="pet-trial-row">
              {clips.map((c) => (
                <button key={c} type="button" className="chip" onClick={() => player.current?.play('clip', c)}>
                  {textOf(m.clips[c].title, i18n.language) || c}
                </button>
              ))}
            </div>
          </details>
        </div>
      )}
    </Modal>
  );
}

