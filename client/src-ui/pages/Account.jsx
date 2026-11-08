// 账户与安全：显示名称、邮箱、专属地址、修改密码、两步验证（去网站上开）、网页登录（浏览器）。数据来自看看平台（shared/platform.js）；
// 连不上平台时显示这台电脑上缓存的账户信息。
import { ExternalLink, KeyRound, LogOut, MonitorSmartphone, PenLine } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Initial, useLogout, useStatus, webUrl } from '../App.jsx';
import { api } from '../shared/api.js';
import { platform } from '../shared/platform.js';
import { Button, Card, CopyButton, ErrorNote, Field, Modal, PasswordInput, Row, Skeleton, Time, useAction, useLoad, useToast } from '../shared/ui.jsx';
import { SourceTag } from './Membership.jsx';

function NicknameModal({ me, onClose, onDone }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [v, setV] = useState(me.nickname);
  const { busy, error, run } = useAction();
  const submit = (e) => {
    e.preventDefault();
    run(async () => {
      await platform.patch('/me', { nickname: v.trim() });
      toast(t('common.saved'));
      onDone();
    });
  };
  return (
    <Modal open onClose={onClose} title={t('acct.edit_nickname')}>
      <form onSubmit={submit}>
        <Field label={t('acct.nickname')} hint={t('acct.nickname_hint')}>
          <input maxLength={32} value={v} onChange={(e) => setV(e.target.value)} autoFocus />
        </Field>
        <ErrorNote error={error} />
        <div className="modal-actions">
          <Button variant="ghost" onClick={onClose}>
            {t('action.cancel')}
          </Button>
          <Button type="submit" busy={busy} disabled={!v.trim() || v.trim() === me.nickname}>
            {t('action.save')}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** 两步验证的动态码（认证器 App 上的 6 位数字，或一个恢复码）。 */
export function MfaCodeField({ value, onChange }) {
  const { t } = useTranslation();
  return (
    <Field label={t('acct.mfa_code')} hint={t('acct.mfa_code_hint')}>
      <input autoComplete="one-time-code" autoCapitalize="none" spellCheck={false} maxLength={12} placeholder="000000" value={value} onChange={(e) => onChange(e.target.value)} autoFocus />
    </Field>
  );
}

function PasswordModal({ onClose }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [cur, setCur] = useState('');
  const [next, setNext] = useState('');
  // 账户开了两步验证：平台回 MFA_REQUIRED 后再显示动态码输入框
  const [code, setCode] = useState('');
  const [needCode, setNeedCode] = useState(false);
  const { busy, error, run } = useAction();
  const submit = (e) => {
    e.preventDefault();
    run(async () => {
      try {
        await platform.put('/me/password', { current_password: cur, new_password: next, code: needCode ? code : undefined });
      } catch (err) {
        if (err.code === 'MFA_REQUIRED') setNeedCode(true);
        throw err;
      }
      toast(t('acct.password_changed'));
      onClose();
    });
  };
  return (
    <Modal open onClose={onClose} title={t('acct.change_password')}>
      <form onSubmit={submit}>
        <Field label={t('acct.current_password')}>
          <PasswordInput autoComplete="current-password" value={cur} onChange={(e) => setCur(e.target.value)} />
        </Field>
        <Field label={t('acct.new_password')} hint={t('acct.password_hint')}>
          <PasswordInput autoComplete="new-password" minLength={8} value={next} onChange={(e) => setNext(e.target.value)} />
        </Field>
        {needCode && <MfaCodeField value={code} onChange={setCode} />}
        <p className="muted small">{t('acct.password_note')}</p>
        <ErrorNote error={error} />
        <div className="modal-actions">
          <Button variant="ghost" onClick={onClose}>
            {t('action.cancel')}
          </Button>
          <Button type="submit" busy={busy} disabled={!cur || next.length < 8}>
            {t('action.save')}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function browserName(ua = '') {
  if (!ua) return '—';
  const os = /iPhone|iPad/.test(ua) ? 'iOS' : /Android/.test(ua) ? 'Android' : /Mac OS/.test(ua) ? 'macOS' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : '';
  const br = /Edg\//.test(ua) ? 'Edge' : /Chrome\//.test(ua) ? 'Chrome' : /Firefox\//.test(ua) ? 'Firefox' : /Safari\//.test(ua) ? 'Safari' : '';
  return [br, os].filter(Boolean).join(' · ') || ua.slice(0, 40);
}

function Sessions() {
  const { t } = useTranslation();
  const toast = useToast();
  const [data, error, reload] = useLoad(() => platform.get('/sessions'));
  const { busy, error: actionError, run } = useAction();
  const others = data?.items?.filter((s) => !s.current) ?? [];
  const signOut = (ids, msg) =>
    run(async () => {
      for (const id of ids) await platform.del(`/sessions/${id}`);
      toast(msg);
      reload();
    });
  return (
    <Card
      title={t('acct.browsers')}
      actions={
        others.length > 1 && (
          <Button variant="ghost" busy={busy} onClick={() => signOut(others.map((s) => s.id), t('acct.others_signed_out'))}>
            {t('acct.sign_out_all')}
          </Button>
        )
      }
    >
      <p className="muted small">{t('acct.browsers_lead')}</p>
      <ErrorNote error={error || actionError} />
      {!data ? (
        !error && <Skeleton height={60} />
      ) : data.items.length === 0 ? (
        <p className="muted small">{t('acct.browsers_empty')}</p>
      ) : (
        <ul className="list">
          {data.items.map((s) => (
            <li key={s.id}>
              <span className="inline grow">
                <MonitorSmartphone size={18} className="muted" aria-hidden="true" />
                <span>
                  <b className="small">{browserName(s.user_agent)}</b> {s.current && <span className="badge badge-ok">{t('acct.this_browser')}</span>}
                  <span className="muted small" style={{ display: 'block' }}>
                    {s.ip || '—'} · <Time value={s.last_seen_at} only="relative" />
                  </span>
                </span>
              </span>
              {!s.current && (
                <Button variant="plain" busy={busy} onClick={() => signOut([s.id], t('acct.signed_out'))}>
                  {t('acct.sign_out')}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

export default function Account() {
  const { t } = useTranslation();
  const { status, reload: reloadStatus } = useStatus();
  const logout = useLogout();
  const [fresh, error, reload] = useLoad(() => platform.get('/me'));
  const [modal, setModal] = useState(null);
  const s = status.account.session;
  // 连不上平台时用这台电脑上缓存的信息
  const me = fresh ?? { nickname: s.user.nickname, email: s.user.email, username: s.user.username, user_host_url: s.user_host_url };
  const host = (me.user_host_url || '').replace(/^https?:\/\//, '').replace(/\/$/, '');
  return (
    <>
      <div className="page-head">
        <h1>{t('acct.account')}</h1>
        <SourceTag error={error} />
      </div>
      <Card>
        <div className="profile">
          <Initial name={me.nickname} large />
          <span className="grow" style={{ minWidth: 0 }}>
            <b style={{ fontSize: 18 }}>{me.nickname}</b>
            <span className="muted small" style={{ display: 'block' }}>
              {me.email}
            </span>
          </span>
          <Button variant="ghost" icon={PenLine} disabled={!fresh} onClick={() => setModal('nickname')}>
            {t('acct.edit_nickname')}
          </Button>
        </div>
        <dl className="kv" style={{ margin: '16px 0 10px', alignItems: 'center' }}>
          <dt>{t('acct.address')}</dt>
          <dd className="inline" style={{ gap: 4 }}>
            <span className="mono ellipsis">{host}</span>
            <CopyButton text={me.user_host_url} variant="plain" label={false} />
          </dd>
          <dt>{t('acct.username')}</dt>
          <dd>
            <span className="mono">{me.username}</span> <span className="muted small">· {t('acct.fixed')}</span>
          </dd>
        </dl>
        <p className="muted small" style={{ margin: 0 }}>
          {t('acct.address_hint')}
        </p>
      </Card>
      <Card title={t('acct.security')}>
        <Row label={t('acct.change_password')} hint={t('acct.password_body')}>
          <Button variant="ghost" icon={KeyRound} disabled={!fresh} onClick={() => setModal('password')}>
            {t('acct.change')}
          </Button>
        </Row>
        {/* 扫码绑定、恢复码只在网站上做；?web=1 让网站不要因为有电脑在线又跳回管理台 */}
        <Row label={t('acct.two_factor')} hint={!fresh ? t('acct.two_factor_body') : fresh.two_factor ? t('acct.two_factor_on') : t('acct.two_factor_off')}>
          <a className="btn btn-ghost" href={webUrl(status, '/security?web=1')} target="_blank" rel="noopener noreferrer">
            <ExternalLink size={18} aria-hidden="true" />
            {fresh?.two_factor ? t('acct.two_factor_manage') : t('acct.two_factor_enable')}
          </a>
        </Row>
      </Card>
      <Sessions />
      <div className="inline" style={{ marginBottom: 24 }}>
        <Button variant="danger" icon={LogOut} onClick={logout}>
          {t('nav.logout')}
        </Button>
      </div>
      {modal === 'nickname' && (
        <NicknameModal
          me={me}
          onClose={() => setModal(null)}
          onDone={() => {
            setModal(null);
            reload();
            api.get('/account').catch(() => {}).finally(reloadStatus);
          }}
        />
      )}
      {modal === 'password' && <PasswordModal onClose={() => setModal(null)} />}
    </>
  );
}
