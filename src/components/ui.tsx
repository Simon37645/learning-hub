// 通用 UI 原件：图标、弹窗、下拉菜单、开关、Toast。
// 图标全部内联 SVG（stroke 1.6，16px 网格），不引入图标库。

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useApp } from "../store/app";

// ---------------------------------------------------------------- 图标

export type IconName =
  | "plus"
  | "search"
  | "hub"
  | "book"
  | "calendar"
  | "settings"
  | "chat"
  | "file"
  | "folder"
  | "layers"
  | "check"
  | "close"
  | "chevron-down"
  | "chevron-right"
  | "arrow-up"
  | "arrow-left"
  | "square"
  | "globe"
  | "external"
  | "refresh"
  | "trash"
  | "sparkle"
  | "copy"
  | "pencil"
  | "clock"
  | "panel"
  | "more"
  | "alert"
  | "eye"
  | "download"
  | "play"
  | "zoom-in"
  | "zoom-out"
  | "list"
  | "target"
  | "sun"
  | "moon"
  | "auto"
  | "puzzle"
  | "plug"
  | "image"
  | "hammer"
  | "box";

const PATHS: Record<IconName, ReactNode> = {
  plus: <path d="M8 3.5v9M3.5 8h9" />,
  search: (
    <>
      <circle cx="7.2" cy="7.2" r="4.2" />
      <path d="M10.4 10.4 13.5 13.5" />
    </>
  ),
  hub: (
    <>
      <rect x="2.2" y="2.2" width="5" height="5" rx="1.2" />
      <rect x="8.8" y="2.2" width="5" height="5" rx="1.2" />
      <rect x="2.2" y="8.8" width="5" height="5" rx="1.2" />
      <rect x="8.8" y="8.8" width="5" height="5" rx="1.2" />
    </>
  ),
  book: (
    <>
      <path d="M2.5 3.2h4.2c.9 0 1.5.6 1.5 1.5v8c0-.8-.6-1.4-1.5-1.4H2.5z" />
      <path d="M13.5 3.2H9.3c-.9 0-1.5.6-1.5 1.5v8c0-.8.6-1.4 1.5-1.4h4.2z" />
    </>
  ),
  calendar: (
    <>
      <rect x="2.2" y="3.2" width="11.6" height="10.6" rx="1.6" />
      <path d="M2.2 6.4h11.6M5.6 2v2.4M10.4 2v2.4" />
    </>
  ),
  // 齿轮：6 个齿 + 中心轴孔。
  // 原来是「圆 + 8 条短线」，和 sun 图标几乎一样，用户看着就是「一个太阳」——
  // 16px 网格上用 8 个齿配 1.6 的描边会糊成一团，所以齿少而厚（外径 6.55 / 齿根 4.85）。
  settings: (
    <>
      <circle cx="8" cy="8" r="2.2" />
      <path d="M9.09 3.27 9.92 1.74 12.47 3.21 11.55 4.69 12.64 6.58 14.38 6.53 14.38 9.47 12.64 9.42 11.55 11.31 12.47 12.79 9.92 14.26 9.09 12.73 6.91 12.73 6.08 14.26 3.53 12.79 4.45 11.31 3.36 9.42 1.62 9.47 1.62 6.53 3.36 6.58 4.45 4.69 3.53 3.21 6.08 1.74 6.91 3.27Z" />
    </>
  ),
  chat: <path d="M13.6 8.4c0 2.9-2.5 5.2-5.6 5.2-.8 0-1.5-.2-2.2-.4l-3 1 .9-2.6A5 5 0 0 1 2.4 8.4c0-2.9 2.5-5.2 5.6-5.2s5.6 2.3 5.6 5.2Z" />,
  file: (
    <>
      <path d="M9 1.8H4.6c-.7 0-1.2.5-1.2 1.2v10c0 .7.5 1.2 1.2 1.2h6.8c.7 0 1.2-.5 1.2-1.2V5.2z" />
      <path d="M9 1.8v3.4h3.6" />
    </>
  ),
  folder: <path d="M2 4.2c0-.7.5-1.2 1.2-1.2h2.4l1.4 1.6h5.8c.7 0 1.2.5 1.2 1.2v5.8c0 .7-.5 1.2-1.2 1.2H3.2c-.7 0-1.2-.5-1.2-1.2z" />,
  layers: (
    <>
      <path d="M8 2 2.4 5.2 8 8.4l5.6-3.2z" />
      <path d="M2.4 8.8 8 12l5.6-3.2M2.4 11.4 8 14.6l5.6-3.2" />
    </>
  ),
  check: <path d="M3.2 8.6 6.4 11.6 12.8 4.8" />,
  close: <path d="M4 4l8 8M12 4l-8 8" />,
  "chevron-down": <path d="M4 6.2 8 10l4-3.8" />,
  "chevron-right": <path d="M6.2 4 10 8l-3.8 4" />,
  "arrow-up": <path d="M8 13V3.4M4 7.4 8 3.4l4 4" />,
  "arrow-left": <path d="M13 8H3.4M7.4 4 3.4 8l4 4" />,
  square: <rect x="3.6" y="3.6" width="8.8" height="8.8" rx="1.6" />,
  globe: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M2.2 8h11.6M8 2c1.6 1.7 2.4 3.7 2.4 6S9.6 12.3 8 14c-1.6-1.7-2.4-3.7-2.4-6S6.4 3.7 8 2Z" />
    </>
  ),
  external: (
    <>
      <path d="M9.4 3.2h3.4v3.4" />
      <path d="M12.8 3.2 7.6 8.4" />
      <path d="M12 9.6v2.8c0 .7-.5 1.2-1.2 1.2H3.6c-.7 0-1.2-.5-1.2-1.2V5.2c0-.7.5-1.2 1.2-1.2h2.8" />
    </>
  ),
  refresh: (
    <>
      <path d="M13.4 8a5.4 5.4 0 1 1-1.6-3.8" />
      <path d="M13.6 2.6v3.2h-3.2" />
    </>
  ),
  trash: (
    <>
      <path d="M3.2 4.4h9.6M6.4 4.4V3c0-.4.3-.8.8-.8h1.6c.5 0 .8.4.8.8v1.4" />
      <path d="M4.6 4.4l.6 7.6c0 .6.5 1 1.1 1h3.4c.6 0 1.1-.4 1.1-1l.6-7.6" />
    </>
  ),
  sparkle: (
    <>
      <path d="M8 2.4l1.3 3.4 3.4 1.3-3.4 1.3L8 11.8 6.7 8.4 3.3 7.1l3.4-1.3z" />
      <path d="M12.4 11.2l.5 1.2 1.2.5-1.2.5-.5 1.2-.5-1.2-1.2-.5 1.2-.5z" />
    </>
  ),
  copy: (
    <>
      <rect x="5.6" y="5.6" width="8" height="8" rx="1.4" />
      <path d="M10.4 5.6V3.8c0-.8-.6-1.4-1.4-1.4H3.8c-.8 0-1.4.6-1.4 1.4v5.2c0 .8.6 1.4 1.4 1.4h1.8" />
    </>
  ),
  pencil: (
    <>
      <path d="M11.2 2.6 13.4 4.8 5.8 12.4l-3 .6.6-3z" />
      <path d="M9.6 4.2 11.8 6.4" />
    </>
  ),
  clock: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.6V8l2.4 1.6" />
    </>
  ),
  panel: (
    <>
      <rect x="2.2" y="2.8" width="11.6" height="10.4" rx="1.6" />
      <path d="M9.6 2.8v10.4" />
    </>
  ),
  more: (
    <>
      <circle cx="3.6" cy="8" r="1" />
      <circle cx="8" cy="8" r="1" />
      <circle cx="12.4" cy="8" r="1" />
    </>
  ),
  alert: (
    <>
      <path d="M8 2.6 14 13H2z" />
      <path d="M8 6.4v3.2M8 11.4v.1" />
    </>
  ),
  eye: (
    <>
      <path d="M1.8 8s2.4-4.2 6.2-4.2S14.2 8 14.2 8s-2.4 4.2-6.2 4.2S1.8 8 1.8 8Z" />
      <circle cx="8" cy="8" r="1.8" />
    </>
  ),
  download: (
    <>
      <path d="M8 2.6v7.2M4.8 7 8 10.2 11.2 7" />
      <path d="M3 12.6h10" />
    </>
  ),
  play: <path d="M5.4 3.4 12.2 8l-6.8 4.6z" />,
  "zoom-in": (
    <>
      <circle cx="7" cy="7" r="4.4" />
      <path d="M10.2 10.2 13.6 13.6M7 5.2v3.6M5.2 7h3.6" />
    </>
  ),
  "zoom-out": (
    <>
      <circle cx="7" cy="7" r="4.4" />
      <path d="M10.2 10.2 13.6 13.6M5.2 7h3.6" />
    </>
  ),
  list: (
    <>
      <path d="M5.6 4.4h8M5.6 8h8M5.6 11.6h8" />
      <path d="M2.6 4.4h.1M2.6 8h.1M2.6 11.6h.1" />
    </>
  ),
  target: (
    <>
      <circle cx="8" cy="8" r="5.6" />
      <circle cx="8" cy="8" r="2.4" />
    </>
  ),
  sun: (
    <>
      <circle cx="8" cy="8" r="3.1" />
      <path d="M8 1.6v1.6M8 12.8v1.6M14.4 8h-1.6M3.2 8H1.6M12.5 3.5l-1.1 1.1M4.6 11.4l-1.1 1.1M12.5 12.5l-1.1-1.1M4.6 4.6 3.5 3.5" />
    </>
  ),
  moon: <path d="M13 9.6A5.6 5.6 0 0 1 6.4 3c0-.6.1-1.2.3-1.7A5.6 5.6 0 1 0 13 9.6Z" />,
  auto: (
    <>
      <rect x="1.8" y="3" width="12.4" height="8.4" rx="1.4" />
      <path d="M5.6 13.6h4.8M8 11.4v2.2" />
    </>
  ),
  puzzle: (
    <>
      <path d="M6.2 2.4c.9 0 1.6.7 1.6 1.6v.6h1.4c.6 0 1 .4 1 1v1.6h.6c.9 0 1.6.7 1.6 1.6s-.7 1.6-1.6 1.6h-.6v1.4c0 .6-.4 1-1 1H7.8v-.6c0-.9-.7-1.6-1.6-1.6s-1.6.7-1.6 1.6v.6H3.4c-.6 0-1-.4-1-1V8.8h.6c.9 0 1.6-.7 1.6-1.6S3.9 5.6 3 5.6h-.6V4.6c0-.6.4-1 1-1h2.8z" />
    </>
  ),
  plug: (
    <>
      <path d="M6 1.8v3.4M10 1.8v3.4" />
      <path d="M3.6 5.2h8.8v2.4a4.4 4.4 0 0 1-8.8 0z" />
      <path d="M8 12v2.2" />
    </>
  ),
  // 归档：一个带盖的收纳盒
  box: (
    <>
      <rect x="2.2" y="2.6" width="11.6" height="3.2" rx="1" />
      <path d="M3.2 5.8v6.2c0 .7.5 1.2 1.2 1.2h7.2c.7 0 1.2-.5 1.2-1.2V5.8" />
      <path d="M6.6 8.6h2.8" />
    </>
  ),
  image: (
    <>
      <rect x="2.2" y="3" width="11.6" height="10" rx="1.6" />
      <circle cx="5.9" cy="6.4" r="1.1" />
      <path d="M3 11.2l3-2.6 2.4 2 1.8-1.5 2.6 2.2" />
    </>
  ),
  // 工坊：一把锤子（造东西），和「技能」的拼图块、「MCP」的插头区分开
  hammer: (
    <>
      <path d="M9.6 2.4l3.9 3.9-1.6 1.6-1.1-1.1-5.4 5.4-1.7-1.7 5.4-5.4-1.1-1.1z" />
      <path d="M4.3 11.1l1.6 1.6-1.2 1.2a1.1 1.1 0 0 1-1.6-1.6z" />
    </>
  ),
};

