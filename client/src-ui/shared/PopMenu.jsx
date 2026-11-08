// “更多”菜单：点按钮弹出一列操作。菜单按按钮在屏幕上的位置固定定位（下面放得下就往下，否则往上），
// 不会被滚动列表、终端标题栏裁掉。菜单画在 body 下面（portal）：祖先元素有 contain / content-visibility（侧栏项目列表）
// 或 transform 时，fixed 定位会改成相对那个元素、还会被它裁掉，菜单就看不见了。
import { Check, ChevronDown, MoreHorizontal, X } from 'lucide-react';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';

import { useLayer } from './layers.js';

/**
 * items：[{ icon, label, onClick, danger, disabled, hint, checked }]，假值会被跳过，'sep' 是一条分隔线，{ head } 是一组的小标题。
 * className：按钮的样式（默认幽灵按钮）；icon：按钮图标；onPointerDown：例如手机终端里按住不抢键盘焦点。
 * text：按钮上除了图标还显示的文字（下拉选择，例如对话里切换模型）；disabled：按钮不可用。
 * children：按钮里的内容换成这个（例如侧栏的用户卡片）。sheet：窄屏上从底部弹出（选项多、要好点的菜单）。
 * 底部弹出时：顶上有标题和关闭按钮，往下划关掉；按钮在手机底部标签栏里时弹在标签栏上面，再点一下那个标签就关。
 * hover：有鼠标时停在按钮上就弹出，移开（按钮和菜单都离开）就收起；悬停弹出后再点按钮，菜单留着，跟点开的一样。
 */
