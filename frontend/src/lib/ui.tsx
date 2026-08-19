/**
 * Local UI primitives for the gRPC plugin frontend (self-contained, no host
 * `@/components/ui/*` imports). Styled via `src/styles.css` with the violet
 * #8B5CF6 accent and light/dark system theme support.
 */
import React from 'react';
import { cn } from './cn';

// ============================================================================
// Icons (inline SVG, stroke-based, 24px viewBox)
// ============================================================================

interface IconProps {
  size?: number;
  className?: string;
  style?: React.CSSProperties;
}

function icon(children: React.ReactNode) {
  return function Icon({ size = 14, className, style }: IconProps) {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={className}
        style={style}
        aria-hidden="true"
      >
        {children}
      </svg>
    );
  };
}

export const IconChevronDown = icon(<><path d="m6 9 6 6 6-6" /></>);export const IconChevronRight = icon(<><path d="m9 18 6-6-6-6" /></>);export const IconPlus = icon(<><><path d="M5 12h14" /><path d="M12 5v14" /></></>);export const IconTrash2 = icon(<>
  <path d="M3 6h18" /><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" /><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" /></>);export const IconCable = icon(<>
  <path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5a2.85 2.83 0 1 1-4-4Z" /><path d="m11 13 2 2" /><path d="m7 9 2 2" /></>);export const IconInfo = icon(<><><circle cx="12" cy="12" r="10" /><path d="M12 16v-4" /><path d="M12 8h.01" /></></>);export const IconSearch = icon(<><circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" /></>);export const IconAlertTriangle = icon(<>
  <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" /><path d="M12 9v4" /><path d="M12 17h.01" /></>);export const IconFolderOpen = icon(<>
  <path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2" /></>);export const IconUpload = icon(<>
  <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><path d="m17 8-5-5-5 5" /><path d="M12 3v12" /></>);export const IconFileUp = icon(<>
  <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" /><path d="M14 2v4a2 2 0 0 0 2 2h4" /><path d="M12 12v6" /><path d="m9 15 3-3 3 3" /></>);export const IconFileText = icon(<>
  <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" /><path d="M14 2v4a2 2 0 0 0 2 2h4" /><path d="M16 13H8" /><path d="M16 17H8" /><path d="M10 9H8" /></>);export const IconActivity = icon(<><path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.48 12H2" /></>);export const IconRefreshCw = icon(<>
  <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" /><path d="M21 3v5h-5" /><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" /><path d="M8 16H3v5" /></>);export const IconRadio = icon(<>
  <circle cx="12" cy="12" r="2" /><path d="M4.93 19.07a10 10 0 0 1 0-14.14" /><path d="M7.76 16.24a6 6 0 0 1 0-8.49" /><path d="M16.24 7.76a6 6 0 0 1 0 8.49" /><path d="M19.07 4.93a10 10 0 0 1 0 14.14" /></>);export const IconFileJson = icon(<>
  <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" /><path d="M14 2v4a2 2 0 0 0 2 2h4" /><path d="M10 12a1 1 0 0 0-1 1v1a1 1 0 0 1-1 1 1 1 0 0 1 1 1v1a1 1 0 0 0 1 1" /><path d="M14 18a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1 1 1 0 0 1-1-1v-1a1 1 0 0 0-1-1" /></>);export const IconCheckCircle2 = icon(<><circle cx="12" cy="12" r="10" /><path d="m9 12 2 2 4-4" /></>);export const IconXCircle = icon(<><><circle cx="12" cy="12" r="10" /><path d="m15 9-6 6" /><path d="m9 9 6 6" /></></>);export const IconTerminal = icon(<>
  <path d="m4 17 6-6-6-6" /><path d="M12 19h8" /></>);export const IconCopy = icon(<>
  <rect width="14" height="14" x="8" y="8" rx="2" ry="2" /><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" /></>);export const IconCheck = icon(<><path d="M20 6 9 17l-5-5" /></>);export const IconBraces = icon(<>
  <path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1" /><path d="M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1" /></>);export const IconUnplug = icon(<>
  <path d="m19 5 3-3" /><path d="m2 22 3-3" /><path d="M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z" /><path d="m7.5 13.5 2 2" /><path d="m10 11 2 2" /><path d="m14.5 8.5 2 2" /><path d="m18 3 1 1c2 2 2 4 0 6-1.5 1.5-4 1.5-5.5 0L10 14l-2-2 3.5-3.5c-1.5-1.5-1.5-4 0-5.5C13 2 15 2 17 4z" /></>);export const IconGlobe = icon(<>
  <circle cx="12" cy="12" r="10" /><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20" /><path d="M2 12h20" /></>);export const IconServer = icon(<>
  <rect width="20" height="8" x="2" y="2" rx="2" ry="2" /><rect width="20" height="8" x="2" y="14" rx="2" ry="2" /><path d="M6 6h.01" /><path d="M6 18h.01" /></>);export const IconClock = icon(<><circle cx="12" cy="12" r="10" /><path d="M12 6v6l4 2" /></>);export const IconX = icon(<><><path d="M18 6 6 18" /><path d="m6 6 12 12" /></></>);export const IconKey = icon(<>
  <path d="m21 2-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0 3 3L22 7l-3-3m-3.5 3.5L19 4" /></>);
