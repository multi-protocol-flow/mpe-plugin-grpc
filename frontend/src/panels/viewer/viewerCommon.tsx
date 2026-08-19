/**
 * Shared report-viewer primitives: JSON renderer, variable-changes card,
 * collapsible section, key/value list, status badge. Ported from the host
 * GrpcViewer's building blocks with local styles/i18n.
 */
import React, { useState } from 'react';
import type { VariableChange } from '../../types';
import { t } from '../../i18n';
import { IconChevronDown, IconChevronRight } from '../../lib/ui';

// ============================================================================
// Raw JSON viewer (syntax-highlighted <pre>)
// ============================================================================

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function highlightJson(value: unknown, depth: number): string {
  if (value === null) {
    return '<span class="json-null">null</span>';
  }
  if (typeof value === 'string') {
    return '<span class="json-str">' + escapeHtml(JSON.stringify(value)) + '</span>';
  }
  if (typeof value === 'number') {
    return '<span class="json-num">' + value + '</span>';
  }
  if (typeof value === 'boolean') {
    return '<span class="json-bool">' + value + '</span>';
  }
  if (Array.isArray(value)) {
    if (value.length === 0) return '[]';
    const items = value.map((v) => highlightJson(v, depth + 1));
    const indent = '  '.repeat(depth + 1);
    const closeIndent = '  '.repeat(depth);
    return (
      '[\n' +
      items.map((item) => indent + item).join(',\n') +
      '\n' +
      closeIndent +
      ']'
    );
  }
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) return '{}';
    const indent = '  '.repeat(depth + 1);
    const closeIndent = '  '.repeat(depth);
    const lines = entries.map(([k, v]) => {
      const key = '<span class="json-key">' + escapeHtml(JSON.stringify(k)) + '</span>';
      return indent + key + ': ' + highlightJson(v, depth + 1);
    });
    return '{\n' + lines.join(',\n') + '\n' + closeIndent + '}';
  }
  return escapeHtml(String(value));
}

/** Render any value as syntax-highlighted JSON. */
export function RawJsonViewer({ data }: { data: unknown }) {
  if (data === undefined || data === null) {
    return <pre className="json-pre muted italic">{t('viewer.common.no_data')}</pre>;
  }
  // Pre-rendered JSON string (request body etc.) — parse first
  let parsed: unknown = data;
  if (typeof data === 'string') {
    try {
      parsed = JSON.parse(data);
    } catch {
      parsed = data;
    }
  }
  // dangerouslySetInnerHTML is deliberate: the HTML is produced from
  // JSON-escaped values only (no raw user HTML can leak in).
  const html = highlightJson(parsed, 0);
  return <pre className="json-pre" dangerouslySetInnerHTML={{ __html: html }} />;
}

// ============================================================================
// Collapsible section
// ============================================================================

interface CollapsibleSectionProps {
  title: string;
  icon?: React.ReactNode;
  defaultOpen?: boolean;
  extra?: React.ReactNode;
  children: React.ReactNode;
}

export function CollapsibleSection({
  title,
  icon,
  defaultOpen = false,
  extra,
  children,
}: CollapsibleSectionProps) {
  const [isOpen, setIsOpen] = useState(defaultOpen);

  return (
    <div className="vsection">
      <div className="vsection-head">
        <button
          type="button"
          onClick={() => setIsOpen((p) => !p)}
          className="vsection-toggle"
        >
          <span className="vsection-head-icon">
            {isOpen ? <IconChevronDown size={15} /> : <IconChevronRight size={15} />}
          </span>
          {icon && <span className="vsection-head-icon">{icon}</span>}
          <span>{title}</span>
        </button>
        {extra && <div className="vsection-extra">{extra}</div>}
      </div>
      {isOpen && <div className="vsection-body">{children}</div>}
    </div>
  );
}

// ============================================================================
// Key/Value list
// ============================================================================

export function KeyValueList({ data }: { data: Record<string, unknown> | undefined | null }) {
  if (!data || Object.keys(data).length === 0) {
    return <p className="muted italic">{t('viewer.common.no_data')}</p>;
  }

  return (
    <div className="kv-list">
      {Object.entries(data).map(([key, value]) => (
        <div key={key} className="kv-item">
          <span className="kv-key">{key}:</span>
          <span className="kv-value">
            {typeof value === 'object' ? JSON.stringify(value) : String(value)}
          </span>
        </div>
      ))}
    </div>
  );
}

// ============================================================================
// Status badge
// ============================================================================

export function StatusBadge({
  isSuccess,
  successText,
  failText,
}: {
  isSuccess: boolean;
  successText: string;
  failText: string;
}) {
  return (
    <span className={'status-badge ' + (isSuccess ? 'status-ok' : 'status-err')}>
      {isSuccess ? successText : failText}
    </span>
  );
}

// ============================================================================
// Variable changes card
// ============================================================================

export function VariableChangesCard({
  changes,
}: {
  changes?: VariableChange[] | Record<string, unknown>[];
}) {
  if (!changes || changes.length === 0) return null;

  return (
    <CollapsibleSection
      title={t('viewer.common.variable_changes') + ' (' + changes.length + ')'}
      defaultOpen={true}
    >
      <div className="col">
        {changes.map((change, idx) => {
          const name = typeof (change as VariableChange).name === 'string'
            ? (change as VariableChange).name
            : 'var-' + idx;
          const before = (change as VariableChange).before;
          const after = (change as VariableChange).after;
          return (
            <div key={idx} className="var-change-item">
              <span className="var-change-name">{name}</span>
              <span className="var-change-value">
                {before !== undefined
                  ? String(typeof before === 'object' ? JSON.stringify(before) : before)
                  : ''}
                {' → '}
                {after !== undefined
                  ? String(typeof after === 'object' ? JSON.stringify(after) : after)
                  : ''}
              </span>
            </div>
          );
        })}
      </div>
    </CollapsibleSection>
  );
}

// ============================================================================
// Timing breakdown grid
// ============================================================================

export function TimingItem({ label, valueMs }: { label: string; valueMs: number }) {
  return (
    <div className="timing-item">
      <span className="timing-label">{label}</span>
      <span className="timing-value">{valueMs}ms</span>
    </div>
  );
}
