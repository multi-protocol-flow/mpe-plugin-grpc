//! 辅助函数
//!
//! gRPC 路径构建、JSON ↔ DynamicMessage 转换、metadata 合并等工具函数。
//! （从宿主 flow-engine-grpc 原样迁移）

use http::uri::PathAndQuery;
use prost_reflect::{DynamicMessage, SerializeOptions};
use serde_json::de::Deserializer;

/// 构建 gRPC 方法路径
///
/// gRPC 路径格式为 `/{package.Service}/{Method}`
pub(crate) fn build_method_path(
    service_name: &str,
    method_name: &str,
) -> Result<PathAndQuery, String> {
    let path = format!("/{}/{}", service_name, method_name);
    path.parse()
        .map_err(|e| format!("Invalid gRPC path '{}': {}", path, e))
}

/// 将 JSON 字符串反序列化为 `DynamicMessage`
pub(crate) fn json_to_dynamic_message(
    request_json: &str,
    descriptor: prost_reflect::MessageDescriptor,
) -> Result<DynamicMessage, String> {
    let mut deserializer = Deserializer::from_str(request_json);
    let msg = DynamicMessage::deserialize(descriptor, &mut deserializer)
        .map_err(|e| format!("Failed to deserialize request message: {}", e))?;
    deserializer
        .end()
        .map_err(|e| format!("Request JSON contains extra data: {}", e))?;
    Ok(msg)
}

/// 将 `DynamicMessage` 序列化为 JSON Value
pub(crate) fn dynamic_message_to_json(msg: &DynamicMessage) -> Result<serde_json::Value, String> {
    let options = SerializeOptions::new().skip_default_fields(false);
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::new(&mut buf);
    msg.serialize_with_options(&mut ser, &options)
        .map_err(|e| format!("Failed to serialize response message: {}", e))?;
    serde_json::from_slice(&buf).map_err(|e| format!("Failed to parse response JSON: {}", e))
}

/// 合并连接级默认 metadata 与调用级 metadata
///
/// 调用级 metadata 同名 key 覆盖连接级（后者优先）。
pub(crate) fn merge_metadata(
    default_metadata: &[(String, String)],
    call_metadata: Vec<(String, String)>,
) -> Vec<(String, String)> {
    default_metadata
        .iter()
        .filter(|(k, _)| !call_metadata.iter().any(|(mk, _)| mk == k))
        .chain(call_metadata.iter())
        .cloned()
        .collect()
}
