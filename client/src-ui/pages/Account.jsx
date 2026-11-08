// 账户：显示名称、邮箱、专属地址。修改密码、两步验证、登录过的浏览器都在网站上（安全卡片只给入口）。数据来自看看平台（shared/platform.js）；
// 连不上平台时显示这台电脑上缓存的账户信息。
import { ExternalLink, LogOut, PenLine } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Initial, useLogout, useStatus, webUrl } from '../App.jsx';
import { api } from '../shared/api.js';
import { platform } from '../shared/platform.js';
import { Button, Card, CopyButton, ErrorNote, Field, Modal, Row, useAction, useLoad, useToast } from '../shared/ui.jsx';
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
      {/* 账户安全只在网站（看看平台）上管：电脑离线、客户端版本旧都不影响，密码和动态码也只在平台域名上输入 */}
      <Card title={t('acct.security')}>
        <Row
          label={t('acct.security_on_web')}
          hint={
            <>
              {t('acct.security_on_web_body')}
              {fresh && (
                <span style={{ display: 'block' }}>
                  {t('acct.two_factor')}：{fresh.two_factor ? t('acct.two_factor_on') : t('acct.two_factor_off')}
                </span>
              )}
            </>
          }
        >
          <a className="btn btn-ghost" href={webUrl(status, '/security')} target="_blank" rel="noopener noreferrer">
            <ExternalLink size={18} aria-hidden="true" />
            {t('acct.security_open')}
          </a>
        </Row>
      </Card>
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
    </>
  );
}
