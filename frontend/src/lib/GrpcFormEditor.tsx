/**
 * gRPC 请求表单编辑器 (port of the host `GrpcFormEditor.tsx`)
 *
 * 根据 proto schema 渲染表单字段，支持 JSON ↔ Form 双向同步。
 * 标量字段渲染为对应输入控件，嵌套消息保持为 JSON 子对象。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  ProtoFieldDescriptor,
  ProtoMessageDescriptor,
  ProtoParseResult,
} from './protoFieldParser';
import { fieldDefaultValue, findMessageDescriptor } from './protoFieldParser';
import { t } from '../i18n';
import {
  Button,
  IconChevronDown,
  IconChevronRight,
  IconPlus,
  IconTrash2,
  Input,
  Label,
  Select,
  SelectItem,
  Textarea,
} from './ui';

// ============================================================================
// 类型
// ============================================================================

interface GrpcFormEditorProps {
  /** 当前 request_json 值 */
  value: string;
  /** 值变更回调 */
  onChange: (json: string) => void;
  /** 解析后的 proto schema */
  parseResult: ProtoParseResult | null;
  /** 选中方法的 input_type 全名 */
  inputType: string | null;
  /** 失焦回调 */
  onBlur?: () => void;
}

/** 表单字段状态 */
interface FieldState {
  /** 当前值的 JSON 表示 */
  value: unknown;
}

// ============================================================================
// 工具函数
// ============================================================================

/** 类型提示标签 */
function typeHintLabel(field: ProtoFieldDescriptor): string {
  if (field.typeCategory === 'map') {
    return 'map<' + field.mapKeyType + ', ' + field.mapValueType + '>';
  }
  let hint = field.rawType;
  if (field.cardinality === 'repeated') {
    hint = 'repeated ' + hint;
  }
  return hint;
}

/** 解析 JSON，失败返回 null */
function safeParseJson(json: string): Record<string, unknown> | null {
  try {
    const parsed = JSON.parse(json);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
    return null;
  } catch {
    return null;
  }
}

/** 根据字段值推断 enum 名称（从数字映射） */
function resolveEnumName(field: ProtoFieldDescriptor, raw: unknown): string {
  if (typeof raw === 'string') return raw;
  if (typeof raw === 'number' && field.enumNumberMap) {
    const entry = Object.entries(field.enumNumberMap).find(([, v]) => v === raw);
    if (entry) return entry[0];
  }
  // 默认值
  if (field.enumValues && field.enumValues.length > 0) {
    return field.enumValues[0];
  }
  return '0';
}

// ============================================================================
// 组件：单个标量字段编辑器
// ============================================================================

interface ScalarFieldEditorProps {
  field: ProtoFieldDescriptor;
  value: unknown;
  onChange: (val: unknown) => void;
  onBlur?: () => void;
}

function ScalarFieldEditor({ field, value, onChange, onBlur }: ScalarFieldEditorProps) {
  const handleNumberChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const v = e.target.value;
    if (v === '' || v === '-') {
      onChange(v);
      return;
    }
    const num = Number(v);
    if (!isNaN(num)) {
      onChange(num);
    }
  }, [onChange]);

  // 枚举字段 -> 下拉选择
  if (field.typeCategory === 'enum' && field.enumValues && field.enumValues.length > 0) {
    const currentVal = resolveEnumName(field, value);
    return (
      <div className="col">
        <Select
          value={currentVal}
          onValueChange={(v) => onChange(v)}
          className="mono"
        >
          {field.enumValues.map((name) => (
            <SelectItem key={name} value={name}>
              {name}
              {field.enumNumberMap && field.enumNumberMap[name] !== undefined
                ? ' (' + field.enumNumberMap[name] + ')'
                : ''}
            </SelectItem>
          ))}
        </Select>
      </div>
    );
  }

  // 布尔 -> 下拉选择
  if (field.typeCategory === 'bool') {
    return (
      <div className="col">
        <Select
          value={value === true ? 'true' : value === false ? 'false' : 'false'}
          onValueChange={(v) => onChange(v === 'true')}
          className="mono"
        >
          <SelectItem value="true">true</SelectItem>
          <SelectItem value="false">false</SelectItem>
        </Select>
      </div>
    );
  }

  // 数值类型
  if (['int', 'uint', 'sint', 'fixed', 'sfixed', 'float', 'double'].includes(field.typeCategory)) {
    return (
      <div className="col">
        <Input
          type="number"
          value={value === undefined || value === null || value === '' ? '' : String(value)}
          onChange={handleNumberChange}
          onBlur={onBlur}
          placeholder="0"
          className="mono"
        />
      </div>
    );
  }

  // 字符串 / bytes / unknown -> 文本输入
  return (
    <div className="col">
      <Input
        type="text"
        value={typeof value === 'string' ? value : value === undefined || value === null ? '' : String(value)}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
        placeholder={field.comment || ''}
        className="mono"
      />
    </div>
  );
}