export function Icon({
  name,
  size = 15,
  className,
  style,
}: {
  name: IconName;
  size?: number;
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={{ flex: "none", ...style }}
      aria-hidden
    >
      {PATHS[name]}
    </svg>
  );
}

// App 图标（首屏那个大标记，和桌面图标同源）
export function AppMark({ size = 76, color = "var(--text)" }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size * 0.62} viewBox="0 0 100 62" fill="none" aria-hidden>
      <g stroke={color} strokeWidth="5.4" strokeLinecap="round" strokeLinejoin="round">
        <path d="M4 46 32 6 60 46" opacity="0.26" />
        <path d="M24 46 47 14 70 46" opacity="0.45" />
        <path d="M44 46 63 21 82 46" opacity="0.8" />
      </g>
    </svg>
  );
}

// ---------------------------------------------------------------- 弹窗

export function Modal({
  title,
  icon,
  children,
  footer,
  onClose,
  wide,
}: {
  title: ReactNode;
  icon?: IconName;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  wide?: boolean;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={"modal" + (wide ? " wide" : "")} role="dialog" aria-modal>
        <div className="modal-head">
          {icon && <Icon name={icon} size={16} />}
          <span className="grow">{title}</span>
          <button className="icon-btn" onClick={onClose} title="关闭 (Esc)">
            <Icon name="close" />
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-foot">{footer}</div>}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 下拉菜单

export function Dropdown({
  trigger,
  children,
  align = "left",
  up,
}: {
  trigger: (open: boolean) => ReactNode;
  children: (close: () => void) => ReactNode;
  align?: "left" | "right";
  up?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number; ready: boolean }>({ left: 0, top: 0, ready: false });
  const ref = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  // 菜单渲染到 body 上（portal）：原来它长在触发按钮旁边，而侧栏主题列表是
  // overflow:auto 的滚动容器——菜单会被裁掉、还会被后面的内容盖住
  // （用户报的「点⋯被上方内容遮住、找不到新建子主题」就是这么来的）。
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      let anchor = ref.current?.getBoundingClientRect();
      // 主题行的「⋯」平时是 display:none（悬停才显示），量出来是 0×0 —— 那就退回外层
      if (anchor && anchor.width === 0 && anchor.height === 0) {
        const outer = ref.current?.parentElement?.getBoundingClientRect();
        if (outer && (outer.width > 0 || outer.height > 0)) anchor = outer;
      }
      // 还是 0×0（没有真实指针，比如脚本触发）→ 用整行当锚点，至少别跑到左上角
      if (anchor && anchor.width === 0 && anchor.height === 0) {
        const row = ref.current?.closest<HTMLElement>(".topic-row, .chat-row, .list-row");
        const rr = row?.getBoundingClientRect();
        if (rr && rr.width > 0) anchor = rr;
      }
      const menu = menuRef.current;
      if (!anchor || !menu) return;
      const h = menu.offsetHeight;
      const w = menu.offsetWidth;
      const below = anchor.bottom + 6 + h <= window.innerHeight - 8;
      const wantUp = up === undefined ? !below : up;
      let top = wantUp ? anchor.top - 6 - h : anchor.bottom + 6;
      top = Math.max(8, Math.min(top, window.innerHeight - h - 8));
      let left = align === "right" ? anchor.right - w : anchor.left;
      left = Math.max(8, Math.min(left, window.innerWidth - w - 8));
      setPos({ left, top, ready: true });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, align, up]);

  useEffect(() => {
    if (!open) {
      setPos({ left: 0, top: 0, ready: false });
      return;
    }
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (ref.current?.contains(t) || menuRef.current?.contains(t)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} style={{ position: "relative", display: "inline-flex" }}>
      <div onClick={() => setOpen((v) => !v)} style={{ display: "inline-flex" }}>
        {trigger(open)}
      </div>
      {open &&
        createPortal(
          <div
            ref={menuRef}
            className="menu portal"
            style={{ left: pos.left, top: pos.top, visibility: pos.ready ? "visible" : "hidden" }}
          >
            {children(() => setOpen(false))}
          </div>,
          document.body,
        )}
    </div>
  );
}

export function MenuItem({
  children,
  onClick,
  selected,
  danger,
}: {
  children: ReactNode;
  onClick?: () => void;
  selected?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      className={"menu-item" + (selected ? " sel" : "")}
      style={danger ? { color: "var(--danger)" } : undefined}
      onClick={onClick}
    >
      {children}
      {selected && <Icon name="check" size={13} className="check" />}
    </button>
  );
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return <div className="menu-label">{children}</div>;
}

export function MenuSep() {
  return <div className="menu-sep" />;
}

// ---------------------------------------------------------------- 其它

export function Switch({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: ReactNode;
}) {
  return (
    <label className="switch">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      {label && <span>{label}</span>}
    </label>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="field">
      <label>{label}</label>
      {children}
      {hint && <div className="hint">{hint}</div>}
    </div>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { id: T; label: string; title?: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="segmented">
      {options.map((o) => (
        <button
          key={o.id}
          className={o.id === value ? "on" : ""}
          onClick={() => onChange(o.id)}
          title={o.title}
          type="button"
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Empty({ icon, children }: { icon?: IconName; children: ReactNode }) {
  return (
    <div className="empty">
      {icon && <Icon name={icon} size={22} style={{ opacity: 0.5 }} />}
      <div>{children}</div>
    </div>
  );
}

export function Spinner() {
  return <span className="spinner" />;
}

export function Toasts() {
  const toasts = useApp((s) => s.toasts);
  const dismiss = useApp((s) => s.dismissToast);
  if (toasts.length === 0) return null;
  return (
    <div className="toast-host">
      {toasts.map((t) => (
        <div key={t.id} className={"toast " + t.level} onClick={() => dismiss(t.id)}>
          <Icon
            name={t.level === "error" || t.level === "warn" ? "alert" : t.level === "success" ? "check" : "hub"}
            size={14}
            style={{ marginTop: 2 }}
          />
          <div className="grow">{t.message}</div>
        </div>
      ))}
    </div>
  );
}

/** 自动长高的 textarea */
export function AutoTextarea({
  value,
  onChange,
  onSend,
  onPaste,
  placeholder,
  minRows = 1,
  maxHeight = 220,
  autoFocus,
}: {
  value: string;
  onChange: (v: string) => void;
  onSend?: () => void;
  /** 粘贴：输入框里用它接截图（Ctrl+V） */
  onPaste?: (e: { clipboardData: DataTransfer | null; preventDefault: () => void }) => void;
  placeholder?: string;
  minRows?: number;
  maxHeight?: number;
  autoFocus?: boolean;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, maxHeight)}px`;
  }, [value, maxHeight]);

  useEffect(() => {
    if (autoFocus) ref.current?.focus();
  }, [autoFocus]);

  return (
    <textarea
      ref={ref}
      className="textarea"
      style={{
        border: "none",
        background: "transparent",
        padding: "12px 14px 4px",
        minHeight: minRows * 24 + 24,
        maxHeight,
        resize: "none",
        fontFamily: "inherit",
        fontSize: 13.5,
        lineHeight: 1.65,
      }}
      value={value}
      placeholder={placeholder}
      onChange={(e) => onChange(e.target.value)}
      onPaste={onPaste}
      onKeyDown={(e) => {
        if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && onSend) {
          e.preventDefault();
          onSend();
        }
        if (e.key === "Enter" && !e.shiftKey && !e.metaKey && !e.ctrlKey && onSend) {
          e.preventDefault();
          onSend();
        }
      }}
    />
  );
}
