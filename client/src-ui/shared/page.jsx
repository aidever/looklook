// 页面骨架（docs/DONT_MAKE_ME_THINK_2026-11.md §3）：每一页都长一个样，用户不用每页重新学。
//   PageHead：标题（和导航上的名字一样）+ 一行说明 + “？”帮助 + 作用于哪台电脑 + 右边最多一个主要操作。
//   Row：一个设置项。左边名字和一行说明，右边开关 / 按钮；控件太宽时（stack）放到下面。
//   More：收起来的次要内容（高级选项、原理说明），默认不展开。
import { useTranslation } from 'react-i18next';

import { useStatus, webUrl } from '../App.jsx';
import { DeviceSwitcher, useDevices } from './devices.jsx';
import { HelpLink } from './help.jsx';

export function PageHead({ title, icon: Icon, sub, help, perDevice = false, children }) {
  const { t } = useTranslation();
  const { status } = useStatus();
  const devices = useDevices();
  const switcher = perDevice && (devices.multi || devices.localOnly);
  return (
    <header className="phead">
      <div className="phead-main">
        <h1 className="phead-title">
          {Icon && <Icon size={22} aria-hidden="true" />}
          <span>{title}</span>
          {help && <HelpLink page={help} />}
        </h1>
        {children && <div className="phead-actions">{children}</div>}
      </div>
      {(sub || switcher) && (
        <div className="phead-sub">
          {sub && <span>{sub}</span>}
          {/* 多台电脑：这一页操作的是哪台，就写在说明后面，点它换 */}
          {switcher && (
            <span className="phead-dev">
              <span className="muted">{t('nav.tools_on')}</span>
              <DeviceSwitcher perDevice helpUrl={webUrl(status, '/docs/remote')} />
            </span>
          )}
        </div>
      )}
    </header>
  );
}

// Row、More 放在 ui.jsx（没有别的依赖，devices.jsx 这类底层文件也能用），这里一并导出
export { More, Row } from './ui.jsx';