// ============================================================================
// 组件：嵌套消息字段（折叠式 JSON 编辑器）
// ============================================================================

interface NestedMessageEditorProps {
  field: ProtoFieldDescriptor;
  value: unknown;
  onChange: (val: unknown) => void;
  onBlur?: () => void;
}

function NestedMessageEditor({ field, value, onChange, onBlur }: NestedMessageEditorProps) {
  const [expanded, setExpanded] = useState(false);
  const jsonStr = useMemo(() => {
    if (value === undefined || value === null) return '{}';
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return '{}';
    }
  }, [value]);

  const handleChange = useCallback((newJson: string) => {
    try {
      const parsed = JSON.parse(newJson);
      onChange(parsed);
    } catch {
      // 保持原样，用户还在编辑
    }
  }, [onChange]);

  return (
    <div className="nested-msg">
      <button
        type="button"
        onClick={() => setExpanded((prev) => !prev)}
        className="nested-msg-head"
      >
        {expanded ? <IconChevronDown size={12} /> : <IconChevronRight size={12} />}
        <span className="mono" style={{ fontWeight: 500 }}>{field.name}</span>
        <span className="muted" style={{ fontWeight: 400 }}>
          ({field.messageName || field.rawType})
        </span>
      </button>
      {expanded && (
        <div className="nested-msg-body">
          <Textarea
            value={jsonStr}
            onChange={(e) => handleChange(e.target.value)}
            onBlur={onBlur}
            rows={4}
          />
        </div>
      )}
    </div>
  );
}

// ============================================================================
// 组件：repeated 字段编辑器
// ============================================================================

interface RepeatedFieldEditorProps {
  field: ProtoFieldDescriptor;
  value: unknown;
  onChange: (val: unknown) => void;
  onBlur?: () => void;
}

