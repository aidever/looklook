// 设置 → 宠物（docs/PET_PACK.md §6.3）：给每种对话配一只宠物、大小、气泡；列出找得到的宠物包，导入、试播、删除。
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Download, FolderOpen, Github, Heart, MessageCircle, Monitor, Play, RefreshCw, Trash2, Upload } from 'lucide-react';
import { api, deviceApi, postFile } from '../shared/api.js';
import { deviceName, useDevices } from '../shared/devices.jsx';
import { errorText } from '../shared/i18n.js';
import { PETS_CHANGED, inDesktopMain } from '../shared/ChatPet.jsx';
import { levelOf } from '../shared/pet/bond.js';
import PetPlayer from '../shared/pet/PetPlayer.jsx';
import { loadBlob, videoFormat } from '../shared/pet/media.js';
import { textOf } from '../shared/pet/brain.js';
import { Button, Card, CopyButton, ErrorNote, Field, Modal, Seg, useConfirm, useLoad, useToast } from '../shared/ui.jsx';
import { FolderPicker } from './Terminals.jsx';

const AGENTS = [
  ['claude', 'Claude Code'],
  ['codex', 'Codex'],
  ['dsh', 'DSH'],
  ['opencode', 'OpenCode'],
];
const GUIDE_URL = 'https://github.com/imythu/looklook/blob/main/client/docs/PET_PACK_GUIDE.md';
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
  const onInstalled = (p) => {
    reload();
    changed();
    // 第一只宠物：直接设成默认
    if (p && s && !s.default && !Object.values(s.byAgent ?? {}).some(Boolean)) save({ ...s, default: p.key });
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
  const d = useDevices();
  const toast = useToast();
  const [open, setOpen] = useState(null);
  const [jobs, setJobs] = useState([]);
  const seen = useRef(new Set());
  const others = d.multi ? d.items.filter((x) => x.id !== d.selected) : [];
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
  }, [others.length > 0, d.selected]);
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
        {others.map((dev) => (
          <div key={dev.id} className="pet-dev">
            <Button variant="secondary" icon={Monitor} disabled={!dev.online} aria-expanded={open === dev.id} onClick={() => setOpen(open === dev.id ? null : dev.id)}>
              {deviceName(t, dev)}
              {!dev.online && <span className="muted small"> · {t('pet.dev_offline')}</span>}
            </Button>
            {open === dev.id && dev.online && <DevicePets dev={dev} mine={mine} jobs={jobs} onImport={(p) => start(dev, p)} />}
          </div>
        ))}
      </div>
    </Card>
  );
}

function DevicePets({ dev, mine, jobs, onImport }) {
  const { t, i18n } = useTranslation();
  const [list, err, reload] = useLoad(() => deviceApi(dev.id).get('/pets'), [dev.id]);
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
  const opts = usable.map((p) => [p.key, `${textOf(p.name, i18n.language) || p.id} · ${t(`pet.src_${p.source}`)}`]);
  const byAgent = s.byAgent ?? {};
  // 选项值：'' = 跟默认，'-' = 不显示，其余是宠物
  const agentValue = (a) => (a in byAgent ? byAgent[a] ?? '-' : '');
  const setAgent = (a, v) => {
    const next = { ...byAgent };
    if (v === '') delete next[a];
    else next[a] = v === '-' ? null : v;
    save({ ...s, byAgent: next });
  };
  return (
    <Card title={t('pet.title')}>
      <p className="muted small">{t('pet.intro')}</p>
      <Field label={t('pet.enabled')}>
        <Seg
          value={Boolean(s.enabled)}
          label={t('pet.enabled')}
          onChange={(v) => save({ ...s, enabled: v })}
          options={[
            [true, t('pet.on')],
            [false, t('pet.off')],
          ]}
        />
      </Field>
      {s.enabled && (
        <>
          <Field label={t('pet.default')} hint={!usable.length ? t('pet.default_none') : undefined}>
            <select value={s.default ?? '-'} onChange={(e) => save({ ...s, default: e.target.value === '-' ? null : e.target.value })}>
              <option value="-">{t('pet.hidden')}</option>
              {opts.map(([k, n]) => (
                <option key={k} value={k}>
                  {n}
                </option>
              ))}
            </select>
          </Field>
          <div className="pet-agents">
            {AGENTS.map(([a, n]) => (
              <Field key={a} label={n}>
                <select value={agentValue(a)} onChange={(e) => setAgent(a, e.target.value)}>
                  <option value="">{t('pet.follow_default')}</option>
                  <option value="-">{t('pet.hidden')}</option>
                  {opts.map(([k, name]) => (
                    <option key={k} value={k}>
                      {name}
                    </option>
                  ))}
                </select>
              </Field>
            ))}
          </div>
          <Field label={t('pet.size')}>
            <Seg value={Number(s.size)} label={t('pet.size')} onChange={(v) => save({ ...s, size: v })} options={[0.75, 1, 1.25, 1.5].map((n) => [n, `${Math.round(n * 100)}%`])} />
          </Field>
          <Field label={t('pet.lines')}>
            <Seg
              value={s.lines !== false}
              label={t('pet.lines')}
              onChange={(v) => save({ ...s, lines: v })}
              options={[
                [true, t('pet.on')],
                [false, t('pet.off')],
              ]}
            />
          </Field>
          {s.lines !== false && (
            <Field label={t('pet.chatter')} hint={t('pet.chatter_hint')}>
              <Seg
                value={s.chatter !== false}
                label={t('pet.chatter')}
                onChange={(v) => save({ ...s, chatter: v })}
                options={[
                  [true, t('pet.on')],
                  [false, t('pet.off')],
                ]}
              />
            </Field>
          )}
          {inDesktopMain() && (
            <Field label={t('pet.desktop')} hint={t('pet.desktop_hint')}>
              <Seg
                value={s.desktop !== false}
                label={t('pet.desktop')}
                onChange={(v) => save({ ...s, desktop: v })}
                options={[
                  [true, t('pet.on')],
                  [false, t('pet.off')],
                ]}
              />
            </Field>
          )}
          <GenApiFields s={s} save={save} />
        </>
      )}
      <ErrorNote error={error} />
    </Card>
  );
}

