// 管理台的“文件”页：浏览、预览（含 Markdown）、上传下载、新建文件夹、重命名、删除这台电脑上的文件。
// 多台电脑时标题下面写着是哪台，可以换（换了电脑就从那台的主目录重新开始）。
// 能做什么写在标题下面的一行说明里，不再弹一次性的提示条。
import { useTranslation } from 'react-i18next';

import { selectedDevice } from '../shared/api.js';
import { PageHead } from '../shared/page.jsx';
import FileBrowser from './FileBrowser.jsx';

export default function Files() {
  const { t } = useTranslation();
  const device = selectedDevice();
  return (
    <>
      <PageHead title={t('fb.page_title')} sub={t('fb.page_sub')} help="files" perDevice />
      <div className="files-page">
        <FileBrowser key={device ?? 'local'} />
      </div>
    </>
  );
}
