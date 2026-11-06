// 下拉选择：替换原生 <select>（各平台样式不一、深色模式下很丑）。用法和原生一样：
//   <Select value={v} onChange={(e) => set(e.target.value)}><option value="a">A</option>…</Select>
// onChange 收到的 e.target.value 是字符串（同原生）；也可以传 options={[[value, label, { disabled, hint }]]} 代替 <option>。
// 列表画在 body 下面（portal），按按钮位置固定定位，下面放不下就往上；窄屏触屏从底部弹出。键盘：↑↓ Home End 回车 Esc、打字跳到开头相同的项。
import { Check, ChevronDown, X } from 'lucide-react';
import { Children, isValidElement, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';

/** 把 <option> 子元素（可以套在数组、Fragment 里）摊平成 [{ value, label, disabled }] */
function optionsOf(children) {
  const out = [];
  const walk = (nodes) =>
    Children.forEach(nodes, (c) => {
      if (!isValidElement(c)) return;
      if (c.type === 'option') {
        const label = c.props.children;
        out.push({ value: String(c.props.value ?? (typeof label === 'string' ? label : '')), label, disabled: Boolean(c.props.disabled) });
      } else if (c.props?.children) walk(c.props.children);
    });
  walk(children);
  return out;
}

const textOf = (node) => {
  if (node == null || node === false) return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  return isValidElement(node) ? textOf(node.props.children) : '';
};

export default function Select({ value, onChange, options, children, disabled, className = '', style, placeholder, sheet = true, 'aria-label': ariaLabel, title, id, name, required, ...rest }) {
  const { t } = useTranslation();
  const opts = options ? options.map(([v, label, o]) => ({ value: String(v), label, disabled: Boolean(o?.disabled), hint: o?.hint })) : optionsOf(children);
  const cur = String(value ?? '');
  const sel = opts.find((o) => o.value === cur);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState(null);
  const [hi, setHi] = useState(-1);
  const btn = useRef(null);
  const list = useRef(null);
  const typed = useRef({ s: '', t: 0 });
  const uid = useId().replace(/[^a-zA-Z0-9_-]/g, '');
  const asSheet = sheet && open && window.matchMedia('(max-width: 640px) and (pointer: coarse)').matches;

  const pick = (o) => {
    if (!o || o.disabled) return;
    setOpen(false);
    btn.current?.focus();
    if (o.value !== cur) onChange?.({ target: { value: o.value, name }, currentTarget: { value: o.value, name } });
  };
  const move = (from, step) => {
    if (!opts.length) return -1;
    let i = from;
    for (let n = 0; n < opts.length; n++) {
      i = (i + step + opts.length) % opts.length;
      if (!opts[i].disabled) return i;
    }
    return from;
  };

  useEffect(() => {
    if (!open) return undefined;
    setHi(Math.max(0, opts.findIndex((o) => o.value === cur)));
    const close = (e) => !btn.current?.contains(e.target) && !list.current?.contains(e.target) && setOpen(false);
    const scrolled = (e) => {
      if (list.current?.contains(e.target)) return;
      if (e.target?.contains?.(btn.current)) setOpen(false);
    };
    const resized = () => !asSheet && setOpen(false);
    document.addEventListener('pointerdown', close);
    window.addEventListener('scroll', scrolled, true);
    window.addEventListener('resize', resized);
    window.addEventListener('blur', () => setOpen(false), { once: true });
    return () => {
      document.removeEventListener('pointerdown', close);
      window.removeEventListener('scroll', scrolled, true);
      window.removeEventListener('resize', resized);
    };
  }, [open]);

  useLayoutEffect(() => {
    if (!open || asSheet || !btn.current || !list.current) return;
    const r = btn.current.getBoundingClientRect();
    const vh = window.visualViewport?.height ?? window.innerHeight;
    const h = list.current.scrollHeight + 2;
    const below = vh - r.bottom - 10;
    const above = r.top - 10;
    const down = below >= Math.min(h, 240) || below >= above;
    const maxHeight = Math.min(360, Math.max(120, down ? below : above));
    const shown = Math.min(h, maxHeight);
    const w = Math.max(r.width, Math.min(list.current.scrollWidth + 2, 360));
    setPos({ top: down ? r.bottom + 4 : r.top - shown - 4, left: Math.max(8, Math.min(r.left, window.innerWidth - w - 8)), minWidth: r.width, maxWidth: Math.max(r.width, 360), maxHeight });
  }, [open, asSheet]);

  // 高亮的那项滚进视野
  useEffect(() => {
    if (open && hi >= 0) list.current?.querySelector(`[data-i="${hi}"]`)?.scrollIntoView?.({ block: 'nearest' });
  }, [open, hi, pos]);

  const onKey = (e) => {
    if (disabled) return;
    if (!open) {
      if (['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(e.key)) {
        e.preventDefault();
        setPos(null);
        setOpen(true);
      }
      return;
    }
    if (e.key === 'Escape' || e.key === 'Tab') {
      if (e.key === 'Escape') e.preventDefault();
      setOpen(false);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHi((i) => move(i, 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHi((i) => move(i < 0 ? 0 : i, -1));
    } else if (e.key === 'Home') {
      e.preventDefault();
      setHi(move(-1, 1));
    } else if (e.key === 'End') {
      e.preventDefault();
      setHi(move(opts.length, -1));
    } else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      pick(opts[hi]);
    } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const now = Date.now();
      typed.current = { s: (now - typed.current.t > 700 ? '' : typed.current.s) + e.key.toLowerCase(), t: now };
      const i = opts.findIndex((o) => !o.disabled && textOf(o.label).toLowerCase().startsWith(typed.current.s));
      if (i >= 0) setHi(i);
    }
  };

  const shownLabel = sel ? sel.label : placeholder ?? '';
  return (
    <>
      <button
        {...rest}
        ref={btn}
        id={id}
        type="button"
        className={`select ${className}`}
        style={style}
        disabled={disabled}
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? `${uid}-list` : undefined}
        aria-activedescendant={open && hi >= 0 ? `${uid}-${hi}` : undefined}
        aria-label={ariaLabel}
        aria-required={required || undefined}
        title={title ?? (textOf(shownLabel) || undefined)}
        data-empty={sel ? undefined : ''}
        onClick={() => {
          setPos(null);
          setOpen(!open);
        }}
        onKeyDown={onKey}
      >
        <span className="select-value ellipsis">{shownLabel}</span>
        <ChevronDown size={15} aria-hidden="true" className="select-caret" />
      </button>
      {open && asSheet && createPortal(<div className="menu-pop-backdrop" aria-hidden="true" />, document.body)}
      {open &&
        createPortal(
          <ul
            ref={list}
            id={`${uid}-list`}
            role="listbox"
            aria-label={ariaLabel}
            className={`menu-pop-list select-list ${asSheet ? 'menu-pop-sheet' : 'menu-pop-fixed'}`}
            style={asSheet ? undefined : pos ?? { visibility: 'hidden', top: 0, left: 0 }}
            onKeyDown={onKey}
          >
            {asSheet && (
              <li className="menu-pop-sheet-head" role="presentation">
                <span className="menu-pop-grab" aria-hidden="true" />
                <b className="ellipsis">{ariaLabel ?? ''}</b>
                <button type="button" className="menu-pop-close" aria-label={t('action.close')} onClick={() => setOpen(false)}>
                  <X size={18} aria-hidden="true" />
                </button>
              </li>
            )}
            {opts.map((o, i) => (
              <li key={`${i}:${o.value}`} role="presentation">
                <button
                  type="button"
                  role="option"
                  id={`${uid}-${i}`}
                  data-i={i}
                  aria-selected={o.value === cur}
                  className={i === hi ? 'hi' : ''}
                  disabled={o.disabled}
                  tabIndex={-1}
                  onMouseMove={() => hi !== i && setHi(i)}
                  onClick={(e) => {
                    e.stopPropagation();
                    pick(o);
                  }}
                >
                  <Check size={15} aria-hidden="true" style={{ visibility: o.value === cur ? 'visible' : 'hidden' }} />
                  <span className="grow">
                    {o.label}
                    {o.hint && <span className="menu-pop-hint">{o.hint}</span>}
                  </span>
                </button>
              </li>
            ))}
          </ul>,
          document.body,
        )}
    </>
  );
}