export default function PopMenu({ items, className = 'btn btn-ghost btn-icon', icon: Icon = MoreHorizontal, label, onPointerDown, text, disabled, children, sheet = false, hover = false }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState(null);
  const btn = useRef(null);
  const list = useRef(null);
  // 悬停弹出的（还没点过）：鼠标离开就收起
  const byHover = useRef(false);
  const hoverTimer = useRef(0);
  const canHover = hover && !disabled && typeof window !== 'undefined' && window.matchMedia?.('(hover: hover) and (pointer: fine)').matches;
  const hoverIn = () => {
    if (!canHover) return;
    clearTimeout(hoverTimer.current);
    if (open) return;
    hoverTimer.current = setTimeout(() => {
      byHover.current = true;
      setPos(null);
      setOpen(true);
    }, 150);
  };
  const hoverOut = () => {
    if (!canHover) return;
    clearTimeout(hoverTimer.current);
    if (open && byHover.current) hoverTimer.current = setTimeout(() => setOpen(false), 250);
  };
  useEffect(() => () => clearTimeout(hoverTimer.current), []);
  useEffect(() => {
    if (!open) {
      byHover.current = false;
      return undefined;
    }
    const close = (e) => !btn.current?.contains(e.target) && !list.current?.contains(e.target) && setOpen(false);
    const esc = (e) => {
      if (e.key !== 'Escape') return;
      setOpen(false);
      btn.current?.focus();
    };
    // 点进终端（iframe）：焦点落在 iframe 上才关（切到别的窗口不关）。终端重连、刷新后自动聚焦在菜单开着时不做
    // （见 termFrame.js 的 termFocus），不然连接不稳、终端反复重连时菜单刚弹出就被关掉
    const away = () =>
      setTimeout(() => {
        if (document.activeElement?.tagName === 'IFRAME') setOpen(false);
      }, 0);
    // 只有按钮所在的那块滚动了（菜单跟着按钮走，位置就不对了）才关；别处自己滚（对话往下滚、终端输出）不管。
    // 菜单自己滚动（底部弹出的长菜单）不算；手机上地址栏、系统栏收起展开会改窗口高度，底部弹出时不跟着关
    const scrolled = (e) => {
      if (list.current?.contains(e.target)) return;
      const el = e.target;
      if (el === document || el === document.documentElement || el === document.body || el?.contains?.(btn.current)) setOpen(false);
    };
    const resized = () => !sheetRef.current && setOpen(false);
    document.addEventListener('pointerdown', close);
    document.addEventListener('keydown', esc);
    window.addEventListener('resize', resized);
    window.addEventListener('scroll', scrolled, true);
    // 点进终端（iframe）时外面收不到 pointerdown，靠失焦关掉
    window.addEventListener('blur', away);
    return () => {
      document.removeEventListener('pointerdown', close);
      document.removeEventListener('keydown', esc);
      window.removeEventListener('resize', resized);
      window.removeEventListener('scroll', scrolled, true);
      window.removeEventListener('blur', away);
    };
  }, [open]);
  const asSheet = sheet && open && window.matchMedia('(max-width: 640px)').matches;
  // 手机底部弹出的菜单：返回键收起它（桌面上的下拉菜单点别处就关，不进历史）
  useLayer(asSheet, () => setOpen(false));
  const sheetRef = useRef(false);
  sheetRef.current = asSheet;
  // 按钮在底部标签栏里：弹在标签栏上面，标签栏还能点
  const overTabs = asSheet && Boolean(btn.current?.closest('.mtabs'));
  const drag = useSwipeDown(list, () => setOpen(false));
  useLayoutEffect(() => {
    if (!open || !btn.current || !list.current || asSheet) return;
    const r = btn.current.getBoundingClientRect();
    const h = list.current.offsetHeight;
    const w = list.current.offsetWidth;
    // 放得下就放下面，否则放上面；两边都放不下（菜单很长、按钮在屏幕底部的标签栏里）时放空间大的一边，限高滚动，
    // 不要伸出窗口底部被挡住
    const vh = window.visualViewport?.height ?? window.innerHeight;
    const roomBelow = vh - r.bottom - 14;
    const roomAbove = r.top - 14;
    const below = roomBelow >= h || (roomAbove < h && roomBelow >= roomAbove);
    const maxHeight = Math.max(120, below ? roomBelow : roomAbove);
    const shownH = Math.min(h, maxHeight);
    setPos({
      top: below ? Math.min(r.bottom + 6, vh - shownH - 8) : Math.max(8, r.top - shownH - 6),
      left: Math.max(8, Math.min(r.right - w, window.innerWidth - w - 8)),
      maxHeight: h > maxHeight ? maxHeight : undefined,
      overflowY: h > maxHeight ? 'auto' : undefined,
    });
  }, [open]);
  const shown = items.filter(Boolean);
  const uid = useId().replace(/[^a-zA-Z0-9_-]/g, '');
  return (
    <>
      <button
        ref={btn}
        type="button"
        className={className}
        aria-label={label ?? t('action.more')}
        title={label ?? t('action.more')}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onPointerDown={onPointerDown}
        onMouseEnter={hoverIn}
        onMouseLeave={hoverOut}
        onClick={(e) => {
          e.stopPropagation();
          clearTimeout(hoverTimer.current);
          // 悬停弹出的：点一下就是要它留着
          if (open && byHover.current) {
            byHover.current = false;
            return;
          }
          setPos(null);
          setOpen(!open);
        }}
      >
        {children ?? <Icon size={text ? 14 : 18} aria-hidden="true" />}
        {!children && text && (
          <>
            <span className="ellipsis">{text}</span>
            <ChevronDown size={13} aria-hidden="true" />
          </>
        )}
      </button>
      {asSheet && createPortal(<div className={`menu-pop-backdrop ${overTabs ? 'over-tabs' : ''}`} aria-hidden="true" />, document.body)}
      {open && createPortal(
        <ul
          ref={list}
          className={`menu-pop-list ${asSheet ? `menu-pop-sheet ${overTabs ? 'over-tabs' : ''}` : 'menu-pop-fixed'}`}
          role="menu"
          aria-label={label}
          style={asSheet ? undefined : pos ?? { visibility: 'hidden', top: 0, left: 0 }}
          onMouseEnter={hoverIn}
          onMouseLeave={hoverOut}
          {...(asSheet ? drag : {})}
        >
          {asSheet && (
            <li className="menu-pop-sheet-head" role="presentation">
              <span className="menu-pop-grab" aria-hidden="true" />
              <b className="ellipsis">{label}</b>
              <button type="button" className="menu-pop-close" aria-label={t('action.close')} onClick={() => setOpen(false)}>
                <X size={18} aria-hidden="true" />
              </button>
            </li>
          )}
          {shown.map((x, i) =>
            x === 'sep' ? (
              <li key={`sep-${i}`} className="menu-pop-sep" role="separator" />
            ) : x.head ? (
              <li key={`head-${i}`} className="menu-pop-head" role="presentation">
                {x.head}
              </li>
            ) : (
            <li key={`${i}:${x.label}`}>
              <button
                type="button"
                role={x.checked !== undefined ? 'menuitemradio' : 'menuitem'}
                aria-checked={x.checked}
                className={x.danger ? 'danger' : ''}
                disabled={x.disabled}
                aria-describedby={x.hint ? `${uid}-${i}-hint` : undefined}
                onPointerDown={onPointerDown}
                onClick={(e) => {
                  e.stopPropagation();
                  setOpen(false);
                  x.onClick();
                }}
              >
                {x.icon && <x.icon size={17} aria-hidden="true" />}
                {x.checked !== undefined && <Check size={15} aria-hidden="true" style={{ visibility: x.checked ? 'visible' : 'hidden' }} />}
                <span className="grow">
                  {x.label}
                  {x.hint && (
                    <span className="menu-pop-hint" id={`${uid}-${i}-hint`}>
                      {x.hint}
                    </span>
                  )}
                </span>
              </button>
            </li>
            ),
          )}
        </ul>,
        document.body,
      )}
    </>
  );
}

/**
 * 底部弹出的菜单往下划关掉：菜单滚在最顶上时手指往下拖，跟着往下移；拖过 80px（或者快速一划）松手就关，否则弹回去。
 * 返回要放到菜单元素上的事件。
 */
function useSwipeDown(ref, onClose) {
  const st = useRef(null);
  const set = (y) => {
    const el = ref.current;
    if (!el) return;
    el.style.transform = y > 0 ? `translateY(${y}px)` : '';
    el.style.transition = y > 0 ? 'none' : '';
  };
  return {
    onTouchStart: (e) => {
      const el = ref.current;
      st.current = el && el.scrollTop <= 0 ? { y: e.touches[0].clientY, t: Date.now(), dy: 0 } : null;
    },
    onTouchMove: (e) => {
      const s = st.current;
      if (!s) return;
      s.dy = e.touches[0].clientY - s.y;
      // 往上拖、或者菜单已经滚动了：交给滚动
      if (s.dy <= 0 || ref.current.scrollTop > 0) {
        set(0);
        if (s.dy < 0) st.current = null;
        return;
      }
      set(s.dy);
    },
    onTouchEnd: () => {
      const s = st.current;
      st.current = null;
      if (!s) return;
      const fast = s.dy > 30 && Date.now() - s.t < 250;
      if (s.dy > 80 || fast) onClose();
      else set(0);
    },
  };
}
