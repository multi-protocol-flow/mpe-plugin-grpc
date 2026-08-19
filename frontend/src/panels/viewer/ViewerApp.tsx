/**
 * Report viewer — renders the execution report for grpc:connect / grpc:call /
 * grpc:close nodes (aligned with the host `GrpcViewer.tsx`) and appends
 * realtime `stream` messages (filtered by `call_id.startsWith(node_uuid + '-')`,
 * grouped by call_id).
 */
import { useMemo, useState } from 'react';
import type {
  GrpcCallOutput,
  GrpcCloseOutput,
  GrpcConnectOutput,
  GrpcErrorDetail,
  GrpcMethodInfo,
  GrpcServiceInfo,
  NodeReport,
  PluginStreamMessage,
} from '../../types';
import { t } from '../../i18n';
import {
  Badge,
  IconAlertTriangle,
  IconCable,
  IconChevronDown,
  IconChevronRight,
  IconClock,
  IconGlobe,
  IconRadio,
  IconServer,
  IconUnplug,
} from '../../lib/ui';
import {
  CollapsibleSection,
  KeyValueList,
  RawJsonViewer,
  StatusBadge,
  TimingItem,
  VariableChangesCard,
} from './viewerCommon';

// ============================================================================
// Realtime stream entries
// ============================================================================

export interface StreamEntry {
  call_id: string;
  kind: 'message' | 'error';
  data: unknown;
  received_at_ms?: number;
}

/** Format bytes as a human-readable string. */
function formatSizeBytes(bytes: number): string {
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
}

interface ViewerAppProps {
  report: NodeReport | null;
  streams: PluginStreamMessage[];
}

export function ViewerApp({ report, streams }: ViewerAppProps) {
  const nodeType = report?.node_type ?? '';
  const nodeUuid = report?.node_uuid ?? '';

  // Final output: success path uses output_data, failure path uses plugin_data.
  const output = useMemo(() => {
    return report?.output_data ?? report?.plugin_data ?? undefined;
  }, [report]);

  // Realtime messages for this node, grouped by call_id, filtered by prefix.
  const realtimeByCallId = useMemo(() => {
    const grouped: Record<string, StreamEntry[]> = {};
    if (!nodeUuid) return grouped;
    for (const msg of streams) {
      if (!msg || typeof msg.call_id !== 'string') continue;
      if (!msg.call_id.startsWith(nodeUuid + '-')) continue;
      const entry: StreamEntry = {
        call_id: msg.call_id,
        kind: msg.kind === 'error' ? 'error' : 'message',
        data: msg.data,
      };
      const list = grouped[msg.call_id] ?? [];
      list.push(entry);
      grouped[msg.call_id] = list;
    }
    return grouped;
  }, [streams, nodeUuid]);

  const realtimeCount = useMemo(() => {
    let count = 0;
    for (const list of Object.values(realtimeByCallId)) {
      count += list.length;
    }
    return count;
  }, [realtimeByCallId]);

  switch (nodeType) {
    case 'grpc:connect':
      return (
        <ConnectSection
          report={report}
          output={output as GrpcConnectOutput | undefined}
        />
      );
    case 'grpc:call':
      return (
        <CallSection
          report={report}
          output={output as GrpcCallOutput | undefined}
          realtimeByCallId={realtimeByCallId}
          realtimeCount={realtimeCount}
        />
      );
    case 'grpc:close':
      return (
        <CloseSection
          report={report}
          output={output as GrpcCloseOutput | undefined}
        />
      );
    default:
      return (
        <div className="panel">
          <p className="muted italic">{t('viewer.grpc.unknown_node_type', { type: nodeType })}</p>
        </div>
      );
  }
}

// ============================================================================
// gRPC Connect viewer
// ============================================================================

