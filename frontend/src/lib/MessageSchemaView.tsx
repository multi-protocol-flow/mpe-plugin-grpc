/**
 * MessageSchemaView — Recursive read-only protobuf schema tree viewer
 * (port of the host `src/components/panels/grpc/MessageSchemaView.tsx`
 * with local UI components and styles).
 *
 * Displays field definitions, nested structures, oneof groups, enums,
 * WKTs, circular references, and reserved fields from gRPC message
 * definitions returned by the plugin backend.
 */
import { useCallback, useMemo, useState } from 'react';
import type { GrpcFieldInfo, GrpcMessageInfo, GrpcOneofInfo } from '../types';
import { t } from '../i18n';
import { cn } from './cn';
import { IconChevronDown, IconChevronRight } from './ui';

// ============================================================================
// Types
// ============================================================================

interface MessageSchemaViewProps {
  /** The message definition to render */
  message: GrpcMessageInfo;
  /** Flat map of all known message definitions keyed by full_name (for fallback lookup) */
  messageDefinitions: Record<string, GrpcMessageInfo>;
  /** Current nesting depth (starts at 0) */
  depth?: number;
  /** Optional label like "Request" or "Response" shown next to the type name */
  label?: string;
}

// ============================================================================
// Helpers
// ============================================================================

/** Format [[1,3],[10,10]] → "1-3, 10" */
function formatReservedRanges(ranges: [number, number][]): string {
  return ranges
    .map(([start, end]) => (start === end ? String(start) : start + '-' + end))
    .join(', ');
}

/** Build a Set of all field numbers that belong to any oneof group */
function buildOneofFieldNumbers(oneofs: GrpcOneofInfo[]): Set<number> {
  const set = new Set<number>();
  for (const g of oneofs) {
    for (const n of g.field_numbers) set.add(n);
  }
  return set;
}

// ============================================================================
// Sub-components
// ============================================================================

/** Single field row rendered inside a message */
interface FieldRowProps {
  field: GrpcFieldInfo;
  messageDefinitions: Record<string, GrpcMessageInfo>;
  depth: number;
}

function FieldRow({ field, messageDefinitions, depth }: FieldRowProps) {
  const [expanded, setExpanded] = useState(false);

  const handleToggle = useCallback(() => setExpanded((p) => !p), []);

  // Resolve the nested message to display when this field is expanded:
  //  1. Inline nested_message from the backend
  //  2. Fallback: look up by type_full_name in the flat definitions map
  const resolvedNested = useMemo<GrpcMessageInfo | null>(() => {
    if (field.nested_message) return field.nested_message;
    if (
      field.type_kind === 'message' &&
      field.type_full_name &&
      messageDefinitions[field.type_full_name]
    ) {
      return messageDefinitions[field.type_full_name];
    }
    return null;
  }, [field, messageDefinitions]);

  const canExpand =
    field.type_kind === 'message' && resolvedNested !== null && !resolvedNested.is_wkt;

  return (
    <div>
      <div className="schema-row">
        {/* Expand chevron for nested message fields */}
        {canExpand ? (
          <button
            type="button"
            onClick={handleToggle}
            className="schema-toggle"
            title={expanded ? t('panels.grpcConnect.collapseType') : t('panels.grpcConnect.expandType')}
          >
            {expanded ? (
              <IconChevronDown size={12} />
            ) : (
              <IconChevronRight size={12} />
            )}
          </button>
        ) : (
          <span className="schema-toggle-spacer" />
        )}

        {/* Type display — clickable for expandable message types */}
        {canExpand ? (
          <button type="button" onClick={handleToggle} className="schema-type-btn">
            {field.type_display}
          </button>
        ) : (
          <span className="schema-type">{field.type_display}</span>
        )}

        {/* Field name */}
        <span className="schema-name">{field.name}</span>

        {/* Field number */}
        <span className="schema-num">= {field.number}</span>

        {/* Enum values inline */}
        {field.type_kind === 'enum' &&
          field.enum_values.length > 0 && (
            <span className="schema-enum">
              // (
              {field.enum_values.map((v) => v.name + '=' + v.number).join(', ')}
              )
            </span>
          )}

        {/* Circular reference indicator */}
        {field.type_kind === 'circular_ref' && (
          <span className="schema-circular">({t('panels.grpcConnect.circularReference')})</span>
        )}

        {/* Cardinality label badge */}
        {(field.label === 'repeated' || field.label === 'optional') && !field.is_map && (
          <span className="schema-label-badge">{field.label}</span>
        )}

        {/* WKT badge for well-known types inlined as fields */}
        {field.type_kind === 'message' &&
          !canExpand &&
          field.type_full_name !== undefined &&
          field.type_full_name.startsWith('google.protobuf.') && (
            <span className="schema-wkt">WKT</span>
          )}
      </div>

      {/* Expanded nested message */}
      {canExpand && expanded && resolvedNested && (
        <MessageSchemaView
          message={resolvedNested}
          messageDefinitions={messageDefinitions}
          depth={depth + 1}
        />
      )}
    </div>
  );
}

