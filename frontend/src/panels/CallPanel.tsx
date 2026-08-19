/**
 * gRPC Call node config panel.
 * Port of the host `GrpcCallConfig.tsx`: connection dropdown sourced from the
 * init.nodes snapshot (filtered `grpc:connect`), service/method selection with
 * manual mode, skeleton generation + debounced validation via uiCall,
 * grpcurl export, JSON/Form dual editing, streaming request_messages.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  GrpcCallConfig,
  GrpcConnectConfig,
  GrpcMessageInfo,
  GrpcMethodInfo,
  GrpcNodeSnapshot,
  GrpcServiceInfo,
  SkeletonResult,
  StreamMessage,
  ValidateResult,
  ValidationIssue,
} from '../types';
import { t } from '../i18n';
import {
  Badge,
  Button,
  Empty,
  IconBraces,
  IconCheck,
  IconCheckCircle2,
  IconChevronDown,
  IconCopy,
  IconFileJson,
  IconFileText,
  IconInfo,
  IconPlus,
  IconRadio,
  IconTerminal,
  IconTrash2,
  IconXCircle,
  Input,
  Label,
  Select,
  SelectItem,
  Spinner,
  Textarea,
} from '../lib/ui';
import { GrpcFormEditor } from '../lib/GrpcFormEditor';
import { MessageSchemaView } from '../lib/MessageSchemaView';
import { parseProtoFiles, findMessageDescriptor } from '../lib/protoFieldParser';
import type { ProtoParseResult } from '../lib/protoFieldParser';
import {
  extractGrpcurlParamsFromConnectConfig,
  generateGrpcurlCommand,
} from '../lib/grpcurlGenerator';
import { MetadataListEditor, StreamingBadge } from './shared';
import { uiCall } from '../bridge';

interface CallPanelProps {
  config: GrpcCallConfig;
  nodes: GrpcNodeSnapshot[];
  onChange: (config: GrpcCallConfig) => void;
}

function isGrpcConnectConfig(config: unknown): config is GrpcConnectConfig {
  return (
    config !== null &&
    typeof config === 'object' &&
    typeof (config as GrpcConnectConfig).url === 'string'
  );
}

export function CallPanel({ config, nodes, onChange }: CallPanelProps) {
  const set = useCallback(
    (patch: Partial<GrpcCallConfig>) => {
      onChange({ ...config, ...patch });
    },
    [config, onChange],
  );

  // ---- connections (from the init.nodes snapshot) -------------------------
  const availableConnections = useMemo(() => {
    return nodes
      .filter((n) => n.type === 'grpc:connect' && isGrpcConnectConfig(n.config))
      .map((n) => ({ uuid: n.uuid, name: n.label, config: n.config as unknown as GrpcConnectConfig }));
  }, [nodes]);

  const currentConnectionId = config.connection_id;

  const connectConfig = useMemo(() => {
    return availableConnections.find((c) => c.uuid === currentConnectionId)?.config ?? null;
  }, [availableConnections, currentConnectionId]);

  const discoveredServices = useMemo((): GrpcServiceInfo[] => {
    return connectConfig?.discovered_services ?? [];
  }, [connectConfig]);

  const protoFiles = useMemo((): Array<[string, string]> => {
    const files = connectConfig?.proto_files;
    if (!files || files.length === 0) return [];
    return files.map((f) => [f.path, f.content] as [string, string]);
  }, [connectConfig]);

  const isReflectionMode = useMemo((): boolean => {
    return !!connectConfig?.enable_reflection && protoFiles.length === 0;
  }, [connectConfig, protoFiles]);

  const reflectionConfig = useMemo(() => {
    if (!connectConfig || !isReflectionMode) return null;
    return {
      url: connectConfig.url,
      use_tls: connectConfig.use_tls,
      tls_skip_verify: connectConfig.tls_skip_verify,
      connect_timeout_ms: connectConfig.connect_timeout_ms,
      tls_ca_cert: connectConfig.tls_ca_cert,
      tls_client_cert: connectConfig.tls_client_cert,
      tls_client_key: connectConfig.tls_client_key,
      tls_server_name_override: connectConfig.tls_server_name_override,
      reflection_metadata: (connectConfig.reflection_metadata ?? [])
        .filter((m) => m.key.trim())
        .map((m) => ({ key: m.key, value: m.value })),
    };
  }, [connectConfig, isReflectionMode]);

  const messageDefinitions = useMemo((): Record<string, GrpcMessageInfo> => {
    if (discoveredServices.length === 0) return {};
    return discoveredServices[0].message_definitions ?? {};
  }, [discoveredServices]);

  // ---- service / method selection -----------------------------------------
  const [serviceInputMode, setServiceInputMode] = useState<'select' | 'manual'>('select');
  const [methodInputMode, setMethodInputMode] = useState<'select' | 'manual'>('select');

  const discoveredMethods = useMemo((): GrpcMethodInfo[] => {
    if (!config.service_name || discoveredServices.length === 0) return [];
    const service = discoveredServices.find((s) => s.service_name === config.service_name);
    return service?.methods ?? [];
  }, [config.service_name, discoveredServices]);

  const selectedMethod = useMemo((): GrpcMethodInfo | null => {
    if (!config.method_name || discoveredMethods.length === 0) return null;
    return discoveredMethods.find((m) => m.method_name === config.method_name) || null;
  }, [config.method_name, discoveredMethods]);

  const isClientStreaming = selectedMethod?.is_client_streaming ?? false;

  // connection change clears stale service/method selections
  useEffect(() => {
    if (!currentConnectionId || discoveredServices.length === 0) return;
    if (config.service_name) {
      const stillExists = discoveredServices.some((s) => s.service_name === config.service_name);
      if (!stillExists) {
        onChange({ ...config, service_name: '', method_name: '' });
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentConnectionId, discoveredServices]);

  // ---- schema expansion -----------------------------------------------------
  const [expandedSchemas, setExpandedSchemas] = useState<Set<string>>(new Set());
  const toggleSchema = useCallback((dir: 'input' | 'output') => {
    setExpandedSchemas((prev) => {
      const next = new Set(prev);
      if (next.has(dir)) next.delete(dir);
      else next.add(dir);
      return next;
    });
  }, []);

  // ---- proto schema (form mode) --------------------------------------------
  const protoParseResult = useMemo((): ProtoParseResult | null => {
    if (protoFiles.length === 0) return null;
    try {
      return parseProtoFiles(protoFiles.map(([path, content]) => ({ path, content })));
    } catch {
      return null;
    }
  }, [protoFiles]);

  const selectedMethodInputType = useMemo((): string | null => {
    return selectedMethod?.input_type ?? null;
  }, [selectedMethod]);

  const hasSchemaForForm = useMemo((): boolean => {
    if (!protoParseResult || !selectedMethodInputType) return false;
    return findMessageDescriptor(protoParseResult, selectedMethodInputType) !== null;
  }, [protoParseResult, selectedMethodInputType]);

  // ---- edit mode -------------------------------------------------------------
  const [editMode, setEditMode] = useState<'json' | 'form'>('json');
  const [modeSwitchError, setModeSwitchError] = useState<string | null>(null);

  // ---- skeleton generation ----------------------------------------------------
  const [isGeneratingSkeleton, setIsGeneratingSkeleton] = useState(false);

  const canGenerateSkeleton = useMemo(() => {
    return (
      config.service_name.length > 0 &&
      config.method_name.length > 0 &&
      (protoFiles.length > 0 || isReflectionMode)
    );
  }, [config.service_name, config.method_name, protoFiles.length, isReflectionMode]);

  const handleGenerateSkeleton = useCallback(async () => {
    const serviceName = config.service_name;
    const methodName = config.method_name;
    if (!serviceName || !methodName) return;
    if (isGeneratingSkeleton) return;

    setIsGeneratingSkeleton(true);
    try {
      if (isReflectionMode && reflectionConfig) {
        if (!reflectionConfig.url) return;
        const result = (await uiCall('grpc.skeletonReflection', {
          ...reflectionConfig,
          service_name: serviceName,
          method_name: methodName,
        })) as SkeletonResult;
        if (result.success && result.skeleton) {
          onChange({ ...config, request_json: result.skeleton });
        }
        return;
      }
      if (protoFiles.length === 0) return;
      const result = (await uiCall('grpc.skeleton', {
        proto_files: protoFiles.map(([path, content]) => ({ path, content })),
        service_name: serviceName,
        method_name: methodName,
      })) as SkeletonResult;
      if (result.success && result.skeleton) {
        onChange({ ...config, request_json: result.skeleton });
      }
    } catch {
      // skeleton generation failed silently (same as host)
    } finally {
      setIsGeneratingSkeleton(false);
    }
  }, [config, isReflectionMode, reflectionConfig, protoFiles, isGeneratingSkeleton, onChange]);

  // ---- debounced validation (500ms) -------------------------------------------
  const [validationErrors, setValidationErrors] = useState<ValidationIssue[]>([]);
  const [isValidating, setIsValidating] = useState(false);
  const [validationValid, setValidationValid] = useState<boolean | null>(null);
  const validationTimerRef = useRef<number | null>(null);

  useEffect(() => {
    clearTimeout(validationTimerRef.current ?? undefined);

    const requestJson = config.request_json;
    if (!requestJson || !selectedMethodInputType || protoFiles.length === 0) {
      setValidationValid(null);
      setValidationErrors([]);
      setIsValidating(false);
      return;
    }
    if (requestJson.trim() === '{}') {
      setValidationValid(null);
      setValidationErrors([]);
      setIsValidating(false);
      return;
    }

    setIsValidating(true);
    const timerId = window.setTimeout(async () => {
      try {
        const result = (await uiCall('grpc.validate', {
          json: requestJson,
          message_name: selectedMethodInputType,
          proto_files: protoFiles.map(([path, content]) => ({ path, content })),
        })) as ValidateResult;
        if (result.success && result.result) {
          setValidationValid(result.result.valid);
          setValidationErrors(result.result.errors ?? []);
        } else {
          setValidationValid(null);
          setValidationErrors([]);
        }
      } catch {
        setValidationValid(null);
        setValidationErrors([]);
      } finally {
        setIsValidating(false);
      }
    }, 500);
    validationTimerRef.current = timerId;

    return () => {
      clearTimeout(timerId);
    };
  }, [config.request_json, selectedMethodInputType, protoFiles]);

  // ---- grpcurl export ----------------------------------------------------------
  const [grpcurlCommand, setGrpcurlCommand] = useState<string | null>(null);
  const [grpcurlCopied, setGrpcurlCopied] = useState(false);

  const handleExportGrpcurl = useCallback(() => {
    const serviceName = config.service_name;
    const methodName = config.method_name;
    const connectionId = config.connection_id;
    if (!serviceName || !methodName || !connectionId) return;

    const connectNode = availableConnections.find((c) => c.uuid === connectionId);
    if (!connectNode) return;

    const connectParams = extractGrpcurlParamsFromConnectConfig(connectNode.config);
    const command = generateGrpcurlCommand({
      ...connectParams,
      serviceName,
      methodName,
      requestJson: config.request_json,
      callMetadata: config.metadata ?? [],
    });
    setGrpcurlCommand(command);
    setGrpcurlCopied(false);
  }, [config, availableConnections]);

  const handleCopyGrpcurl = useCallback(async () => {
    if (!grpcurlCommand) return;
    try {
      await navigator.clipboard.writeText(grpcurlCommand);
      setGrpcurlCopied(true);
      setTimeout(() => setGrpcurlCopied(false), 2000);
    } catch {
      // clipboard write failed
    }
  }, [grpcurlCommand]);

  // ---- streaming request messages ----------------------------------------------
  const requestMessages = config.request_messages ?? [];

  const addMessage = useCallback(() => {
    set({ request_messages: [...requestMessages, { enabled: true, content: '{}' }] });
  }, [requestMessages, set]);

  const removeMessage = useCallback((index: number) => {
    set({ request_messages: requestMessages.filter((_, i) => i !== index) });
  }, [requestMessages, set]);

  const updateMessage = useCallback((index: number, patch: Partial<StreamMessage>) => {
    set({
      request_messages: requestMessages.map((m, i) => (i === index ? { ...m, ...patch } : m)),
    });
  }, [requestMessages, set]);

  const patternLabel = useMemo(() => {
    if (!selectedMethod) return null;
    return (
      <StreamingBadge method={selectedMethod} />
    );
  }, [selectedMethod]);

  return (
    <div className="panel">
      {/* 主说明卡片 */}
      <div className="card">
        <h4 className="card-title">
          <IconRadio size={16} />
          {t('panels.grpcCall.title')}
        </h4>
        <p className="card-desc">{t('panels.grpcCall.description')}</p>
      </div>

      {/* 连接选择 */}
      <div className="field">
        <Label htmlFor="connection_id" className="field-label">
          {t('panels.common.connection')} <span className="req">*</span>
        </Label>
        {availableConnections.length > 0 ? (
          <Select
            id="connection_id"
            value={currentConnectionId || ''}
            onValueChange={(value) => {
              onChange({
                ...config,
                connection_id: value,
                service_name: '',
                method_name: '',
              });
            }}
            placeholder={t('panels.common.selectConnection')}
            className="mono"
          >
            {availableConnections.map((conn) => (
              <SelectItem key={conn.uuid} value={conn.uuid}>
                {conn.name} ({conn.uuid.substring(0, 8)}...)
              </SelectItem>
            ))}
          </Select>
        ) : (
          <Input
            id="connection_id"
            value={currentConnectionId ?? ''}
            onChange={(e) => set({ connection_id: e.target.value })}
            placeholder={t('panels.common.enterConnectionId')}
            className="mono"
          />
        )}
        {availableConnections.length === 0 && (
          <p className="hint">{t('panels.grpcCall.noConnectNodeDetected')}</p>
        )}
      </div>

      {/* 服务选择 */}
      <div className="field">
        <div className="row-between">
          <Label htmlFor="service_name" className="field-label">
            {t('panels.grpcCall.serviceName')} <span className="req">*</span>
          </Label>
          {discoveredServices.length > 0 && (
            <button
              type="button"
              onClick={() => setServiceInputMode((prev) => (prev === 'select' ? 'manual' : 'select'))}
              className="btn-link"
            >
              {serviceInputMode === 'select'
                ? t('panels.common.manualInput')
                : t('panels.common.dropdownSelect')}
            </button>
          )}
        </div>
        {discoveredServices.length > 0 && serviceInputMode === 'select' ? (
          <div className="col">
            <Select
              id="service_name"
              value={config.service_name || ''}
              onValueChange={(value) => {
                onChange({ ...config, service_name: value, method_name: '' });
                setMethodInputMode('select');
              }}
              placeholder={t('panels.grpcCall.selectService')}
              className="mono"
            >
              {discoveredServices.map((svc) => (
                <SelectItem key={svc.service_name} value={svc.service_name}>
                  {svc.service_name} ({t('panels.grpcCall.methodUnit', { count: svc.methods.length })})
                </SelectItem>
              ))}
            </Select>
            <div className="row">
              <Badge className="badge-violet">
                {t('panels.grpcCall.serviceUnit', { count: discoveredServices.length })}
              </Badge>
            </div>
          </div>
        ) : (
          <div className="col">
            <Input
              id="service_name"
              value={config.service_name ?? ''}
              onChange={(e) => set({ service_name: e.target.value })}
              placeholder="package.ServiceName"
              className="mono"
            />
            {discoveredServices.length > 0 && (
              <p className="hint">
                {t('panels.common.manualInput')}，
                <button
                  type="button"
                  onClick={() => setServiceInputMode('select')}
                  className="btn-link"
                >
                  {t('panels.common.switchToDropdown')}
                </button>
              </p>
            )}
          </div>
        )}
        {discoveredServices.length === 0 && (
          <p className="hint">
            {t('panels.grpcCall.fullServiceName')}{' '}
            <code className="code">mypackage.MyService</code>
            {currentConnectionId && t('panels.grpcCall.noDiscoveredServices')}
          </p>
        )}
      </div>

      {/* 方法选择 */}
      <div className="field">
        <div className="row-between">
          <Label htmlFor="method_name" className="field-label">
            {t('panels.grpcCall.methodName')} <span className="req">*</span>
          </Label>
          {discoveredMethods.length > 0 && (
            <button
              type="button"
              onClick={() => setMethodInputMode((prev) => (prev === 'select' ? 'manual' : 'select'))}
              className="btn-link"
            >
              {methodInputMode === 'select'
                ? t('panels.common.manualInput')
                : t('panels.common.dropdownSelect')}
            </button>
          )}
        </div>
        {discoveredMethods.length > 0 && methodInputMode === 'select' ? (
          <div className="col">
            <Select
              id="method_name"
              value={config.method_name || ''}
              onValueChange={(value) => set({ method_name: value })}
              placeholder={t('panels.grpcCall.selectMethod')}
              className="mono"
            >
              {discoveredMethods.map((method) => (
                <SelectItem key={method.method_name} value={method.method_name}>
                  {method.method_name} {patternText(method)}
                </SelectItem>
              ))}
            </Select>

            {config.method_name &&
              (() => {
                const selected = discoveredMethods.find((m) => m.method_name === config.method_name);
                if (!selected) return null;
                const hasInputDef = !!messageDefinitions[selected.input_type];
                const hasOutputDef = !!messageDefinitions[selected.output_type];
                return (
                  <div className="col">
                    <div className="row">
                      {hasInputDef ? (
                        <button
                          type="button"
                          onClick={() => toggleSchema('input')}
                          className="chip chip-violet"
                        >
                          <IconChevronDown
                            size={12}
                            style={{
                              transform: expandedSchemas.has('input') ? 'rotate(180deg)' : undefined,
                              transition: 'transform 0.15s',
                            }}
                          />
                          {selected.input_type.split('.').pop() || selected.input_type}
                        </button>
                      ) : (
                        <span className="chip chip-violet chip-static">
                          {selected.input_type}
                        </span>
                      )}
                      <span className="method-arrow">→</span>
                      {hasOutputDef ? (
                        <button
                          type="button"
                          onClick={() => toggleSchema('output')}
                          className="chip chip-blue"
                        >
                          <IconChevronDown
                            size={12}
                            style={{
                              transform: expandedSchemas.has('output') ? 'rotate(180deg)' : undefined,
                              transition: 'transform 0.15s',
                            }}
                          />
                          {selected.output_type.split('.').pop() || selected.output_type}
                        </button>
                      ) : (
                        <span className="chip chip-blue chip-static">
                          {selected.output_type}
                        </span>
                      )}
                      {patternLabel}
                    </div>
                    {expandedSchemas.has('input') && hasInputDef && (
                      <div className="schema-wrap">
                        <div className="schema-label">{t('panels.grpcConnect.requestSchema')}</div>
                        <MessageSchemaView
                          message={messageDefinitions[selected.input_type]}
                          messageDefinitions={messageDefinitions}
                        />
                      </div>
                    )}
                    {expandedSchemas.has('output') && hasOutputDef && (
                      <div className="schema-wrap">
                        <div className="schema-label">{t('panels.grpcConnect.responseSchema')}</div>
                        <MessageSchemaView
                          message={messageDefinitions[selected.output_type]}
                          messageDefinitions={messageDefinitions}
                        />
                      </div>
                    )}
                  </div>
                );
              })()}
          </div>
        ) : (
          <div className="col">
            <Input
              id="method_name"
              value={config.method_name ?? ''}
              onChange={(e) => set({ method_name: e.target.value })}
              placeholder="MethodName"
              className="mono"
            />
            {discoveredMethods.length > 0 && (
              <p className="hint">
                {t('panels.common.manualInput')}，
                <button
                  type="button"
                  onClick={() => setMethodInputMode('select')}
                  className="btn-link"
                >
                  {t('panels.common.switchToDropdown')}
                </button>
              </p>
            )}
          </div>
        )}
      </div>

      {/* grpcurl 导出 */}
      <div className="col">
        <Button
          size="sm"
          disabled={!config.service_name || !config.method_name || !currentConnectionId}
          onClick={handleExportGrpcurl}
        >
          <IconTerminal size={12} />
          {t('panels.grpcCall.exportGrpcurl')}
        </Button>
        {grpcurlCommand && (
          <div className="section">
            <div className="row-between">
              <span className="section-title-sub">{t('panels.grpcCall.grpcurlCommand')}</span>
              <Button variant="ghost" size="sm" onClick={handleCopyGrpcurl} className="btn-violet">
                {grpcurlCopied ? (
                  <>
                    <IconCheck size={12} className="ok-text" />
                    {t('panels.grpcCall.copied')}
                  </>
                ) : (
                  <>
                    <IconCopy size={12} />
                    {t('panels.grpcCall.copyToClipboard')}
                  </>
                )}
              </Button>
            </div>
            <pre className="json-pre" style={{ maxHeight: 200, background: 'var(--code-bg)', color: 'var(--ok)' }}>
              {grpcurlCommand}
            </pre>
            <button type="button" onClick={() => setGrpcurlCommand(null)} className="btn-link">
              {t('panels.common.close')}
            </button>
          </div>
        )}
      </div>

      {/* 消息列表（Client/Bidi Streaming） */}
      {isClientStreaming && (
        <div className="col">
          <div className="row-between">
            <Label className="field-label">{t('panels.grpcCall.messageList')}</Label>
            <Button variant="ghost" size="sm" onClick={addMessage} className="btn-violet">
              <IconPlus size={12} />
              {t('panels.grpcCall.addMessage')}
            </Button>
          </div>
          {requestMessages.length === 0 ? (
            <Empty>{t('panels.grpcCall.clickAddMessageHint')}</Empty>
          ) : (
            <div className="col">
              {requestMessages.map((msg, index) => (
                <div key={index} className="kv-row" style={{ alignItems: 'flex-start' }}>
                  <input
                    type="checkbox"
                    checked={msg.enabled}
                    onChange={(e) => updateMessage(index, { enabled: e.target.checked })}
                    style={{ marginTop: 8, width: 16, height: 16, flexShrink: 0 }}
                  />
                  <div className="grow col">
                    <Textarea
                      value={msg.content}
                      onChange={(e) => updateMessage(index, { content: e.target.value })}
                      placeholder='{"field": "value"}'
                      rows={3}
                    />
                    <p className="hint-soft">
                      {t('panels.grpcCall.messageNumber', { n: index + 1 })}
                    </p>
                  </div>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => removeMessage(index)}
                    className="btn-rose"
                  >
                    <IconTrash2 size={14} />
                  </Button>
                </div>
              ))}
            </div>
          )}
          <p className="hint">{t('panels.grpcCall.messageOrderHint')}</p>
        </div>
      )}

      {/* 单消息编辑器（Unary / Server Streaming） */}
      {!isClientStreaming && (
        <div className="col">
          <div className="row-between">
            <Label htmlFor="request_json" className="field-label">
              {t('panels.grpcCall.requestEdit')}
            </Label>
            <div className="row">
              {/* 编辑模式切换 */}
              <div className="seg">
                <button
                  type="button"
                  onClick={() => {
                    setEditMode('json');
                    setModeSwitchError(null);
                  }}
                  className={'seg-btn' + (editMode === 'json' ? ' active' : '')}
                >
                  <IconBraces size={12} />
                  JSON
                </button>
                <button
                  type="button"
                  disabled={!hasSchemaForForm}
                  onClick={() => {
                    if (!hasSchemaForForm) return;
                    const currentJson = config.request_json;
                    if (currentJson && currentJson.trim() !== '{}') {
                      try {
                        JSON.parse(currentJson);
                      } catch {
                        setModeSwitchError(t('panels.grpcCall.jsonFormatError'));
                        return;
                      }
                    }
                    setModeSwitchError(null);
                    setEditMode('form');
                  }}
                  className={'seg-btn' + (editMode === 'form' ? ' active' : '')}
                >
                  <IconFileText size={12} />
                  {t('panels.grpcCall.formMode')}
                </button>
              </div>
              {editMode === 'json' && (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={!canGenerateSkeleton || isGeneratingSkeleton}
                  onClick={handleGenerateSkeleton}
                  className="btn-violet"
                >
                  <IconFileJson size={12} />
                  {isGeneratingSkeleton
                    ? t('panels.grpcCall.generating')
                    : t('panels.grpcCall.generateSkeleton')}
                </Button>
              )}
            </div>
          </div>

          {modeSwitchError && (
            <div className="alert alert-red">
              <IconXCircle size={14} className="alert-icon" />
              <span>{modeSwitchError}</span>
            </div>
          )}

          {editMode === 'json' && (
            <>
              <Textarea
                id="request_json"
                value={config.request_json ?? ''}
                onChange={(e) => set({ request_json: e.target.value })}
                placeholder='{"field": "value"}'
                rows={5}
              />
              {isValidating && (
                <div className="row muted">
                  <Spinner size={12} />
                  <span>{t('panels.grpcCall.validating')}</span>
                </div>
              )}
              {!isValidating && validationValid === true && (
                <div className="row ok-text">
                  <IconCheckCircle2 size={14} />
                  <span>{t('panels.grpcCall.jsonMatchSchema')}</span>
                </div>
              )}
              {!isValidating && validationValid === false && validationErrors.length > 0 && (
                <div className="col">
                  <div className="row warn-text">
                    <IconXCircle size={14} />
                    <span>
                      {t('panels.grpcCall.validationWarning', { count: validationErrors.length })}
                    </span>
                  </div>
                  <div
                    className="col"
                    style={{
                      maxHeight: 120,
                      overflowY: 'auto',
                      border: '1px solid var(--warn-border)',
                      background: 'var(--warn-soft)',
                      borderRadius: 8,
                      padding: 8,
                    }}
                  >
                    {validationErrors.map((err, idx) => (
                      <div key={idx} className="row" style={{ alignItems: 'flex-start', fontSize: 12 }}>
                        <code className="code warn-text" style={{ background: 'transparent' }}>
                          {err.path || '(root)'}
                        </code>
                        <span className="warn-text">—</span>
                        <span className="warn-text">{err.message}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}
              <p className="hint">{t('panels.grpcCall.requestJsonHint')}</p>
            </>
          )}

          {editMode === 'form' && (
            <>
              <div className="section" style={{ background: 'var(--bg)', borderColor: 'var(--border)' }}>
                <GrpcFormEditor
                  value={config.request_json || '{}'}
                  onChange={(json: string) => set({ request_json: json })}
                  parseResult={protoParseResult}
                  inputType={selectedMethodInputType}
                />
              </div>
              {!hasSchemaForForm && (
                <p className="hint">
                  {protoFiles.length === 0
                    ? t('panels.grpcCall.noProtoFileHint')
                    : t('panels.grpcCall.noMessageDefHint')}
                </p>
              )}
            </>
          )}
        </div>
      )}

      {/* 调用级 Metadata */}
      <div className="col">
        <Label className="field-label">{t('panels.grpcCall.metadataOptional')}</Label>
        <MetadataListEditor
          entries={config.metadata ?? []}
          onChange={(entries) => set({ metadata: entries })}
          emptyText={t('panels.grpcCall.noCallMetadata')}
          hint={t('panels.grpcCall.metadataMergeHint')}
        />
      </div>

      {/* 调用超时 */}
      <div className="field">
        <Label htmlFor="timeout_ms" className="field-label">
          {t('panels.grpcCall.callTimeout')}
        </Label>
        <Input
          id="timeout_ms"
          type="number"
          min={1000}
          max={300000}
          value={config.timeout_ms ?? ''}
          onChange={(e) =>
            set({ timeout_ms: e.target.value === '' ? undefined : Number(e.target.value) })
          }
        />
        <p className="hint">{t('panels.common.timeoutMs')}</p>
      </div>

      {/* 压缩编码 */}
      <div className="field">
        <Label htmlFor="call_compression_encoding" className="field-label">
          {t('panels.grpcCall.compressionEncoding')}
        </Label>
        <Select
          id="call_compression_encoding"
          value={config.compression_encoding || 'inherit'}
          onValueChange={(value) =>
            set({ compression_encoding: value === 'inherit' ? null : value })
          }
          className="mono"
        >
          <SelectItem value="inherit">{t('panels.grpcCall.inheritDefault')}</SelectItem>
          <SelectItem value="none">{t('panels.common.none')}</SelectItem>
          <SelectItem value="gzip">Gzip</SelectItem>
        </Select>
        <p className="hint">{t('panels.grpcCall.compressionInheritHint')}</p>
      </div>

      {/* 提示信息 */}
      <div className="help-card">
        <span className="alert-icon">
          <IconInfo size={14} className="muted" />
        </span>
        <div className="help-body">
          <p className="help-title">{t('panels.grpcCall.callExplanation')}</p>
          <p>{t('panels.grpcCall.callExplanation1')}</p>
          <p>{t('panels.grpcCall.callExplanation2')}</p>
          <p>{t('panels.grpcCall.callExplanation3')}</p>
        </div>
      </div>
    </div>
  );
}

/** Compact streaming-mode text for the native `<option>` label. */
function patternText(method: GrpcMethodInfo): string {
  if (method.is_server_streaming && method.is_client_streaming) {
    return '[' + t('panels.grpcCall.bidiStreaming') + ']';
  }
  if (method.is_server_streaming) {
    return '[' + t('panels.grpcCall.serverStreaming') + ']';
  }
  if (method.is_client_streaming) {
    return '[' + t('panels.grpcCall.clientStreaming') + ']';
  }
  return '';
}
