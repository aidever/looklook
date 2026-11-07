// 文件预览里的音视频播放（Vidstack 默认界面：倍速、画中画、全屏、音量、快捷键、手机上双击快进）。
// 按需加载（FileBrowser 的 Preview 里 lazy）：不看视频的人不用下这几百 KB。
// 地址直接指向电脑的下载接口（`inline=1`，电脑给真实类型），浏览器用 Range 边下边播、拖进度条只取那一段。
import '@vidstack/react/player/styles/default/theme.css';
import '@vidstack/react/player/styles/default/layouts/audio.css';
import '@vidstack/react/player/styles/default/layouts/video.css';

import { MediaPlayer, MediaProvider } from '@vidstack/react';
import { DefaultAudioLayout, defaultLayoutIcons, DefaultVideoLayout } from '@vidstack/react/player/layouts/default';
import { useTranslation } from 'react-i18next';

const ZH = {
  Announcements: '公告', Accessibility: '无障碍', AirPlay: 'AirPlay', Audio: '音轨', Auto: '自动', Boost: '增强', Captions: '字幕',
  'Caption Styles': '字幕样式', 'Captions look like this': '字幕是这个样子', Chapters: '章节', 'Closed-Captions Off': '关闭字幕',
  'Closed-Captions On': '打开字幕', Connected: '已连接', Continue: '继续', Connecting: '正在连接', Default: '默认', Disabled: '已关闭',
  Disconnected: '已断开', 'Display Background': '背景', Download: '下载', 'Enter Fullscreen': '全屏', 'Enter PiP': '画中画',
  'Exit Fullscreen': '退出全屏', 'Exit PiP': '退出画中画', Font: '字体', Family: '字体', Fullscreen: '全屏', 'Google Cast': 'Google Cast',
  'Keyboard Animations': '按键提示动画', LIVE: '直播', Loop: '循环播放', Mute: '静音', Normal: '正常', Off: '关', Pause: '暂停', Play: '播放',
  Playback: '播放', PiP: '画中画', Quality: '清晰度', Replay: '重新播放', Reset: '重置', 'Seek Backward': '后退', 'Seek Forward': '快进',
  Seek: '跳转', Settings: '设置', 'Skip To Live': '回到直播', Speed: '倍速', Size: '大小', Color: '颜色', Opacity: '不透明度', Shadow: '阴影',
  Text: '文字', 'Text Background': '文字背景', Track: '轨道', Unmute: '取消静音', Volume: '音量',
};

/** Vidstack 按类型挑播放方式：它不认识的容器（mov、mkv、wav…）也交给 `<video>` / `<audio>`，能不能放由浏览器看编码决定 */
const VIDEO_AS = { webm: 'video/webm', ogv: 'video/ogg' };
const AUDIO_AS = { ogg: 'audio/ogg', oga: 'audio/ogg', opus: 'audio/ogg', flac: 'audio/flac', m4a: 'audio/mp4', aac: 'audio/mp4' };
const ext = (name) => name.toLowerCase().split('.').pop();

/** `audio`：只有声音；`onError`：浏览器放不了（编码不支持、取不到） */
export default function Player({ src, name, audio = false, onError }) {
  const { i18n } = useTranslation();
  const zh = (i18n.resolvedLanguage ?? i18n.language ?? '').startsWith('zh');
  const type = audio ? AUDIO_AS[ext(name)] ?? 'audio/mpeg' : VIDEO_AS[ext(name)] ?? 'video/mp4';
  return (
    <MediaPlayer
      className={`fb-player ${audio ? 'fb-player-audio' : ''}`}
      title={name}
      src={{ src, type }}
      viewType={audio ? 'audio' : 'video'}
      playsInline
      load="eager"
      // 键盘快捷键只在播放器有焦点时生效：不抢文件列表的 ↑↓
      keyTarget="player"
      onError={onError}
    >
      <MediaProvider />
      {audio ? (
        <DefaultAudioLayout icons={defaultLayoutIcons} translations={zh ? ZH : null} colorScheme="system" />
      ) : (
        <DefaultVideoLayout icons={defaultLayoutIcons} translations={zh ? ZH : null} colorScheme="dark" />
      )}
    </MediaPlayer>
  );
}
