import { StrictMode, Suspense, lazy } from 'react';
import { createRoot } from 'react-dom/client';

import App from './App.jsx';
import { installRtcDownloads } from './shared/api.js';
import ErrorBoundary, { reportUiError } from './shared/ErrorBoundary.jsx';
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