/** 生成人设台词用的接口（OpenAI 兼容）：不填就用本机的 Claude Code / Codex */
function GenApiFields({ s, save }) {
  const { t } = useTranslation();
  const a = s.api ?? {};
  const [url, setUrl] = useState(a.url ?? '');
  const [model, setModel] = useState(a.model ?? '');
  const [key, setKey] = useState('');
  const dirty = url !== (a.url ?? '') || model !== (a.model ?? '') || key !== '';
  const apply = () => {
    const blank = !url.trim() && !model.trim();
    save({ ...s, api: blank ? null : { url: url.trim(), model: model.trim(), key } });
    setKey('');
  };
  return (
    <details className="pet-gen-api">
      <summary className="small">{t('pet.gen_api')}</summary>
      <p className="muted small">{t('pet.gen_api_hint')}</p>
      <Field label={t('pet.gen_api_url')}>
        <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://api.deepseek.com/v1" spellCheck={false} />
      </Field>
      <Field label={t('pet.gen_api_model')}>
        <input value={model} onChange={(e) => setModel(e.target.value)} placeholder="deepseek-chat" spellCheck={false} />
      </Field>
      <Field label={t('pet.gen_api_key')} hint={a.keySet ? t('pet.gen_api_key_set') : undefined}>
        <input type="password" value={key} onChange={(e) => setKey(e.target.value)} autoComplete="off" placeholder={a.keySet ? '••••••••' : ''} />
      </Field>
      <Button variant="secondary" disabled={!dirty} onClick={apply}>
        {t('action.save')}
      </Button>
    </details>
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

/** 按人设生成一批台词（中英两套）：花一次 token，之后宠物说话只是挑一句填数据 */
function LinesModal({ p, name, onClose }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const url = `/pets/${enc(p.source)}/${enc(p.id)}/lines`;
  const [data, err, reload] = useLoad(() => api.get(url), [p.key]);
  const [via, setVia] = useState('auto');
  const [busy, setBusy] = useState(false);
  const [genErr, setGenErr] = useState(null);
  const lang = i18n.language?.startsWith('en') ? 'en-US' : 'zh-CN';
  const mine = data?.[lang];
  const run = async () => {
    setBusy(true);
    setGenErr(null);
    try {
      await api.post(url, { via });
      reload();
      changed();
      toast?.(t('pet.gen_done'));
    } catch (e) {
      setGenErr(e);
    } finally {
      setBusy(false);
    }
  };
  const clear = async () => {
    await api.del(url).catch(() => {});
    reload();
    changed();
  };
  return (
    <Modal open onClose={onClose} title={t('pet.gen_title', { name })} wide>
      <p className="muted small">{t('pet.gen_intro')}</p>
      <Field label={t('pet.gen_via')}>
        <Seg
          value={via}
          label={t('pet.gen_via')}
          onChange={setVia}
          options={[
            ['auto', t('pet.gen_via_auto')],
            ['claude', 'Claude Code'],
            ['codex', 'Codex'],
            ['api', t('pet.gen_via_api')],
          ]}
        />
      </Field>
      <div className="pet-gen-actions">
        <Button busy={busy} icon={MessageCircle} onClick={run}>
          {mine ? t('pet.gen_again') : t('pet.gen_run')}
        </Button>
        {data?.at && (
          <Button variant="ghost" onClick={clear}>
            {t('pet.gen_clear')}
          </Button>
        )}
      </div>
      {busy && <p className="muted small">{t('pet.gen_wait')}</p>}
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