function ConnectSection({
  report,
  output,
}: {
  report: NodeReport | null;
  output: GrpcConnectOutput | undefined;
}) {
  const resolvedConfig = report?.resolved_config as
    | GrpcConnectOutput['request']
    | undefined;
  const isSuccess = output?.success ?? report?.status === 'success';

  const requestConfig = output?.request || resolvedConfig;
  const configUrl = requestConfig?.url || output?.url || '';
  const connectDuration = output?.timing?.connect_ms;
  const totalDuration = output?.timing?.total_ms;
  const services = output?.services;
  const serviceCount = output?.service_count ?? services?.length ?? 0;

  return (
    <div className="viewer">
      {/* 连接配置 */}
      <CollapsibleSection
        title={t('viewer.common.connection_config')}
        icon={<IconCable size={15} />}
        defaultOpen={true}
      >
        <div className="col">
          <div className="row">
            <Badge className="badge-violet">gRPC</Badge>
            <code className="code grow" style={{ wordBreak: 'break-all' }}>
              {configUrl || <span className="muted italic">{t('viewer.common.config_url')}</span>}
            </code>
          </div>
          {requestConfig && (
            <div className="row" style={{ flexWrap: 'wrap' }}>
              {requestConfig.use_tls !== undefined && (
                <Badge className="badge-outline mono">
                  TLS: {requestConfig.use_tls ? t('viewer.common.tls_enabled') : t('viewer.common.tls_disabled')}
                </Badge>
              )}
              {requestConfig.tls_skip_verify && (
                <Badge className="badge-amber mono">{t('viewer.common.skip_cert_verify')}</Badge>
              )}
              {requestConfig.enable_reflection !== undefined && (
                <Badge className="badge-outline mono">
                  Reflection: {requestConfig.enable_reflection ? t('viewer.common.reflection_enabled') : t('viewer.common.reflection_disabled')}
                </Badge>
              )}
              {requestConfig.proto_files_count !== undefined && requestConfig.proto_files_count > 0 && (
                <Badge className="badge-outline mono">
                  {t('viewer.common.proto_files')}: {requestConfig.proto_files_count}
                </Badge>
              )}
              {requestConfig.connect_timeout_ms !== undefined && (
                <Badge className="badge-outline mono">
                  {t('viewer.common.timeout_label')}: {requestConfig.connect_timeout_ms}ms
                </Badge>
              )}
            </div>
          )}
        </div>
      </CollapsibleSection>

      {/* 连接状态 */}
      <CollapsibleSection
        title={t('viewer.grpc.section_connection')}
        icon={<IconGlobe size={15} />}
        defaultOpen={true}
      >
        <div className="col">
          <div className="row">
            <StatusBadge
              isSuccess={isSuccess}
              successText={t('viewer.grpc.status_connected')}
              failText={t('viewer.grpc.status_connection_failed')}
            />
            <span className="muted">
              {isSuccess
                ? t('viewer.grpc.connect_success')
                : output?.error ?? t('viewer.common.error_message')}
            </span>
          </div>
          {output?.connection_id && (
            <div className="row">
              <span className="muted">{t('viewer.common.connection_id_label')}:</span>
              <code className="code">{output.connection_id}</code>
            </div>
          )}
          {!isSuccess && output?.error && (
            <div className="alert alert-red">
              <span>{output.error}</span>
            </div>
          )}
        </div>
      </CollapsibleSection>

      {/* 服务发现 */}
      {isSuccess && services && services.length > 0 && (
        <CollapsibleSection
          title={t('viewer.grpc.section_services')}
          icon={<IconServer size={15} />}
          defaultOpen={false}
          extra={<Badge className="badge-outline mono">{t('viewer.grpc.services_count', { count: serviceCount })}</Badge>}
        >
          <div className="col">
            {services.map((service: GrpcServiceInfo) => (
              <div key={service.service_name} className="service-box" style={{ padding: 8 }}>
                <div className="service-name" style={{ marginBottom: 6 }}>
                  {service.service_name}
                </div>
                {service.methods.length > 0 ? (
                  <div className="col">
                    {service.methods.map((method: GrpcMethodInfo) => (
                      <div key={method.method_name} className="method-row">
                        <code className="method-name">{method.method_name}</code>
                        <span className="method-arrow">→</span>
                        <code className="method-type muted">{method.output_type}</code>
                        <StreamingBadgeView method={method} />
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="muted italic">{t('viewer.common.no_methods')}</p>
                )}
              </div>
            ))}
          </div>
        </CollapsibleSection>
      )}

      {/* 耗时分解 */}
      {(connectDuration !== undefined || totalDuration !== undefined) && (
        <CollapsibleSection
          title={t('viewer.common.timing_breakdown_short')}
          icon={<IconClock size={15} />}
          defaultOpen={false}
          extra={
            totalDuration !== undefined && (
              <span className="mono" style={{ fontSize: 12, color: 'var(--accent-strong)' }}>
                {t('viewer.common.total_label')} {totalDuration}ms
              </span>
            )
          }
        >
          <div className="timing-grid">
            {connectDuration !== undefined && (
              <TimingItem label={t('viewer.common.timing_connect')} valueMs={connectDuration} />
            )}
          </div>
        </CollapsibleSection>
      )}

      {/* 变量变更 */}
      <VariableChangesCard changes={report?.variable_changes} />
    </div>
  );
}

function StreamingBadgeView({ method }: { method: GrpcMethodInfo }) {
  if (method.is_server_streaming && method.is_client_streaming) {
    return <Badge className="badge-purple">{t('viewer.grpc.bidi_streaming')}</Badge>;
  }
  if (method.is_server_streaming) {
    return <Badge className="badge-amber">{t('viewer.grpc.server_streaming')}</Badge>;
  }
  if (method.is_client_streaming) {
    return <Badge className="badge-blue">{t('viewer.grpc.client_streaming')}</Badge>;
  }
  return <Badge className="badge-green">{t('viewer.grpc.unary_call')}</Badge>;
}

// ============================================================================
// gRPC Call viewer
// ============================================================================

/** `type.googleapis.com/google.rpc.RetryInfo` → `RetryInfo` */
function formatTypeUrl(typeUrl: string): { shortName: string; fullName: string } {
  const fullName = typeUrl.split('/').pop() ?? typeUrl;
  const parts = fullName.split('.');
  const shortName = parts.length > 0 ? parts[parts.length - 1] : fullName;
  return { shortName, fullName };
}

/** gRPC error-details card (google.rpc.Status structured details). */
function GrpcErrorDetailsCard({ details }: { details: GrpcErrorDetail[] }) {
  if (details.length === 0) return null;

  return (
    <CollapsibleSection
      title={t('viewer.grpc.section_errors') + ' (' + details.length + ')'}
      icon={<IconAlertTriangle size={15} />}
      defaultOpen={true}
    >
      <div className="col">
        {details.map((detail, idx) => {
          const { shortName, fullName } = formatTypeUrl(detail.type_url);
          return (
            <div key={idx} className="err-detail">
              <div className="err-detail-head">
                <Badge className="badge-amber mono">{shortName}</Badge>
                <code className="code muted truncate">{fullName}</code>
              </div>
              <div className="err-detail-body">
                {detail.decoded ? (
                  <div className="json-box">
                    <RawJsonViewer data={detail.decoded} />
                  </div>
                ) : detail.raw_hex ? (
                  <div>
                    <div className="muted" style={{ fontSize: 12, marginBottom: 4 }}>
                      {t('viewer.common.label_raw_data')}
                    </div>
                    <code className="code" style={{ display: 'block', wordBreak: 'break-all' }}>
                      {detail.raw_hex.length > 512
                        ? detail.raw_hex.slice(0, 512) + '...'
                        : detail.raw_hex}
                    </code>
                  </div>
                ) : (
                  <p className="muted italic">{t('viewer.grpc.no_message_body')}</p>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </CollapsibleSection>
  );
}

function RealtimeMessageItem({ message }: { message: StreamEntry & { index: number; isLast: boolean } }) {
  const [isOpen, setIsOpen] = useState(false);

  let parsedData: unknown;
  try {
    parsedData = typeof message.data === 'string' ? JSON.parse(message.data) : message.data;
  } catch {
    parsedData = message.data;
  }

  return (
    <div className="stream-item">
      <div className="stream-item-head">
        <Badge className="badge-outline mono">#{message.index}</Badge>
        {message.received_at_ms !== undefined && (
          <span className="stream-item-meta">+{message.received_at_ms}ms</span>
        )}
        {message.isLast && (
          <Badge className="badge-amber mono">{t('viewer.grpc.label_last_one')}</Badge>
        )}
        <button type="button" onClick={() => setIsOpen((p) => !p)} className="stream-item-toggle">
          {isOpen ? <IconChevronDown size={13} /> : <IconChevronRight size={13} />}
        </button>
      </div>
      {isOpen && (
        <div className="json-box">
          <RawJsonViewer data={parsedData} />
        </div>
      )}
    </div>
  );
}

function CallSection({
  report,
  output,
  realtimeByCallId,
  realtimeCount,
}: {
  report: NodeReport | null;
  output: GrpcCallOutput | undefined;
  realtimeByCallId: Record<string, StreamEntry[]>;
  realtimeCount: number;
}) {
  const resolvedConfig = report?.resolved_config as GrpcCallOutput['request'] | undefined;
  const isSuccess = output?.success ?? report?.status === 'success';

  const requestConfig = output?.request || resolvedConfig;
  const serviceName = output?.service_name || requestConfig?.service_name || '';
  const methodName = output?.method_name || requestConfig?.method_name || '';
  const callDuration = output?.timing?.call_ms;
  const totalDuration = output?.timing?.total_ms;
  const responseData = output?.data;
  const responseMetadata = output?.response_metadata;
  const trailers = output?.trailers;
  const isStreaming = output?.is_streaming ?? Array.isArray(responseData);
  const streamingResponses = output?.responses ?? [];
  const hasStreamingResponses = streamingResponses.length > 0;

  // Pattern label + color
  const pattern = (() => {
    switch (output?.pattern) {
      case 'unary':
        return <Badge className="badge-green mono">{t('viewer.grpc.unary_call')}</Badge>;
      case 'server_streaming':
        return <Badge className="badge-amber mono">{t('viewer.grpc.server_streaming')}</Badge>;
      case 'client_streaming':
        return <Badge className="badge-blue mono">{t('viewer.grpc.client_streaming')}</Badge>;
      case 'bidi_streaming':
        return <Badge className="badge-purple mono">{t('viewer.grpc.bidi_streaming')}</Badge>;
      default:
        return isStreaming ? (
          <Badge className="badge-amber mono">{t('viewer.grpc.server_streaming')}</Badge>
        ) : (
          <Badge className="badge-green mono">{t('viewer.grpc.unary_call')}</Badge>
        );
    }
  })();

  // Flatten realtime messages for rendering
  const realtimeMessages = useMemo(() => {
    const flat: Array<StreamEntry & { index: number; isLast: boolean }> = [];
    for (const list of Object.values(realtimeByCallId)) {
      list.forEach((entry, i) => {
        flat.push({ ...entry, index: i + 1, isLast: i === list.length - 1 });
      });
    }
    return flat;
  }, [realtimeByCallId]);

  const realtimeError = useMemo(() => {
    for (const list of Object.values(realtimeByCallId)) {
      const err = list.find((e) => e.kind === 'error');
      if (err) return err;
    }
    return null;
  }, [realtimeByCallId]);

  return (
    <div className="viewer">
      {/* 调用配置 */}
      <CollapsibleSection
        title={t('viewer.grpc.section_call_config')}
        icon={<IconRadio size={15} />}
        defaultOpen={true}
      >
        <div className="col">
          <div className="row">
            {pattern}
            <code className="code grow" style={{ wordBreak: 'break-all' }}>
              {serviceName && methodName ? (
                serviceName + '/' + methodName
              ) : (
                <span className="muted italic">{t('viewer.grpc.no_service_method')}</span>
              )}
            </code>
          </div>
          {requestConfig && (
            <div className="row" style={{ flexWrap: 'wrap' }}>
              {requestConfig.connection_id && (
                <Badge className="badge-outline mono">
                  {t('viewer.grpc.label_connection')}: {requestConfig.connection_id}
                </Badge>
              )}
              {requestConfig.timeout_ms !== undefined && (
                <Badge className="badge-outline mono">
                  {t('viewer.grpc.label_timeout')}: {requestConfig.timeout_ms}ms
                </Badge>
              )}
              {requestConfig.metadata_count !== undefined && requestConfig.metadata_count > 0 && (
                <Badge className="badge-outline mono">
                  {t('viewer.grpc.label_metadata_count')}: {requestConfig.metadata_count}
                </Badge>
              )}
            </div>
          )}
          {requestConfig?.request_json && (
            <div>
              <div className="field-label-sm" style={{ marginBottom: 4 }}>
                {t('viewer.grpc.label_request_body')}
              </div>
              <div className="json-box">
                <RawJsonViewer data={requestConfig.request_json} />
              </div>
            </div>
          )}
        </div>
      </CollapsibleSection>

      {/* 响应状态 */}
      <CollapsibleSection
        title={t('viewer.grpc.section_response_status')}
        icon={<IconGlobe size={15} />}
        defaultOpen={true}
      >
        <div className="col">
          <div className="row">
            <StatusBadge
              isSuccess={isSuccess}
              successText={output?.status ?? 'OK'}
              failText={output?.status ?? 'Error'}
            />
            <span className="muted">
              {isSuccess
                ? t('viewer.grpc.call_success_msg', { servicePath: serviceName + '/' + methodName })
                : output?.status_message ?? output?.error ?? t('viewer.grpc.call_failed_details')}
            </span>
          </div>
          {!isSuccess && output?.error && (
            <div className="alert alert-red">
              <span>{output.error}</span>
            </div>
          )}
        </div>
      </CollapsibleSection>

      {/* 错误详情 */}
      {!isSuccess && output?.error_details && output.error_details.length > 0 && (
        <GrpcErrorDetailsCard details={output.error_details} />
      )}

      {/* 流式统计 */}
      {(output?.sent_count !== undefined || output?.received_count !== undefined) && (
        <CollapsibleSection
          title={t('viewer.grpc.section_streaming_stats')}
          icon={<IconServer size={15} />}
          defaultOpen={true}
        >
          <div className="timing-grid">
            {output?.sent_count !== undefined && (
              <div className="timing-item">
                <span className="timing-label">{t('viewer.grpc.label_sent_count')}</span>
                <span className="mono" style={{ color: 'var(--info)' }}>{output.sent_count}</span>
              </div>
            )}
            {output?.received_count !== undefined && (
              <div className="timing-item">
                <span className="timing-label">{t('viewer.grpc.label_received_count')}</span>
                <span className="mono" style={{ color: 'var(--ok)' }}>{output.received_count}</span>
              </div>
            )}
            {output?.streaming_timing?.first_response_ms !== undefined && (
              <TimingItem
                label={t('viewer.grpc.label_first_response_time')}
                valueMs={output.streaming_timing.first_response_ms}
              />
            )}
            {output?.streaming_timing?.total_ms !== undefined && (
              <TimingItem
                label={t('viewer.grpc.label_streaming_total_time')}
                valueMs={output.streaming_timing.total_ms}
              />
            )}
          </div>
        </CollapsibleSection>
      )}

      {/* 实时流式消息 */}
      {realtimeCount > 0 && !hasStreamingResponses && (
        <CollapsibleSection
          title={t('viewer.grpc.section_streaming_stats') + ' (' + realtimeCount + ')'}
          icon={<IconServer size={15} />}
          defaultOpen={true}
          extra={
            <div className="row">
              <span className="mono" style={{ fontSize: 12, color: 'var(--ok)' }}>
                <span className="pulse">{t('viewer.grpc.label_realtime')}</span>
              </span>
              <Badge className="badge-outline mono">{realtimeCount}</Badge>
            </div>
          }
        >
          <div className="stream-list">
            {realtimeMessages.map((msg, idx) => (
              <RealtimeMessageItem key={msg.call_id + '-' + idx} message={msg} />
            ))}
          </div>
          {realtimeError && (
            <div className="stream-error">{String((realtimeError.data as { message?: string })?.message ?? realtimeError.data)}</div>
          )}
        </CollapsibleSection>
      )}

      {/* 单响应（Unary / Client Streaming） */}
      {responseData !== undefined && responseData !== null && !hasStreamingResponses && (
        <CollapsibleSection
          title={t('viewer.grpc.section_response')}
          icon={<IconGlobe size={15} />}
          defaultOpen={false}
        >
          <div className="json-box">
            <RawJsonViewer data={responseData} />
          </div>
        </CollapsibleSection>
      )}

      {/* 流式响应列表（Server / Bidi Streaming） */}
      {hasStreamingResponses && (
        <CollapsibleSection
          title={t('viewer.grpc.section_streaming_responses') + ' (' + streamingResponses.length + ')'}
          icon={<IconGlobe size={15} />}
          defaultOpen={false}
        >
          <div className="col">
            {streamingResponses.map((resp, idx) => (
              <div key={idx} className="stream-item">
                <div className="stream-item-head">
                  <Badge className="badge-outline mono">#{resp.index ?? idx + 1}</Badge>
                  {resp.received_at_ms !== undefined && (
                    <span className="stream-item-meta">+{resp.received_at_ms}ms</span>
                  )}
                  {resp.size_bytes !== undefined && (
                    <span className="stream-item-meta">{formatSizeBytes(resp.size_bytes)}</span>
                  )}
                </div>
                <div className="json-box">
                  <RawJsonViewer data={resp.data} />
                </div>
              </div>
            ))}
          </div>
        </CollapsibleSection>
      )}

      {/* Legacy: array response data fallback */}
      {responseData !== undefined &&
        responseData !== null &&
        hasStreamingResponses &&
        Array.isArray(responseData) && (
          <CollapsibleSection
            title={t('viewer.grpc.section_response') + ' (' + responseData.length + ')'}
            icon={<IconGlobe size={15} />}
            defaultOpen={false}
          >
            <div className="json-box">
              <RawJsonViewer data={responseData} />
            </div>
          </CollapsibleSection>
        )}

      {/* 响应 Metadata */}
      {responseMetadata && Object.keys(responseMetadata).length > 0 && (
        <CollapsibleSection
          title={t('viewer.grpc.section_response_metadata')}
          icon={<IconServer size={15} />}
          defaultOpen={false}
        >
          <KeyValueList data={responseMetadata} />
        </CollapsibleSection>
      )}

      {/* Trailers */}
      {trailers && Object.keys(trailers).length > 0 && (
        <CollapsibleSection
          title={t('viewer.grpc.section_trailers')}
          icon={<IconServer size={15} />}
          defaultOpen={false}
        >
          <KeyValueList data={trailers} />
        </CollapsibleSection>
      )}

      {/* 耗时分解 */}
      {(callDuration !== undefined || totalDuration !== undefined) && (
        <CollapsibleSection
          title={t('viewer.common.duration_decomposition')}
          icon={<IconClock size={15} />}
          defaultOpen={false}
          extra={
            totalDuration !== undefined && (
              <span className="mono" style={{ fontSize: 12, color: 'var(--accent-strong)' }}>
                {t('viewer.common.total_duration_ms', { ms: totalDuration })}
              </span>
            )
          }
        >
          <div className="timing-grid">
            {callDuration !== undefined && (
              <TimingItem label={t('viewer.common.timing_call')} valueMs={callDuration} />
            )}
          </div>
        </CollapsibleSection>
      )}

      {/* 变量变更 */}
      <VariableChangesCard changes={report?.variable_changes} />
    </div>
  );
}

// ============================================================================
// gRPC Close viewer
// ============================================================================

function CloseSection({
  report,
  output,
}: {
  report: NodeReport | null;
  output: GrpcCloseOutput | undefined;
}) {
  const resolvedConfig = report?.resolved_config as GrpcCloseOutput['request'] | undefined;
  const isSuccess = output?.success ?? report?.status === 'success';

  const requestConfig = output?.request || resolvedConfig;
  const connectionId = output?.connection_id || requestConfig?.connection_id || '';
  const closeDuration = output?.timing?.close_ms;
  const totalDuration = output?.timing?.total_ms;

  return (
    <div className="viewer">
      {/* 关闭配置 */}
      <CollapsibleSection
        title={t('viewer.grpc.section_close_config')}
        icon={<IconUnplug size={15} />}
        defaultOpen={true}
      >
        <div className="row">
          <Badge className="badge-violet">gRPC</Badge>
          <code className="code grow" style={{ wordBreak: 'break-all' }}>
            {connectionId || <span className="muted italic">{t('viewer.grpc.no_connection_id')}</span>}
          </code>
        </div>
      </CollapsibleSection>

      {/* 关闭状态 */}
      <CollapsibleSection
        title={t('viewer.grpc.section_close')}
        icon={<IconGlobe size={15} />}
        defaultOpen={true}
      >
        <div className="col">
          <div className="row">
            <StatusBadge
              isSuccess={isSuccess}
              successText={t('viewer.grpc.status_closed')}
              failText={t('viewer.grpc.status_close_failed')}
            />
            <span className="muted">
              {isSuccess ? t('viewer.grpc.close_success') : output?.error ?? t('viewer.grpc.close_error')}
            </span>
          </div>
          {!isSuccess && output?.error && (
            <div className="alert alert-red">
              <span>{output.error}</span>
            </div>
          )}
        </div>
      </CollapsibleSection>

      {/* 耗时分解 */}
      {(closeDuration !== undefined || totalDuration !== undefined) && (
        <CollapsibleSection
          title={t('viewer.common.duration_decomposition')}
          icon={<IconClock size={15} />}
          defaultOpen={false}
          extra={
            totalDuration !== undefined && (
              <span className="mono" style={{ fontSize: 12, color: 'var(--accent-strong)' }}>
                {t('viewer.common.total_duration_ms', { ms: totalDuration })}
              </span>
            )
          }
        >
          <div className="timing-grid">
            {closeDuration !== undefined && (
              <TimingItem label={t('viewer.common.timing_close')} valueMs={closeDuration} />
            )}
          </div>
        </CollapsibleSection>
      )}

      {/* 变量变更 */}
      <VariableChangesCard changes={report?.variable_changes} />
    </div>
  );
}