// ============================================================================
// Form primitives
// ============================================================================

export function Label({
  htmlFor,
  className,
  children,
}: {
  htmlFor?: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <label htmlFor={htmlFor} className={cn('field-label', className)}>
      {children}
    </label>
  );
}

export function Input({
  className,
  ...rest
}: React.InputHTMLAttributes<HTMLInputElement>) {
  return <input className={cn('input', className)} {...rest} />;
}

export function Textarea({
  className,
  ...rest
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea className={cn('textarea', className)} {...rest} />;
}

export function Button({
  variant = 'outline',
  size,
  className,
  children,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'outline' | 'ghost';
  size?: 'sm' | 'icon';
}) {
  return (
    <button
      type="button"
      className={cn(
        'btn',
        variant === 'primary' && 'btn-primary',
        variant === 'ghost' && 'btn-ghost',
        size === 'sm' && 'btn-sm',
        size === 'icon' && 'btn-icon',
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Switch({
  checked,
  onCheckedChange,
  id,
  label,
}: {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  id?: string;
  label?: React.ReactNode;
}) {
  return (
    <label className="switch" htmlFor={id}>
      <input
        id={id}
        type="checkbox"
        checked={!!checked}
        onChange={(e) => onCheckedChange(e.target.checked)}
      />
      <span className="switch-track" />
      {label !== undefined && <span className="switch-label">{label}</span>}
    </label>
  );
}

/**
 * Native `<select>` styled to match the host's custom Select. API mirrors
 * the host SelectTrigger/SelectItem split for easy porting.
 */
export function Select({
  value,
  onValueChange,
  id,
  className,
  children,
  placeholder,
}: {
  value: string;
  onValueChange: (value: string) => void;
  id?: string;
  className?: string;
  children?: React.ReactNode;
  placeholder?: string;
}) {
  return (
    <select
      id={id}
      className={cn('select', className)}
      value={value}
      onChange={(e) => onValueChange(e.target.value)}
    >
      {placeholder !== undefined && (
        <option value="" disabled>
          {placeholder}
        </option>
      )}
      {children}
    </select>
  );
}

export function SelectItem({
  value,
  children,
  className,
}: {
  value: string;
  children?: React.ReactNode;
  className?: string;
}) {
  return (
    <option value={value} className={className}>
      {children}
    </option>
  );
}

export function Badge({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return <span className={cn('badge', className)}>{children}</span>;
}

export function Spinner({ className, size }: { className?: string; size?: number }) {
  return (
    <span
      className={cn('spinner', className)}
      style={size !== undefined ? { width: size, height: size } : undefined}
    />
  );
}

export function Empty({ children, sub }: { children: React.ReactNode; sub?: React.ReactNode }) {
  return (
    <div className="empty">
      {children}
      {sub !== undefined && <div className="empty-sub">{sub}</div>}
    </div>
  );
}

/** `{count}` placeholder interpolation helper (viewer common keys). */
export function fmt(template: string, params: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/g, (m, key: string) =>
    Object.prototype.hasOwnProperty.call(params, key) ? String(params[key]) : m,
  );
}
