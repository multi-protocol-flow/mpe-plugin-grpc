//! 插件本地 gRPC 数据类型（从宿主 `flow-engine-core::traits` 迁移）。
//!
//! 插件是独立进程，绝不 import 宿主类型；这些结构体保持与宿主侧
//! 字节级兼容的 serde 形状（discovered_services 在节点 config 里往返、
//! 报告 output_data 形状、错误详情 JSON 等），前端无需任何改动即可
//! 继续消费。

/// gRPC 通信模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrpcPattern {
    /// Unary（一问一答）
    Unary,
    /// Server Streaming（一问多答）
    ServerStreaming,
    /// Client Streaming（多问一答）
    ClientStreaming,
    /// Bidi Streaming（多问多答）
    BidiStreaming,
}

impl GrpcPattern {
    /// 根据方法信息推断通信模式
    pub fn from_method_info(is_client_streaming: bool, is_server_streaming: bool) -> Self {
        match (is_client_streaming, is_server_streaming) {
            (false, false) => GrpcPattern::Unary,
            (false, true) => GrpcPattern::ServerStreaming,
            (true, false) => GrpcPattern::ClientStreaming,
            (true, true) => GrpcPattern::BidiStreaming,
        }
    }
}

/// gRPC 计时信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GrpcTimingInfo {
    /// 总耗时（毫秒）
    pub total_ms: u64,
    /// 首个响应耗时（毫秒）
    pub first_response_ms: Option<u64>,
}

/// gRPC 流式结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GrpcStreamingResult {
    /// gRPC 状态码
    pub status: i32,
    /// 响应列表
    pub responses: Vec<serde_json::Value>,
    /// 已发送消息数
    pub sent_count: usize,
    /// 已接收消息数
    pub received_count: usize,
    /// Trailers metadata
    pub trailers: std::collections::HashMap<String, String>,
    /// 响应 Initial Metadata（从 tonic Response 提取的初始 metadata）
    #[serde(default)]
    pub response_metadata: Vec<(String, String)>,
    /// 计时信息
    pub timing: GrpcTimingInfo,
    /// 是否被用户取消
    #[serde(default)]
    pub cancelled: bool,
}

/// gRPC 枚举值
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrpcEnumValue {
    /// 枚举值名称
    pub name: String,
    /// 枚举值编号
    pub number: i32,
}

/// gRPC oneof 分组
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrpcOneofInfo {
    /// oneof 组名
    pub name: String,
    /// 属于该 oneof 组的字段编号列表
    pub field_numbers: Vec<u32>,
}

/// gRPC 消息定义
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrpcMessageInfo {
    /// 消息全名（如 "package.MessageName"）
    pub full_name: String,
    /// 字段列表
    pub fields: Vec<GrpcFieldInfo>,
    /// oneof 分组列表
    pub oneof_groups: Vec<GrpcOneofInfo>,
    /// 是否为 Well-Known Type（google.protobuf.*）
    pub is_wkt: bool,
    /// 保留字段编号范围
    pub reserved_ranges: Vec<(u32, u32)>,
    /// 保留字段名称
    pub reserved_names: Vec<String>,
}

/// gRPC 字段信息
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrpcFieldInfo {
    /// 字段名
    pub name: String,
    /// 字段编号
    pub number: u32,
    /// 类型分类标识（如 "string", "int32", "MESSAGE", "ENUM"）
    pub type_kind: String,
    /// 类型显示名（如 "string", "map<string, int32>"）
    pub type_display: String,
    /// 消息/枚举类型的全名（仅 `type_kind` 为 MESSAGE/ENUM 时存在）
    #[serde(default)]
    pub type_full_name: Option<String>,
    /// 字段 cardinality（"optional" / "required" / "repeated"）
    pub label: String,
    /// 是否为 map 类型
    #[serde(default)]
    pub is_map: bool,
    /// map 的 key 类型（仅 `is_map=true` 时存在）
    #[serde(default)]
    pub map_key_type: Option<String>,
    /// map 的 value 类型（仅 `is_map=true` 时存在）
    #[serde(default)]
    pub map_value_type: Option<String>,
    /// 枚举值列表（仅 `type_kind` 为 ENUM 时非空）
    #[serde(default)]
    pub enum_values: Vec<GrpcEnumValue>,
    /// 嵌套消息定义（仅 `type_kind` 为 MESSAGE 时存在）
    #[serde(default)]
    pub nested_message: Option<Box<GrpcMessageInfo>>,
}

