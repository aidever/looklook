// 管理台的“文件”页：先是快速访问（最近改动、项目、收藏、常用位置，docs/REMOTE_UX_2026-11-07.md §9.3），
// 进了文件夹就浏览、预览（含 Markdown）、上传下载、新建文件夹、重命名、移动 / 复制、删除（进回收站，能撤销）。
// 文件夹放在地址里（`?path=`）：每进一个文件夹一条历史，返回键逐级退回，刷新、分享都回到同一个文件夹。
// 多台电脑时标题下面写着是哪台，可以换（换了电脑就回到快速访问）。
import { useTranslation } from 'react-i18next';

import { selectedDevice } from '../shared/api.js';
import { PageHead } from '../shared/page.jsx';
import { query, useRouter } from '../shared/router.jsx';
import FileBrowser from './FileBrowser.jsx';
import { useSessionUi } from './Sessions.jsx';

export default function Files() {
  const { t } = useTranslation();
  const device = selectedDevice();
  const ui = useSessionUi();
  const { search, navigate } = useRouter();
  const path = query(search, 'path') ?? '';
  const go = (p) => {
    if (p === path) return;
    navigate(p ? `/files?path=${encodeURIComponent(p)}` : '/files');
  };
  return (
    <>
      <PageHead title={t('fb.page_title')} sub={t('fb.page_sub')} help="files" perDevice />
      <div className="files-page">
        <FileBrowser
          key={device ?? 'local'}
          path={path}
          onPath={go}
          openSession={ui?.openSession}
          onNewHere={ui ? (dir) => ui.newSession({ device, workdir: dir }) : undefined}
        />
      </div>
    </>
  );
}
