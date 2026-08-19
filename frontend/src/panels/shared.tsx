/**
 * Shared panel helpers: metadata entry list editor, streaming badge,
 * error normalization.
 */
import { useCallback } from 'react';
import type { GrpcMetadataEntry, GrpcMethodInfo } from '../types';
import { t } from '../i18n';
import { Button, Empty, IconPlus, IconTrash2, Input } from '../lib/ui';

/** Normalize a thrown error into a readable string. */
export function errorMessage(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

/**
 * Key/Value metadata list editor (used by connect default/reflection metadata
 * and call-level metadata).
 */
export function MetadataListEditor({
  entries,
  onChange,
  emptyText,
  hint,
}: {
  entries: GrpcMetadataEntry[];
  onChange: (entries: GrpcMetadataEntry[]) => void;
  emptyText: string;
  hint?: string;
}) {
  const addEntry = useCallback(() => {
    onChange([...entries, { key: '', value: '' }]);
  }, [entries, onChange]);

  const removeEntry = useCallback((index: number) => {
    onChange(entries.filter((_, i) => i !== index));
  }, [entries, onChange]);

  const updateEntry = useCallback((index: number, patch: Partial<GrpcMetadataEntry>) => {
    onChange(entries.map((entry, i) => (i === index ? { ...entry, ...patch } : entry)));
  }, [entries, onChange]);

  return (
    <div className="col">
      <div className="row-between">
        <span />
        <Button variant="ghost" size="sm" onClick={addEntry} className="btn-violet">
          <IconPlus size={12} />
          {t('panels.common.add')}
        </Button>
      </div>
      {entries.length === 0 ? (
        <Empty>
          {emptyText}
          {hint && <div className="empty-sub">{hint}</div>}
        </Empty>
      ) : (
        <div className="col">
          {entries.map((entry, index) => (
            <div key={index} className="kv-row">
              <Input
                value={entry.key}
                onChange={(e) => updateEntry(index, { key: e.target.value })}
                placeholder="Key"
              />
              <Input
                value={entry.value}
                onChange={(e) => updateEntry(index, { value: e.target.value })}
                placeholder="Value"
              />
              <Button
                variant="ghost"
                size="icon"
                onClick={() => removeEntry(index)}
                className="btn-rose"
              >
                <IconTrash2 size={14} />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Streaming pattern badge for a method. */
export function StreamingBadge({ method }: { method: GrpcMethodInfo }) {
  if (method.is_server_streaming && method.is_client_streaming) {
    return <span className="badge badge-purple">{t('panels.grpcConnect.bidiStreaming')}</span>;
  }
  if (method.is_server_streaming) {
    return <span className="badge badge-amber">{t('panels.grpcConnect.serverStreaming')}</span>;
  }
  if (method.is_client_streaming) {
    return <span className="badge badge-blue">{t('panels.grpcConnect.clientStreaming')}</span>;
  }
  return <span className="badge badge-green">{t('panels.grpcConnect.unary')}</span>;
}

/** String list editor (load-balancing endpoints). */
export function StringListEditor({
  items,
  onChange,
  emptyText,
  placeholder,
  addLabel,
}: {
  items: string[];
  onChange: (items: string[]) => void;
  emptyText: string;
  placeholder: string;
  addLabel: string;
}) {
  const addItem = useCallback(() => {
    onChange([...items, '']);
  }, [items, onChange]);

  const removeItem = useCallback((index: number) => {
    onChange(items.filter((_, i) => i !== index));
  }, [items, onChange]);

  const updateItem = useCallback((index: number, value: string) => {
    onChange(items.map((item, i) => (i === index ? value : item)));
  }, [items, onChange]);

  return (
    <div className="col">
      <div className="row-between">
        <span />
        <Button variant="ghost" size="sm" onClick={addItem} className="btn-violet">
          <IconPlus size={12} />
          {addLabel}
        </Button>
      </div>
      {items.length === 0 ? (
        <Empty>{emptyText}</Empty>
      ) : (
        <div className="col">
          {items.map((item, index) => (
            <div key={index} className="kv-row">
              <Input
                value={item}
                onChange={(e) => updateItem(index, e.target.value)}
                placeholder={placeholder}
                className="mono grow"
              />
              <Button
                variant="ghost"
                size="icon"
                onClick={() => removeItem(index)}
                className="btn-rose"
              >
                <IconTrash2 size={14} />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