/** Oneof group with a bordered container */
interface OneofGroupViewProps {
  group: GrpcOneofInfo;
  allFields: GrpcFieldInfo[];
  messageDefinitions: Record<string, GrpcMessageInfo>;
  depth: number;
}

function OneofGroupView({
  group,
  allFields,
  messageDefinitions,
  depth,
}: OneofGroupViewProps) {
  const [expanded, setExpanded] = useState(true);

  const handleToggle = useCallback(() => setExpanded((p) => !p), []);

  const groupFields = useMemo(
    () => allFields.filter((f) => group.field_numbers.includes(f.number)),
    [allFields, group.field_numbers],
  );

  if (groupFields.length === 0) return null;

  return (
    <div className="schema-oneof">
      <button type="button" onClick={handleToggle} className="schema-oneof-head">
        {expanded ? (
          <IconChevronDown size={12} />
        ) : (
          <IconChevronRight size={12} />
        )}
        <span className="schema-oneof-label">{t('panels.grpcConnect.oneofGroup', { group: group.name })}</span>
      </button>
      {expanded && (
        <div>
          {groupFields.map((field) => (
            <FieldRow
              key={field.number}
              field={field}
              messageDefinitions={messageDefinitions}
              depth={depth}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/** Reserved ranges and names display */
function ReservedFieldsView({
  reservedRanges,
  reservedNames,
}: {
  reservedRanges?: [number, number][];
  reservedNames?: string[];
}) {
  const parts: string[] = [];

  if (reservedRanges && reservedRanges.length > 0) {
    parts.push(t('panels.grpcConnect.reserved', { ranges: formatReservedRanges(reservedRanges) }));
  }

  if (reservedNames && reservedNames.length > 0) {
    parts.push(t('panels.grpcConnect.reservedNames', { names: reservedNames.join(', ') }));
  }

  if (parts.length === 0) return null;

  return (
    <div className="schema-reserved">{parts.join('; ')}</div>
  );
}

// ============================================================================
// Main component
// ============================================================================

export function MessageSchemaView({
  message,
  messageDefinitions,
  depth = 0,
  label,
}: MessageSchemaViewProps) {
  const [expanded, setExpanded] = useState(depth < 2);

  const handleToggle = useCallback(() => setExpanded((p) => !p), []);

  // ── 1. WKT — collapsed single line ──────────────────────────────────
  if (message.is_wkt) {
    return (
      <div className={cn('schema-row', depth > 0 && 'schema-pl')}>
        <span className="schema-name">{message.full_name}</span>
        <span className="schema-wkt">WKT</span>
      </div>
    );
  }

  // ── 2. Empty message ────────────────────────────────────────────────
  if (
    message.fields.length === 0 &&
    message.oneof_groups.length === 0 &&
    (!message.reserved_ranges || message.reserved_ranges.length === 0) &&
    (!message.reserved_names || message.reserved_names.length === 0)
  ) {
    return (
      <div className={cn('schema-row', depth > 0 && 'schema-pl')}>
        {depth > 0 && <span className="schema-toggle-spacer" />}
        <span className="schema-name">{message.full_name}</span>
        <span className="muted italic">{t('panels.grpcConnect.emptyMessage')}</span>
      </div>
    );
  }

  // ── 3. Max depth reached ────────────────────────────────────────────
  if (depth >= 5) {
    return (
      <div className={cn('schema-row', depth > 0 && 'schema-pl')}>
        {depth > 0 && <span className="schema-toggle-spacer" />}
        <span className="schema-name">{message.full_name}</span>
        <span className="muted">... {t('panels.grpcConnect.maxDepth')}</span>
      </div>
    );
  }

  // ── 4. Full rendering ───────────────────────────────────────────────
  const oneofFieldNumbers = useMemo(
    () => buildOneofFieldNumbers(message.oneof_groups),
    [message.oneof_groups],
  );

  const regularFields = useMemo(
    () => message.fields.filter((f) => !oneofFieldNumbers.has(f.number)),
    [message.fields, oneofFieldNumbers],
  );

  return (
    <div className={cn(depth > 0 && 'schema-pl')}>
      {/* Collapsible header */}
      <button type="button" onClick={handleToggle} className="schema-msg-head">
        {expanded ? (
          <IconChevronDown size={14} />
        ) : (
          <IconChevronRight size={14} />
        )}
        <span className="schema-msg-name">{message.full_name}</span>
        {label && (
          <span className="badge badge-violet">{label}</span>
        )}
      </button>

      {/* Expanded body */}
      {expanded && (
        <div>
          {/* Regular fields (not part of any oneof group) */}
          {regularFields.map((field) => (
            <FieldRow
              key={field.number}
              field={field}
              messageDefinitions={messageDefinitions}
              depth={depth}
            />
          ))}

          {/* Oneof groups */}
          {message.oneof_groups.map((group) => (
            <OneofGroupView
              key={group.name}
              group={group}
              allFields={message.fields}
              messageDefinitions={messageDefinitions}
              depth={depth}
            />
          ))}

          {/* Reserved fields */}
          <ReservedFieldsView
            reservedRanges={message.reserved_ranges}
            reservedNames={message.reserved_names}
          />
        </div>
      )}
    </div>
  );
}