/// gRPC 服务信息（service + method 列表）
#[derive(Debug, Clone, PartialEq)]
pub struct GrpcServiceInfo {
    /// 服务全名（如 "package.ServiceName"）
    pub service_name: String,
    /// 方法列表
    pub methods: Vec<GrpcMethodInfo>,
    /// Flat map of all message definitions keyed by `full_name` (e.g. "package.MessageName")
    ///
    /// Wrapped in `Arc` to share the same definition map across all services
    /// from the same gRPC connection, avoiding N deep-copies for N services.
    pub message_definitions: std::sync::Arc<std::collections::HashMap<String, GrpcMessageInfo>>,
}

impl serde::Serialize for GrpcServiceInfo {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("GrpcServiceInfo", 3)?;
        state.serialize_field("service_name", &self.service_name)?;
        state.serialize_field("methods", &self.methods)?;
        state.serialize_field("message_definitions", self.message_definitions.as_ref())?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for GrpcServiceInfo {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct GrpcServiceInfoHelper {
            service_name: String,
            methods: Vec<GrpcMethodInfo>,
            #[serde(default)]
            message_definitions: std::collections::HashMap<String, GrpcMessageInfo>,
        }
        let helper = GrpcServiceInfoHelper::deserialize(deserializer)?;
        Ok(GrpcServiceInfo {
            service_name: helper.service_name,
            methods: helper.methods,
            message_definitions: std::sync::Arc::new(helper.message_definitions),
        })
    }
}

/// gRPC 方法信息
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrpcMethodInfo {
    /// 方法名
    pub method_name: String,
    /// 输入类型全名
    pub input_type: String,
    /// 输出类型全名
    pub output_type: String,
    /// 是否为服务端流式
    pub is_server_streaming: bool,
    /// 是否为客户端流式
    #[serde(default)]
    pub is_client_streaming: bool,
}

/// gRPC 流式消息（用于实时流式传递）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GrpcStreamMessage {
    /// 消息序号（从 1 开始）
    pub index: usize,
    /// 消息数据
    pub data: serde_json::Value,
    /// 接收时间戳（毫秒）
    pub received_at_ms: u64,
    /// 消息大小（字节）
    pub size_bytes: usize,
}

/// 健康检查响应
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthCheckResponse {
    /// 服务状态："UNKNOWN", "SERVING", "NOT_SERVING", "NOT_IMPLEMENTED"
    pub status: String,
}

/// gRPC Error详情
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GrpcErrorDetail {
    /// Error详情类型 URL
    pub type_url: String,
    /// 解码后的Error详情
    pub decoded: Option<serde_json::Value>,
    /// 原始十六进制数据
    pub raw_hex: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grpc_pattern_from_method_info() {
        assert_eq!(GrpcPattern::from_method_info(false, false), GrpcPattern::Unary);
        assert_eq!(
            GrpcPattern::from_method_info(false, true),
            GrpcPattern::ServerStreaming
        );
        assert_eq!(
            GrpcPattern::from_method_info(true, false),
            GrpcPattern::ClientStreaming
        );
        assert_eq!(
            GrpcPattern::from_method_info(true, true),
            GrpcPattern::BidiStreaming
        );
    }

    #[test]
    fn test_grpc_pattern_serde_roundtrip() {
        for pattern in [
            GrpcPattern::Unary,
            GrpcPattern::ServerStreaming,
            GrpcPattern::ClientStreaming,
            GrpcPattern::BidiStreaming,
        ] {
            let json = serde_json::to_string(&pattern).expect("serialize");
            let back: GrpcPattern = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(pattern, back);
        }
    }

    #[test]
    fn test_grpc_service_info_roundtrip() {
        let info = GrpcServiceInfo {
            service_name: "pkg.MyService".to_string(),
            methods: vec![GrpcMethodInfo {
                method_name: "DoSomething".to_string(),
                input_type: "pkg.Input".to_string(),
                output_type: "pkg.Output".to_string(),
                is_server_streaming: false,
                is_client_streaming: false,
            }],
            message_definitions: std::sync::Arc::new(std::collections::HashMap::new()),
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let back: GrpcServiceInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(info, back);
    }

    #[test]
    fn test_grpc_service_info_backward_compat() {
        let old_json = r#"{
            "service_name": "pkg.MyService",
            "methods": [{
                "method_name": "DoSomething",
                "input_type": "pkg.Input",
                "output_type": "pkg.Output",
                "is_server_streaming": false
            }]
        }"#;
        let info: GrpcServiceInfo = serde_json::from_str(old_json).expect("deserialize");
        assert_eq!(info.service_name, "pkg.MyService");
        assert!(
            info.message_definitions.is_empty(),
            "missing message_definitions should default to empty map"
        );
    }
}
