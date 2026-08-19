/**
 * gRPC Connect node config panel.
 * Port of the host `GrpcConnectConfig.tsx` — all `invoke('grpc_*')` calls
 * replaced by uiCall bridge methods (`grpc.discover` / `grpc.scanProtoDirectory`
 * / `grpc.readProtoFiles` / `grpc.parseDescriptorSet` / `grpc.channelz`),
 * file pickers by `openFileDialog`, persistence by debounced `configChanged`.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  ChannelzResult,
  DiscoverResult,
  GrpcConnectConfig,
  GrpcMessageInfo,
  GrpcServiceInfo,
  ReadProtoFilesResult,
} from '../types';
import { t } from '../i18n';
import {
  Badge,
  Button,
  Empty,
  IconActivity,
  IconAlertTriangle,
  IconCable,
  IconChevronDown,
  IconFileText,
  IconFileUp,
  IconFolderOpen,
  IconInfo,
  IconPlus,
  IconRefreshCw,
  IconSearch,
  IconTrash2,
  IconUpload,
  Input,
  Label,
  Select,
  SelectItem,
  Spinner,
  Switch,
  Textarea,
} from '../lib/ui';
import { MessageSchemaView } from '../lib/MessageSchemaView';
import { errorMessage, MetadataListEditor, StreamingBadge, StringListEditor } from './shared';
import { openFileDialog, uiCall } from '../bridge';

interface ConnectPanelProps {
  config: GrpcConnectConfig;
  onChange: (config: GrpcConnectConfig) => void;
}

export function ConnectPanel({ config, onChange }: ConnectPanelProps) {
  const set = useCallback(
    (patch: Partial<GrpcConnectConfig>) => {
      onChange({ ...config, ...patch });
    },
    [config, onChange],
  );

  const useTls = config.use_tls ?? false;
  const enableReflection = config.enable_reflection ?? false;
  const tlsSkipVerify = config.tls_skip_verify ?? false;

  // ---- discovery ----------------------------------------------------------
  const [isDiscovering, setIsDiscovering] = useState(false);
  const [discoveryResult, setDiscoveryResult] = useState<DiscoverResult | null>(null);

  // URL change clears cached discovered services (same as host behavior).
  const prevUrlRef = useRef<string>(config.url);
  useEffect(() => {
    const trimmed = (config.url ?? '').trim();
    const prev = (prevUrlRef.current ?? '').trim();
    if (trimmed !== prev && prev !== '') {
      onChange({ ...config, discovered_services: undefined });
      setDiscoveryResult(null);
    }
    prevUrlRef.current = config.url ?? '';
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config.url]);

  const handleDiscover = useCallback(async () => {
    const url = (config.url ?? '').trim();
    if (!url || isDiscovering) return;
    setIsDiscovering(true);
    setDiscoveryResult(null);
    try {
      const result = (await uiCall('grpc.discover', {
        url,
        use_tls: useTls,
        tls_skip_verify: tlsSkipVerify,
        enable_reflection: enableReflection,
        proto_files: (config.proto_files ?? []).filter(
          (f) => f.path.trim() && f.content.trim(),
        ),
        connect_timeout_ms: config.connect_timeout_ms ?? 30000,
        tls_ca_cert: (config.tls_ca_cert ?? '').trim() || undefined,
        tls_client_cert: (config.tls_client_cert ?? '').trim() || undefined,
        tls_client_key: (config.tls_client_key ?? '').trim() || undefined,
        tls_server_name_override: (config.tls_server_name_override ?? '').trim() || undefined,
        reflection_metadata: (config.reflection_metadata ?? [])
          .filter((m) => m.key.trim())
          .map((m) => ({ key: m.key, value: m.value })),
      })) as DiscoverResult;
      setDiscoveryResult(result);
      if (result.success && result.services && result.services.length > 0) {
        // persist discovered services into the node data
        onChange({ ...config, discovered_services: result.services });
      }
    } catch (err) {
      setDiscoveryResult({ success: false, services: [], error: errorMessage(err) });
    } finally {
      setIsDiscovering(false);
    }
  }, [config, useTls, tlsSkipVerify, enableReflection, isDiscovering, onChange]);

  // ---- proto file management ---------------------------------------------
  const mergeProtoFiles = useCallback(
    (files: { path: string; content: string }[]) => {
      if (!files || files.length === 0) return;
      const existing = config.proto_files ?? [];
      const next = [...existing];
      for (const file of files) {
        const idx = next.findIndex((f) => f.path === file.path);
        if (idx >= 0) {
          next[idx] = { path: file.path, content: file.content };
        } else {
          next.push({ path: file.path, content: file.content });
        }
      }
      set({ proto_files: next });
    },
    [config.proto_files, set],
  );

  const handleSelectProtoFiles = useCallback(async () => {
    try {
      const selected = await openFileDialog({
        multiple: true,
        filters: [{ name: t('panels.grpcConnect.protoFileFilter'), extensions: ['proto'] }],
      });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      if (paths.length === 0) return;
      const result = (await uiCall('grpc.readProtoFiles', { file_paths: paths })) as ReadProtoFilesResult;
      if (result.success) {
        mergeProtoFiles(result.files);
      }
    } catch {
      // cancelled or non-Tauri environment
    }
  }, [mergeProtoFiles]);

  const handleSelectDirectory = useCallback(async () => {
    try {
      const selected = await openFileDialog({ directory: true });
      if (!selected || typeof selected !== 'string') return;
      const result = (await uiCall('grpc.scanProtoDirectory', {
        dir_path: selected,
      })) as ReadProtoFilesResult;
      if (result.success && result.files.length > 0) {
        mergeProtoFiles(result.files);
      }
    } catch {
      // cancelled or failed
    }
  }, [mergeProtoFiles]);

  const handleImportDescriptorSet = useCallback(async () => {
    try {
      const selected = await openFileDialog({
        multiple: false,
        filters: [{ name: 'Descriptor Set', extensions: ['pb', 'fdset', 'desc'] }],
      });
      if (!selected || typeof selected !== 'string') return;
      const result = (await uiCall('grpc.parseDescriptorSet', {
        file_path: selected,
      })) as DiscoverResult;
      if (result.success && result.services && result.services.length > 0) {
        const existing = config.discovered_services ?? [];
        const merged = [...existing];
        for (const svc of result.services) {
          if (!merged.some((s) => s.service_name === svc.service_name)) {
            merged.push(svc);
          }
        }
        onChange({ ...config, discovered_services: merged });
        setDiscoveryResult({ success: true, services: merged, error: null });
      }
    } catch {
      // cancelled or failed
    }
  }, [config, onChange]);

  const pickTlsPath = useCallback(
    async (field: 'tls_ca_cert' | 'tls_client_cert' | 'tls_client_key', filters: { name: string; extensions: string[] }) => {
      try {
        const selected = await openFileDialog({ multiple: false, filters: [filters] });
        if (selected && typeof selected === 'string') {
          set({ [field]: selected } as Partial<GrpcConnectConfig>);
        }
      } catch {
        // cancelled
      }
    },
    [set],
  );

  // ---- TLS cert file inputs ----------------------------------------------
  const renderTlsPathInput = (
    field: 'tls_ca_cert' | 'tls_client_cert' | 'tls_client_key',
    label: string,
    placeholder: string,
    filterName: string,
    extensions: string[],
  ) => (
    <div className="field">
      <Label className="field-label-sm">{label}</Label>
      <div className="row">
        <Input
          value={config[field] ?? ''}
          onChange={(e) => set({ [field]: e.target.value } as Partial<GrpcConnectConfig>)}
          placeholder={placeholder}
          className="mono"
        />
        <Button
          variant="ghost"
          size="icon"
          className="btn-rose"
          onClick={() => pickTlsPath(field, { name: filterName, extensions })}
        >
          <IconFolderOpen size={14} />
        </Button>
      </div>
    </div>
  );

  // ---- service tree -------------------------------------------------------
  const [expandedServices, setExpandedServices] = useState<Set<string>>(new Set());
  const [expandedSchemas, setExpandedSchemas] = useState<Set<string>>(new Set());

  const toggleService = useCallback((name: string) => {
    setExpandedServices((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  }, []);

  const toggleSchema = useCallback((serviceName: string, methodName: string, direction: 'input' | 'output') => {
    const key = serviceName + '::' + methodName + '::' + direction;
    setExpandedSchemas((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  const displayServices = useMemo((): GrpcServiceInfo[] => {
    if (discoveryResult && discoveryResult.success && discoveryResult.services.length > 0) {
      return discoveryResult.services;
    }
    return config.discovered_services ?? [];
  }, [discoveryResult, config.discovered_services]);

  const messageDefinitions = useMemo((): Record<string, GrpcMessageInfo> => {
    if (displayServices.length === 0) return {};
    return displayServices[0].message_definitions ?? {};
  }, [displayServices]);

  const typeDefined = useCallback(
    (defs: Record<string, GrpcMessageInfo>, typeName: string): boolean => {
      return !!defs[typeName];
    },
    [],
  );

  // ---- channelz ------------------------------------------------------------
  const [isChannelzLoading, setIsChannelzLoading] = useState(false);
  const [channelzResult, setChannelzResult] = useState<ChannelzResult | null>(null);
  const [channelzConnectionId, setChannelzConnectionId] = useState('');

  const handleChannelzQuery = useCallback(async () => {
    const connectionId = channelzConnectionId.trim();
    if (!connectionId || isChannelzLoading) return;
    setIsChannelzLoading(true);
    setChannelzResult(null);
    try {
      const result = (await uiCall('grpc.channelz', {
        connection_id: connectionId,
      })) as ChannelzResult;
      setChannelzResult(result);
    } catch (err) {
      setChannelzResult({ success: false, info: null, error: errorMessage(err) });
    } finally {
      setIsChannelzLoading(false);
    }
  }, [channelzConnectionId, isChannelzLoading]);

  const formatUptime = (secs: number): string => {
    if (secs < 60) return secs + 's';
    if (secs < 3600) return Math.floor(secs / 60) + 'm ' + (secs % 60) + 's';
    return Math.floor(secs / 3600) + 'h ' + Math.floor((secs % 3600) / 60) + 'm';
  };

  const channelzStateColor = (state: string): string => {
    switch (state) {
      case 'READY':
        return 'ok-text';
      case 'IDLE':
        return 'warn-text';
      case 'CONNECTING':
        return 'badge-blue';
      case 'TRANSIENT_FAILURE':
        return 'error-text';
      default:
        return 'muted';
    }
  };

  const channelzDotClass = (state: string): string => {
    switch (state) {
      case 'READY':
        return 'dot-green';
      case 'IDLE':
        return 'dot-amber';
      case 'CONNECTING':
        return 'dot-blue pulse';
      case 'TRANSIENT_FAILURE':
        return 'dot-red';
      default:
        return 'dot-amber';
    }
  };

  return (
    <div className="panel">
      {/* 主说明卡片 */}
      <div className="card">
        <h4 className="card-title">
          <IconCable size={16} />
          {t('panels.grpcConnect.title')}
        </h4>
        <p className="card-desc">{t('panels.grpcConnect.description')}</p>
      </div>

      {/* 服务器地址 */}
      <div className="field">
        <Label htmlFor="url" className="field-label">
          {t('panels.grpcConnect.serverAddress')} <span className="req">*</span>
        </Label>
        <div className="input-icon-wrap">
          <span className="input-icon">
            <IconCable size={14} />
          </span>
          <Input
            id="url"
            value={config.url ?? ''}
            onChange={(e) => set({ url: e.target.value })}
            placeholder="http://localhost:50051"
            className="mono"
          />
        </div>
        <p className="hint">{t('panels.grpcConnect.serverAddressHint')}</p>
      </div>

      {/* TLS 配置 */}
      <div className="section">
        <Switch
          id="use_tls"
          checked={useTls}
          onCheckedChange={(checked) => {
            set({ use_tls: checked, tls_skip_verify: checked ? tlsSkipVerify : false });
          }}
          label={t('panels.common.enableTls')}
        />

        {useTls && (
          <>
            <Switch
              id="tls_skip_verify"
              checked={tlsSkipVerify}
              onCheckedChange={(checked) => set({ tls_skip_verify: checked })}
              label={t('panels.common.skipCertVerify')}
            />
            {tlsSkipVerify && (
              <div className="alert alert-amber">
                <IconAlertTriangle size={14} className="alert-icon" />
                <span>{t('panels.common.skipCertVerifyWarning')}</span>
              </div>
            )}

            {!tlsSkipVerify && (
              <div className="col" style={{ paddingLeft: 10, borderLeft: '2px solid var(--accent-border)' }}>
                <p className="section-title-sub">{t('panels.grpcConnect.tlsAdvancedConfig')}</p>

                {renderTlsPathInput(
                  'tls_ca_cert',
                  t('panels.grpcConnect.caCert'),
                  t('panels.grpcConnect.caCertPlaceholder'),
                  t('panels.grpcConnect.pemCert'),
                  ['pem', 'crt', 'cer', 'ca'],
                )}
                {renderTlsPathInput(
                  'tls_client_cert',
                  t('panels.grpcConnect.clientCert'),
                  t('panels.grpcConnect.clientCertPlaceholder'),
                  t('panels.grpcConnect.pemCert'),
                  ['pem', 'crt', 'cer'],
                )}
                {renderTlsPathInput(
                  'tls_client_key',
                  t('panels.grpcConnect.clientKey'),
                  t('panels.grpcConnect.clientKeyPlaceholder'),
                  t('panels.grpcConnect.pemKey'),
                  ['pem', 'key'],
                )}

                <div className="field">
                  <Label className="field-label-sm">{t('panels.grpcConnect.sniOverride')}</Label>
                  <Input
                    value={config.tls_server_name_override ?? ''}
                    onChange={(e) => set({ tls_server_name_override: e.target.value })}
                    placeholder={t('panels.grpcConnect.sniOverridePlaceholder')}
                    className="mono"
                  />
                  <p className="hint-soft">{t('panels.grpcConnect.sniOverrideHint')}</p>
                </div>
              </div>
            )}
          </>
        )}
      </div>

      {/* Proto 来源 */}
      <div className="col">
        <Label className="field-label">{t('panels.grpcConnect.protoSource')}</Label>
        <Switch
          id="enable_reflection"
          checked={enableReflection}
          onCheckedChange={(checked) => set({ enable_reflection: checked })}
          label={t('panels.grpcConnect.useServerReflection')}
        />
        <p className="hint">
          {enableReflection
            ? t('panels.grpcConnect.reflectionDescription')
            : t('panels.grpcConnect.manualProtoDescription')}
        </p>

        {enableReflection && (
          <div className="col" style={{ paddingLeft: 10, borderLeft: '2px solid var(--accent-border)' }}>
            <Label className="field-label-sm">{t('panels.grpcConnect.authMetadata')}</Label>
            <p className="hint-soft">{t('panels.grpcConnect.authMetadataHint')}</p>
            <MetadataListEditor
              entries={config.reflection_metadata ?? []}
              onChange={(entries) => set({ reflection_metadata: entries })}
              emptyText={t('panels.grpcConnect.noAuthMetadata')}
            />
          </div>
        )}

        {!enableReflection && (
          <div className="col">
            <div className="row-between">
              <Label className="field-label-sm">{t('panels.grpcConnect.protoFileList')}</Label>
              <Button variant="ghost" size="sm" onClick={() => set({ proto_files: [...(config.proto_files ?? []), { path: '', content: '' }] })} className="btn-violet">
                <IconPlus size={12} />
                {t('panels.grpcConnect.addFile')}
              </Button>
            </div>

            <div className="row" style={{ flexWrap: 'wrap' }}>
              <Button size="sm" onClick={handleSelectProtoFiles}>
                <IconFileUp size={14} />
                {t('panels.grpcConnect.selectProtoFile')}
              </Button>
              <Button size="sm" onClick={handleSelectDirectory}>
                <IconFolderOpen size={14} />
                {t('panels.grpcConnect.selectDirectory')}
              </Button>
              <Button size="sm" onClick={handleImportDescriptorSet}>
                <IconUpload size={14} />
                {t('panels.grpcConnect.importDescriptorSet')}
              </Button>
            </div>

            <div className="col">
              {(config.proto_files ?? []).length === 0 ? (
                <Empty>
                  {t('panels.grpcConnect.noProtoFiles')}
                  <div className="empty-sub">{t('panels.grpcConnect.addFileHint')}</div>
                </Empty>
              ) : (
                (config.proto_files ?? []).map((file, index) => (
                  <div key={index} className="kv-row" style={{ flexDirection: 'column', alignItems: 'stretch' }}>
                    <div className="row">
                      <IconFileText size={14} className="muted shrink0" />
                      <Input
                        value={file.path}
                        onChange={(e) => {
                          const files = [...(config.proto_files ?? [])];
                          files[index] = { ...file, path: e.target.value };
                          set({ proto_files: files });
                        }}
                        placeholder={t('panels.grpcConnect.protoFileNamePlaceholder')}
                        className="mono grow"
                      />
                      <Button
                        variant="ghost"
                        size="icon"
                        className="btn-rose"
                        onClick={() => {
                          const files = (config.proto_files ?? []).filter((_, i) => i !== index);
                          set({ proto_files: files });
                        }}
                      >
                        <IconTrash2 size={14} />
                      </Button>
                    </div>
                    <Textarea
                      value={file.content}
                      onChange={(e) => {
                        const files = [...(config.proto_files ?? [])];
                        files[index] = { ...file, content: e.target.value };
                        set({ proto_files: files });
                      }}
                      rows={5}
                      placeholder={'syntax = "proto3";\n\nservice MyService {\n  rpc MyMethod (Request) returns (Response);\n}'}
                    />
                  </div>
                ))
              )}
            </div>
          </div>
        )}
      </div>

      {/* 测试连接 & 发现服务 */}
      <div className="col">
        <Button
          className="btn-block"
          disabled={!(config.url ?? '').trim() || isDiscovering}
          onClick={handleDiscover}
        >
          {isDiscovering ? <Spinner /> : <IconSearch size={14} />}
          {isDiscovering
            ? t('panels.grpcConnect.discoveringServices')
            : t('panels.grpcConnect.testConnectionDiscover')}
        </Button>

        {discoveryResult && (
          <div
            className={
              'alert ' + (discoveryResult.success ? 'alert-green' : 'alert-red')
            }
          >
            {discoveryResult.success ? (
              <span>
                {t('panels.grpcConnect.servicesDiscoveredPrefix', {
                  count: discoveryResult.services.length,
                })}{' '}
                {t('panels.grpcConnect.servicesUnit', { count: discoveryResult.services.length })}
              </span>
            ) : (
              <span>{discoveryResult.error}</span>
            )}
          </div>
        )}

        {displayServices.length > 0 && (
          <div className="col">
            <div className="row-between">
              <Label className="field-label-sm">{t('panels.grpcConnect.discoveredServices')}</Label>
              <Badge className="badge-violet">
                {t('panels.grpcConnect.servicesUnit', { count: displayServices.length })}
              </Badge>
            </div>
            <div className="tree">
              {displayServices.map((service) => {
                const isExpanded = expandedServices.has(service.service_name);
                return (
                  <div key={service.service_name} className="service-box">
                    <button
                      type="button"
                      onClick={() => toggleService(service.service_name)}
                      className="service-head"
                    >
                      <span className="service-name">{service.service_name}</span>
                      <span className="service-meta">
                        <span className="muted">
                          {t('panels.grpcConnect.methodUnit', { count: service.methods.length })}
                        </span>
                        <IconChevronDown
                          size={14}
                          className={isExpanded ? '' : 'muted'}
                          style={{
                            transform: isExpanded ? 'rotate(180deg)' : undefined,
                            transition: 'transform 0.15s',
                          }}
                        />
                      </span>
                    </button>
                    {isExpanded && service.methods.length > 0 && (
                      <div className="method-list">
                        {service.methods.map((method) => (
                          <div key={method.method_name} className="method-row">
                            <code className="method-name">{method.method_name}</code>
                            <span className="method-arrow">→</span>

                            {typeDefined(messageDefinitions, method.input_type) ? (
                              <button
                                type="button"
                                onClick={() => toggleSchema(service.service_name, method.method_name, 'input')}
                                className="chip chip-violet"
                              >
                                <IconChevronDown
                                  size={12}
                                  style={{
                                    transform: expandedSchemas.has(service.service_name + '::' + method.method_name + '::input')
                                      ? 'rotate(180deg)'
                                      : undefined,
                                    transition: 'transform 0.15s',
                                  }}
                                />
                                {method.input_type.split('.').pop() || method.input_type}
                              </button>
                            ) : (
                              <code className="method-type muted">{method.input_type}</code>
                            )}

                            <span className="method-arrow">→</span>

                            {typeDefined(messageDefinitions, method.output_type) ? (
                              <button
                                type="button"
                                onClick={() => toggleSchema(service.service_name, method.method_name, 'output')}
                                className="chip chip-blue"
                              >
                                <IconChevronDown
                                  size={12}
                                  style={{
                                    transform: expandedSchemas.has(service.service_name + '::' + method.method_name + '::output')
                                      ? 'rotate(180deg)'
                                      : undefined,
                                    transition: 'transform 0.15s',
                                  }}
                                />
                                {method.output_type.split('.').pop() || method.output_type}
                              </button>
                            ) : (
                              <code className="method-type muted">{method.output_type}</code>
                            )}

                            <StreamingBadge method={method} />
                          </div>
                        ))}
                      </div>
                    )}

                    {isExpanded && service.methods.length > 0 && (
                      <>
                        {service.methods.map((method) => (
                          <div key={method.method_name + '-schemas'}>
                            {expandedSchemas.has(service.service_name + '::' + method.method_name + '::input') &&
                              messageDefinitions[method.input_type] && (
                                <div className="schema-wrap">
                                  <div className="schema-label">{t('panels.grpcConnect.requestSchema')}</div>
                                  <MessageSchemaView
                                    message={messageDefinitions[method.input_type]}
                                    messageDefinitions={messageDefinitions}
                                  />
                                </div>
                              )}
                            {expandedSchemas.has(service.service_name + '::' + method.method_name + '::output') &&
                              messageDefinitions[method.output_type] && (
                                <div className="schema-wrap">
                                  <div className="schema-label">{t('panels.grpcConnect.responseSchema')}</div>
                                  <MessageSchemaView
                                    message={messageDefinitions[method.output_type]}
                                    messageDefinitions={messageDefinitions}
                                  />
                                </div>
                              )}
                          </div>
                        ))}
                      </>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </div>

      {/* 默认 Metadata */}
      <div className="col">
        <Label className="field-label">{t('panels.grpcConnect.defaultMetadata')}</Label>
        <MetadataListEditor
          entries={config.default_metadata ?? []}
          onChange={(entries) => set({ default_metadata: entries })}
          emptyText={t('panels.grpcConnect.noDefaultMetadata')}
        />
      </div>

      {/* 连接超时 */}
      <div className="field">
        <Label htmlFor="connect_timeout_ms" className="field-label">
          {t('panels.grpcConnect.connectionTimeout')}
        </Label>
        <Input
          id="connect_timeout_ms"
          type="number"
          min={1000}
          max={300000}
          value={config.connect_timeout_ms ?? ''}
          onChange={(e) =>
            set({ connect_timeout_ms: e.target.value === '' ? undefined : Number(e.target.value) })
          }
        />
        <p className="hint">{t('panels.common.timeoutMs')}</p>
      </div>

      {/* 压缩编码 */}
      <div className="field">
        <Label htmlFor="compression_encoding" className="field-label">
          {t('panels.grpcConnect.compressionEncoding')}
        </Label>
        <Select
          id="compression_encoding"
          value={config.compression_encoding || 'none'}
          onValueChange={(value) => set({ compression_encoding: value === 'none' ? null : value })}
          className="mono"
        >
          <SelectItem value="none">{t('panels.common.none')}</SelectItem>
          <SelectItem value="gzip">Gzip</SelectItem>
        </Select>
        <p className="hint">{t('panels.grpcConnect.compressionHint')}</p>
      </div>

      {/* Keepalive 配置 */}
      <div className="section">
        <p className="section-title">
          <IconCable size={14} />
          {t('panels.grpcConnect.keepaliveConfig')}
        </p>
        <div className="field">
          <Label className="field-label-sm">{t('panels.grpcConnect.pingIntervalMs')}</Label>
          <Input
            type="number"
            min={1000}
            max={300000}
            placeholder={t('panels.grpcConnect.pingIntervalPlaceholder')}
            value={config.keepalive_time_ms ?? ''}
            onChange={(e) =>
              set({ keepalive_time_ms: e.target.value === '' ? null : Number(e.target.value) })
            }
            className="mono"
          />
          <p className="hint-soft">{t('panels.grpcConnect.pingIntervalHint')}</p>
        </div>
        <div className="field">
          <Label className="field-label-sm">{t('panels.grpcConnect.pingTimeoutMs')}</Label>
          <Input
            type="number"
            min={1000}
            max={60000}
            placeholder={t('panels.grpcConnect.pingTimeoutPlaceholder')}
            value={config.keepalive_timeout_ms ?? ''}
            onChange={(e) =>
              set({ keepalive_timeout_ms: e.target.value === '' ? null : Number(e.target.value) })
            }
            className="mono"
          />
          <p className="hint-soft">{t('panels.grpcConnect.pingTimeoutHint')}</p>
        </div>
        <Switch
          id="keepalive_permit_without_streams"
          checked={config.keepalive_permit_without_streams ?? false}
          onCheckedChange={(checked) => set({ keepalive_permit_without_streams: checked || null })}
          label={t('panels.grpcConnect.pingWithoutStreams')}
        />
        <p className="hint-soft">{t('panels.grpcConnect.pingWithoutStreamsHint')}</p>
      </div>

      {/* Channelz 内省 */}
      <div className="section">
        <div className="row">
          <IconActivity size={14} className="muted" />
          <span className="section-title">{t('panels.grpcConnect.channelzIntrospection')}</span>
          <span className="badge badge-outline" style={{ textTransform: 'uppercase' }}>
            Experimental
          </span>
        </div>
        <div className="col">
          <p className="hint">
            {t('panels.grpcConnect.channelzDescription1')}{' '}
            {t('panels.grpcConnect.channelzDescription2', { id: channelzConnectionId || '' })}
          </p>
          <div className="field">
            <Label className="field-label-sm">{t('panels.grpcConnect.channelzConnectionId')}</Label>
            <Input
              value={channelzConnectionId}
              onChange={(e) => setChannelzConnectionId(e.target.value)}
              placeholder={t('panels.grpcConnect.channelzConnectionIdPlaceholder')}
              className="mono"
            />
          </div>
          <Button
            className="btn-block"
            disabled={isChannelzLoading}
            onClick={handleChannelzQuery}
          >
            {isChannelzLoading ? <Spinner /> : <IconRefreshCw size={14} />}
            {isChannelzLoading
              ? t('panels.grpcConnect.channelzQuerying')
              : t('panels.grpcConnect.queryState')}
          </Button>

          {channelzResult && (
            <div className={'alert ' + (channelzResult.success ? 'alert-blue' : 'alert-red')}>
              {channelzResult.success && channelzResult.info ? (
                <div className="col">
                  <div className="row">
                    <span className={'dot ' + channelzDotClass(channelzResult.info.state)} />
                    <span className={channelzStateColor(channelzResult.info.state)}>
                      {channelzResult.info.state}
                    </span>
                  </div>
                  <div className="kv-list">
                    <div className="kv-item">
                      <span className="kv-key">{t('panels.grpcConnect.uptime')}</span>
                      <span className="kv-value mono">
                        {formatUptime(channelzResult.info.uptime_secs)}
                      </span>
                    </div>
                    <div className="kv-item">
                      <span className="kv-key">{t('panels.grpcConnect.activeCalls')}</span>
                      <span className="kv-value mono">{channelzResult.info.active_calls}</span>
                    </div>
                    <div className="kv-item">
                      <span className="kv-key">{t('panels.grpcConnect.connectedAt')}</span>
                      <span className="kv-value mono" style={{ fontSize: 11 }}>
                        {new Date(channelzResult.info.connected_at).toLocaleTimeString()}
                      </span>
                    </div>
                  </div>
                  {channelzResult.info.last_error && (
                    <div className="error-text" style={{ fontSize: 11 }}>
                      {t('panels.grpcConnect.lastError')} {channelzResult.info.last_error}
                    </div>
                  )}
                </div>
              ) : (
                <span className="error-text">{channelzResult.error}</span>
              )}
            </div>
          )}
        </div>
      </div>

      {/* 负载均衡 */}
      <div className="section">
        <div className="row-between">
          <p className="section-title">
            <IconCable size={14} />
            {t('panels.grpcConnect.loadBalancing')}
          </p>
        </div>
        <p className="hint">{t('panels.grpcConnect.loadBalancingHint')}</p>
        <StringListEditor
          items={config.endpoints ?? []}
          onChange={(items) => set({ endpoints: items })}
          emptyText={t('panels.grpcConnect.noExtraEndpoints')}
          placeholder="http://host:port"
          addLabel={t('panels.grpcConnect.addEndpoint')}
        />
      </div>

      {/* 提示信息 */}
      <div className="help-card">
        <span className="alert-icon">
          <IconInfo size={14} className="muted" />
        </span>
        <div className="help-body">
          <p className="help-title">
            {t('panels.grpcConnect.protoSource')}
            {t('panels.grpcConnect.helpSuffix')}
          </p>
          <p>
            <code className="code">Server Reflection</code> {t('panels.grpcConnect.reflectionExplanation')}
          </p>
          <p>
            <code className="code">{t('panels.grpcConnect.manualUploadLabel')}</code>{' '}
            {t('panels.grpcConnect.manualUploadExplanation')}
          </p>
          <p>
            <code className="code">{t('panels.grpcConnect.testConnectionLabel')}</code>{' '}
            {t('panels.grpcConnect.testConnectionExplanation')}
          </p>
        </div>
      </div>
    </div>
  );
}