function RepeatedFieldEditor({ field, value, onChange, onBlur }: RepeatedFieldEditorProps) {
  const items = useMemo(() => {
    if (Array.isArray(value)) return value;
    return [];
  }, [value]);

  const handleAdd = useCallback(() => {
    const newItem =
      field.typeCategory === 'message'
        ? {}
        : field.typeCategory === 'enum'
          ? (field.enumValues?.[0] ?? 0)
          : field.typeCategory === 'bool'
            ? false
            : field.typeCategory === 'string' || field.typeCategory === 'bytes'
              ? ''
              : 0;
    onChange([...items, newItem]);
  }, [items, field, onChange]);

  const handleRemove = useCallback((index: number) => {
    const next = [...items];
    next.splice(index, 1);
    onChange(next);
  }, [items, onChange]);

  const handleItemChange = useCallback((index: number, val: unknown) => {
    const next = [...items];
    next[index] = val;
    onChange(next);
  }, [items, onChange]);

  const itemField = useMemo(
    (): ProtoFieldDescriptor => ({
      ...field,
      cardinality: 'singular',
      rawType: field.typeCategory === 'map' ? (field.mapValueType || 'string') : field.rawType,
      typeCategory: field.typeCategory === 'map' ? 'string' : field.typeCategory,
    }),
    [field],
  );

  return (
    <div className="col">
      <div className="list-head">
        <span className="field-label-sm">{t('panels.grpcCall.form_element_count', { count: items.length })}</span>
        <Button variant="ghost" size="sm" onClick={handleAdd} className="btn-violet">
          <IconPlus size={12} />
          {t('panels.common.add')}
        </Button>
      </div>
      {items.length === 0 ? (
        <div className="empty">{t('panels.grpcCall.form_empty_list')}</div>
      ) : (
        <div className="col">
          {items.map((item, idx) => (
            <div key={idx} className="list-item">
              <span className="list-index">{idx}</span>
              {field.typeCategory === 'message' ? (
                <div className="grow">
                  <NestedMessageEditor
                    field={{ ...field, cardinality: 'singular', name: '[' + idx + ']' }}
                    value={item}
                    onChange={(val) => handleItemChange(idx, val)}
                    onBlur={onBlur}
                  />
                </div>
              ) : (
                <div className="grow">
                  <ScalarFieldEditor
                    field={itemField}
                    value={item}
                    onChange={(val) => handleItemChange(idx, val)}
                    onBlur={onBlur}
                  />
                </div>
              )}
              <Button
                variant="ghost"
                size="icon"
                onClick={() => handleRemove(idx)}
                className="btn-rose"
              >
                <IconTrash2 size={12} />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ============================================================================
// 组件：map 字段编辑器
// ============================================================================

interface MapFieldEditorProps {
  field: ProtoFieldDescriptor;
  value: unknown;
  onChange: (val: unknown) => void;
  onBlur?: () => void;
}

function MapFieldEditor({ value, onChange, onBlur }: MapFieldEditorProps) {
  const entries = useMemo(() => {
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      return Object.entries(value as Record<string, unknown>);
    }
    return [] as [string, unknown][];
  }, [value]);

  const handleAdd = useCallback(() => {
    const current =
      value && typeof value === 'object' && !Array.isArray(value)
        ? { ...(value as Record<string, unknown>) }
        : {};
    const newKey = 'key_' + entries.length;
    current[newKey] = '';
    onChange(current);
  }, [value, entries.length, onChange]);

  const handleRemove = useCallback((key: string) => {
    const current = { ...(value as Record<string, unknown>) };
    delete current[key];
    onChange(current);
  }, [value, onChange]);

  const handleKeyChange = useCallback((oldKey: string, newKey: string) => {
    const current = { ...(value as Record<string, unknown>) };
    const val = current[oldKey];
    delete current[oldKey];
    current[newKey] = val;
    onChange(current);
  }, [value, onChange]);

  const handleValueChange = useCallback((key: string, val: unknown) => {
    const current = { ...(value as Record<string, unknown>) };
    current[key] = val;
    onChange(current);
  }, [value, onChange]);

  return (
    <div className="col">
      <div className="list-head">
        <span className="field-label-sm">{t('panels.grpcCall.form_entry_count', { count: entries.length })}</span>
        <Button variant="ghost" size="sm" onClick={handleAdd} className="btn-violet">
          <IconPlus size={12} />
          {t('panels.common.add')}
        </Button>
      </div>
      {entries.length === 0 ? (
        <div className="empty">{t('panels.grpcCall.form_empty_map')}</div>
      ) : (
        <div className="col">
          {entries.map(([key, val]) => (
            <div key={key} className="list-item">
              <Input
                type="text"
                value={key}
                onChange={(e) => handleKeyChange(key, e.target.value)}
                onBlur={onBlur}
                placeholder="Key"
                className="mono"
                style={{ width: '33%' }}
              />
              <Input
                type="text"
                value={typeof val === 'string' ? val : JSON.stringify(val)}
                onChange={(e) => handleValueChange(key, e.target.value)}
                onBlur={onBlur}
                placeholder="Value"
                className="mono grow"
              />
              <Button
                variant="ghost"
                size="icon"
                onClick={() => handleRemove(key)}
                className="btn-rose"
              >
                <IconTrash2 size={12} />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ============================================================================
// 主组件
// ============================================================================

export function GrpcFormEditor({ value, onChange, parseResult, inputType, onBlur }: GrpcFormEditorProps) {
  // 查找当前方法的 input message 描述
  const messageDesc = useMemo((): ProtoMessageDescriptor | null => {
    if (!parseResult || !inputType) return null;
    return findMessageDescriptor(parseResult, inputType);
  }, [parseResult, inputType]);

  // 解析当前 JSON 值为字段状态
  const [fieldStates, setFieldStates] = useState<Record<string, FieldState>>({});
  const isInitializedRef = useRef(false);

  // 从 JSON 初始化字段状态
  useEffect(() => {
    const parsed = safeParseJson(value);
    if (!parsed || !messageDesc) {
      isInitializedRef.current = false;
      return;
    }

    // 仅在首次或 messageDesc 变化时初始化
    if (!isInitializedRef.current) {
      const states: Record<string, FieldState> = {};
      for (const field of messageDesc.fields) {
        states[field.name] = {
          value: parsed[field.name] ?? fieldDefaultValue(field),
        };
      }
      setFieldStates(states);
      isInitializedRef.current = true;
    }
  }, [value, messageDesc]);

  // 当 messageDesc 变化时重置初始化标记
  useEffect(() => {
    isInitializedRef.current = false;
  }, [messageDesc]);

  // 字段值变更 -> 序列化为 JSON
  const handleFieldChange = useCallback((fieldName: string, val: unknown) => {
    setFieldStates((prev) => {
      const next = { ...prev, [fieldName]: { value: val } };

      // 构建完整 JSON 对象
      const jsonObj: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(next)) {
        jsonObj[k] = v.value;
      }

      // 序列化并通知父组件
      try {
        const jsonStr = JSON.stringify(jsonObj, null, 2);
        onChange(jsonStr);
      } catch {
        // 序列化失败，静默处理
      }

      return next;
    });
  }, [onChange]);

  // 当外部 value 变化（非自身触发），同步字段状态
  const prevValueRef = useRef(value);
  useEffect(() => {
    if (prevValueRef.current === value) return;
    prevValueRef.current = value;

    if (!messageDesc) return;
    const parsed = safeParseJson(value);
    if (!parsed) return;

    const states: Record<string, FieldState> = {};
    for (const field of messageDesc.fields) {
      states[field.name] = {
        value: parsed[field.name] ?? fieldDefaultValue(field),
      };
    }
    setFieldStates(states);
    isInitializedRef.current = true;
  }, [value, messageDesc]);

  // 没有消息描述时显示提示
  if (!messageDesc) {
    return (
      <div className="empty">
        <p>
          {parseResult
            ? t('panels.grpcCall.form_no_message_def', { type: inputType || '' })
            : t('panels.grpcCall.form_no_proto_schema')}
        </p>
      </div>
    );
  }

  return (
    <div className="form-editor">
      {/* 消息类型提示 */}
      <div className="form-msg-type">
        <span>{t('panels.grpcCall.form_message_type_label')}</span>
        <span className="form-type-chip">{messageDesc.fullName}</span>
        <span className="soft">{t('panels.grpcCall.form_field_count', { count: messageDesc.fields.length })}</span>
      </div>

      {/* 字段列表 */}
      <div className="col">
        {messageDesc.fields.map((field) => {
          const state = fieldStates[field.name];
          const currentValue = state?.value ?? fieldDefaultValue(field);

          return (
            <div key={field.name} className="form-field">
              {/* 字段标签 */}
              <div className="form-field-head">
                <Label className="form-field-name">{field.name}</Label>
                <span className="form-field-type">{typeHintLabel(field)}</span>
                {field.comment && (
                  <span className="form-field-comment">{field.comment}</span>
                )}
              </div>

              {/* 字段编辑器 */}
              {field.typeCategory === 'map' ? (
                <MapFieldEditor
                  field={field}
                  value={currentValue}
                  onChange={(val) => handleFieldChange(field.name, val)}
                  onBlur={onBlur}
                />
              ) : field.cardinality === 'repeated' ? (
                <RepeatedFieldEditor
                  field={field}
                  value={currentValue}
                  onChange={(val) => handleFieldChange(field.name, val)}
                  onBlur={onBlur}
                />
              ) : field.typeCategory === 'message' ? (
                <NestedMessageEditor
                  field={field}
                  value={currentValue}
                  onChange={(val) => handleFieldChange(field.name, val)}
                  onBlur={onBlur}
                />
              ) : (
                <ScalarFieldEditor
                  field={field}
                  value={currentValue}
                  onChange={(val) => handleFieldChange(field.name, val)}
                  onBlur={onBlur}
                />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
