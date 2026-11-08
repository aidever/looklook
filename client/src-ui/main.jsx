import { StrictMode, Suspense, lazy } from 'react';
import { createRoot } from 'react-dom/client';

import App from './App.jsx';
import { installRtcDownloads } from './shared/api.js';
import ErrorBoundary, { reportUiError } from './shared/ErrorBoundary.jsx';
import { baseDeepLink, installDesktopNav } from './shared/layers.js';
import './shared/i18n.js';
import { ConfirmProvider, ToastProvider } from './shared/ui.jsx';
import './styles.css';

window.addEventListener('error', (e) => reportUiError(e.message, e.error?.stack ?? `${e.filename}:${e.lineno}`));
window.addEventListener('unhandledrejection', (e) => {
  const r = e.reason;
  // 接口返回的业务错误（ApiError）已经在界面上提示过，不算脚本错误
  if (r?.code && r?.status !== undefined) return;
  reportUiError(r?.message ?? String(r), r?.stack ?? '');
});

installRtcDownloads();
// 从链接直接打开会话、终端、工具页时垫一条首页：手机上按返回回到首页，而不是离开网站（docs/REMOTE_UX_2026-11-07.md §7.2）。
// 抽屉页（设置、账户……）关掉时自己会回到下面那页，不垫
baseDeepLink((p) => p !== '/' && p !== '/pet-desk' && !['/settings', '/devices', '/account', '/membership'].includes(p.replace(/\/+$/, '')));
// 桌面 App 没有浏览器的前进 / 后退按钮：⌘[ / ⌘]、Alt+← / →、鼠标侧键
installDesktopNav();

// 桌面应用的宠物窗口：只放宠物（src/desktop.rs 开的透明置顶窗口）
const DeskPet = lazy(() => import('./pages/DeskPet.jsx'));
const root = createRoot(document.getElementById('root'));

if (window.location.pathname === '/pet-desk') {
  root.render(
    <StrictMode>
      <Suspense fallback={null}>
        <DeskPet />
      </Suspense>
    </StrictMode>,
  );
} else {
  root.render(
  <StrictMode>
    <ToastProvider>
      <ConfirmProvider>
        <ErrorBoundary page>
          <App />
        </ErrorBoundary>
      </ConfirmProvider>
    </ToastProvider>
  </StrictMode>,
  );
}
