/**
 * Local type definitions for the gRPC plugin frontend.
 *
 * Config shapes mirror the plugin's Rust pool/executor parsing (snake_case).
 * GrpcServiceInfo/GrpcMethodInfo/GrpcMessageInfo/GrpcFieldInfo mirror the
 * backend's proto_parser output (and the former host `@/types`).
 */

// ============================================================================
// Wire-level message payloads
// ============================================================================

/** `init` message payload (host -> iframe). */
export interface PluginIframeInitPayload {
  config: Record<string, unknown>;
  variables: Record<string, unknown>;
  host_api?: Record<string, unknown>;
  nodeType: string;
  locale: string;
  nodes?: GrpcNodeSnapshot[];
  /** viewer only: the execution report node object */
  node_report?: NodeReport;
}

/** `stream` message payload (host -> viewer iframe, realtime). */
export interface PluginStreamMessage {
  call_id: string;
  kind: 'message' | 'error';
  data: unknown;
}

// ============================================================================
// Config shapes (snake_case, matches plugin Rust side)
// ============================================================================

export interface GrpcMetadataEntry {
  key: string;
  value: string;
}

export interface ProtoFile {
  path: string;
  content: string;
}

export interface StreamMessage {
  enabled: boolean;
  content: string;
}

export interface GrpcConnectConfig {
  type?: string;
  url: string;
  use_tls?: boolean;
  tls_skip_verify?: boolean;
  enable_reflection?: boolean;
  proto_files?: ProtoFile[];
  default_metadata?: GrpcMetadataEntry[];
  connect_timeout_ms?: number;
  discovered_services?: GrpcServiceInfo[];
  tls_ca_cert?: string | null;
  tls_client_cert?: string | null;
  tls_client_key?: string | null;
  tls_server_name_override?: string | null;
  compression_encoding?: string | null;
  keepalive_time_ms?: number | null;
  keepalive_timeout_ms?: number | null;
  keepalive_permit_without_streams?: boolean | null;
  reflection_metadata?: GrpcMetadataEntry[] | null;
  health_check_service?: string | null;
  endpoints?: string[] | null;
  max_retries?: number | null;
  initial_backoff_ms?: number | null;
}

export interface GrpcCallConfig {
  type?: string;
  connection_id: string;
  service_name: string;
  method_name: string;
  request_json: string;
  timeout_ms?: number;
  metadata?: GrpcMetadataEntry[];
  request_messages?: StreamMessage[];
  compression_encoding?: string | null;
}

export interface GrpcCloseConfig {
  type?: string;
  connection_id: string;
}

/** Snapshot of another node on the canvas (init.nodes element). */
export interface GrpcNodeSnapshot {
  uuid: string;
  label: string;
  type: string;
  /** full config of that node (includes a `type` field per host injection) */
  config: Record<string, unknown>;
}

// ============================================================================
// Discovered services (backend proto_parser output)
// ============================================================================

export interface GrpcEnumValue {
  name: string;
  number: number;
}

export interface GrpcOneofInfo {
  name: string;
  field_numbers: number[];
}

export interface GrpcFieldInfo {
  name: string;
  number: number;
  type_kind: string;
  type_display: string;
  type_full_name?: string;
  label: string;
  is_map: boolean;
  map_key_type?: string;
  map_value_type?: string;
  enum_values: GrpcEnumValue[];
  nested_message?: GrpcMessageInfo;
}

export interface GrpcMessageInfo {
  full_name: string;
  fields: GrpcFieldInfo[];
  oneof_groups: GrpcOneofInfo[];
  is_wkt: boolean;
  reserved_ranges?: [number, number][];
  reserved_names?: string[];
}

export interface GrpcMethodInfo {
  method_name: string;
  input_type: string;
  output_type: string;
  is_server_streaming: boolean;
  is_client_streaming?: boolean;
}

export interface GrpcServiceInfo {
  service_name: string;
  methods: GrpcMethodInfo[];
  message_definitions?: Record<string, GrpcMessageInfo>;
}

export type GrpcPattern = 'unary' | 'server_streaming' | 'client_streaming' | 'bidi_streaming';

// ============================================================================
// Design-time query results (uiCall responses)
// ============================================================================

export interface DiscoverResult {
  success: boolean;
  services: GrpcServiceInfo[];
  error: string | null;
}

export interface ReadProtoFilesResult {
  success: boolean;
  files: ProtoFile[];
  import_path?: string | null;
  error: string | null;
}

export interface ParseDescriptorSetResult {
  success: boolean;
  services: GrpcServiceInfo[];
  error: string | null;
}

export interface SkeletonResult {
  success: boolean;
  skeleton: string;
  error: string | null;
}

export interface ValidationIssue {
  path: string;
  message: string;
}

export interface ValidateResult {
  success: boolean;
  result: { valid: boolean; errors: ValidationIssue[] } | null;
  error: string | null;
}

export interface ChannelzInfo {
  state: string;
  connected_at: number;
  uptime_secs: number;
  active_calls: number;
  last_error: string | null;
}

export interface ChannelzResult {
  success: boolean;
  info: ChannelzInfo | null;
  error: string | null;
}

// ============================================================================
// Report viewer types
// ============================================================================

export interface GrpcErrorDetail {
  type_url: string;
  decoded?: Record<string, unknown> | null;
  raw_hex?: string | null;
}

export interface GrpcConnectOutput {
  success?: boolean;
  connection_id?: string | null;
  url?: string;
  request?: {
    url?: string;
    use_tls?: boolean;
    tls_skip_verify?: boolean;
    enable_reflection?: boolean;
    proto_files_count?: number;
    default_metadata_count?: number;
    connect_timeout_ms?: number;
  };
  timing?: {
    connect_ms?: number;
    total_ms?: number;
  };
  services?: GrpcServiceInfo[];
  service_count?: number;
  error?: string;
}

export interface GrpcCallOutput {
  success?: boolean;
  pattern?: string;
  connection_id?: string;
  service_name?: string;
  method_name?: string;
  request?: {
    connection_id?: string;
    service_name?: string;
    method_name?: string;
    request_json?: string;
    timeout_ms?: number;
    metadata_count?: number;
  };
  timing?: {
    call_ms?: number;
    total_ms?: number;
  };
  data?: unknown;
  responses?: Array<{ index?: number; data?: unknown; received_at_ms?: number; size_bytes?: number }>;
  sent_count?: number;
  received_count?: number;
  status?: string;
  status_message?: string;
  response_metadata?: Record<string, unknown>;
  trailers?: Record<string, unknown>;
  is_streaming?: boolean;
  streaming_timing?: { total_ms?: number; first_response_ms?: number };
  error?: string;
  error_details?: GrpcErrorDetail[];
}

export interface GrpcCloseOutput {
  success?: boolean;
  connection_id?: string;
  request?: { connection_id?: string };
  timing?: { close_ms?: number; total_ms?: number };
  error?: string;
}

export interface VariableChange {
  name: string;
  before?: unknown;
  after?: unknown;
  kind?: string;
}

export interface NodeReport {
  node_uuid?: string;
  node_type?: string;
  node_name?: string;
  status?: string;
  output_data?: unknown;
  plugin_data?: unknown;
  resolved_config?: unknown;
  variable_changes?: VariableChange[] | Record<string, unknown>[];
}
