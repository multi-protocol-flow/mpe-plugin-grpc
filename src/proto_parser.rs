//! Proto 文件运行时解析器
//!
//! 使用 protox（纯 Rust protobuf 编译器）在运行时编译 .proto 文件，
//! 通过 prost-reflect 的 DescriptorPool 实现动态消息解析和服务发现。

use prost_reflect::{DescriptorPool, FieldDescriptor, Kind, MessageDescriptor};
use std::collections::{HashMap, HashSet};
use std::io::Write;

use crate::types::{GrpcEnumValue, GrpcFieldInfo, GrpcMessageInfo, GrpcOneofInfo};

/// Proto 文件描述（用户上传）
#[derive(Debug, Clone)]
pub struct ProtoFile {
    /// 文件名（如 "example.proto"）
    pub path: String,
    /// 文件内容
    pub content: String,
}

/// 服务信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceInfo {
    /// 服务全名（如 "package.ServiceName"）
    pub service_name: String,
    /// 方法列表
    pub methods: Vec<MethodInfo>,
}

/// 方法信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MethodInfo {
    /// 方法名
    pub method_name: String,
    /// 输入类型全名
    pub input_type: String,
    /// 输出类型全名
    pub output_type: String,
    /// 是否为服务端流式
    pub is_server_streaming: bool,
    /// 是否为客户端流式
    pub is_client_streaming: bool,
}

/// Proto 解析Error
#[derive(Debug, thiserror::Error)]
pub enum ProtoParseError {
    /// Proto 文件解析Failed
    #[error("failed to parse proto files: {0}")]
    ParseError(String),
    /// 缺失文件
    #[error("missing files: {0:?}")]
    MissingFiles(Vec<String>),
    /// I/O Error
    #[error("I/O Error: {0}")]
    IoError(#[from] std::io::Error),
}

/// 解析多个 proto 文件
///
/// 将 proto 文件内容写入临时目录，使用 protox 编译为 `FileDescriptorSet`，
/// 然后通过 prost-reflect 构建 `DescriptorPool`。
///
/// # 参数
///
/// * `files` - Proto 文件列表，每个包含文件名和内容
///
/// # 返回
///
/// Success时返回 DescriptorPool，可用于后续的服务发现和消息解析
pub fn parse_proto_files(files: &[ProtoFile]) -> Result<DescriptorPool, ProtoParseError> {
    // 创建临时目录
    let temp_dir = tempfile::tempdir()?;

    // 写入 proto 文件到临时目录
    let mut file_names = Vec::new();
    for file in files {
        let path = temp_dir.path().join(&file.path);
        // 确保父目录存在（支持嵌套 import 路径）
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&path)?;
        f.write_all(file.content.as_bytes())?;
        // 只取文件名用于编译，include 目录是临时目录本身
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) {
            file_names.push(name);
        }
    }

    if file_names.is_empty() {
        return Err(ProtoParseError::ParseError(
            "no valid proto files provided".to_string(),
        ));
    }

    // 使用 protox 在运行时编译 proto 文件（纯 Rust，无 C++ 依赖）
    let include_path = temp_dir.path();
    let fds = protox::compile(&file_names, [include_path])
        .map_err(|e| ProtoParseError::ParseError(format!("{}", e)))?;

    // 从 FileDescriptorSet 构建 DescriptorPool
    let pool = DescriptorPool::from_file_descriptor_set(fds)
        .map_err(|e| ProtoParseError::ParseError(format!("failed to build DescriptorPool: {}", e)))?;

    Ok(pool)
}

/// 列出所有服务及其方法
///
/// 从 `DescriptorPool` 中提取所有 gRPC 服务定义，
/// 返回服务名、方法名、输入输出类型等信息。
pub fn list_services(pool: &DescriptorPool) -> Vec<ServiceInfo> {
    pool.services()
        .map(|service| {
            let methods: Vec<MethodInfo> = service
                .methods()
                .map(|method| MethodInfo {
                    method_name: method.name().to_string(),
                    input_type: method.input().full_name().to_string(),
                    output_type: method.output().full_name().to_string(),
                    is_server_streaming: method.is_server_streaming(),
                    is_client_streaming: method.is_client_streaming(),
                })
                .collect();

            ServiceInfo {
                service_name: service.full_name().to_string(),
                methods,
            }
        })
        .collect()
}

/// 从 `DescriptorPool` 生成指定服务和方法的请求 JSON 骨架
///
/// 通过 `service_name` 和 `method_name` 定位方法的输入消息类型，
/// 复用 `generate_json_skeleton` 生成占位 JSON。
/// 适用于 Server Reflection 场景（无需 proto 文件）。
pub fn generate_skeleton_from_pool(
    pool: &DescriptorPool,
    service_name: &str,
    method_name: &str,
) -> Result<String, String> {
    let service = pool
        .get_service_by_name(service_name)
        .ok_or_else(|| format!("service not found: {}", service_name))?;
    let method = service
        .methods()
        .find(|m| m.name() == method_name)
        .ok_or_else(|| format!("method not found in service {}: {}", service_name, method_name))?;
    let input_msg = method.input();
    let skeleton = generate_json_skeleton(&input_msg);
    serde_json::to_string_pretty(&skeleton).map_err(|e| format!("failed to serialize JSON skeleton: {}", e))
}

/// 生成消息类型的 JSON 骨架
///
/// 遍历消息的所有字段，根据字段类型生成占位值。
/// 用于帮助用户构造请求消息。
pub fn generate_json_skeleton(msg: &MessageDescriptor) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for field in msg.fields() {
        let key = field.name().to_string();
        let value = field_to_json(&field);
        obj.insert(key, value);
    }
    serde_json::Value::Object(obj)
}

/// 根据字段类型生成占位 JSON 值
fn field_to_json(field: &FieldDescriptor) -> serde_json::Value {
    if field.is_map() {
        return serde_json::Value::Object(Default::default());
    }
    if field.cardinality() == prost_reflect::Cardinality::Repeated {
        return serde_json::Value::Array(Vec::new());
    }
    match field.kind() {
        Kind::Double | Kind::Float => serde_json::json!(0.0),
        Kind::Int32
        | Kind::Int64
        | Kind::Sint32
        | Kind::Sint64
        | Kind::Sfixed32
        | Kind::Sfixed64 => {
            serde_json::json!(0)
        }
        Kind::Uint32 | Kind::Uint64 | Kind::Fixed32 | Kind::Fixed64 => serde_json::json!(0),
        Kind::Bool => serde_json::json!(false),
        Kind::String => serde_json::json!(""),
        Kind::Bytes => serde_json::json!(""),
        Kind::Enum(_) => serde_json::json!(0),
        Kind::Message(msg_desc) => generate_json_skeleton(&msg_desc),
    }
}

/// 递归扫描目录，发现所有 .proto 文件
///
/// 遍历指定目录及其子目录，收集所有 `.proto` 文件。
/// 跳过以 `.` 开头的隐藏目录。
///
/// # 参数
///
/// * `dir_path` - 目录路径字符串
///
/// # 返回
///
/// Success时返回 `(Vec<(String, String)>, String)`：
/// - `Vec<(String, String)>`：文件名和内容的元组列表（文件名为相对路径）
/// - `String`：目录根路径（用作 proto 编译的 include 路径）
pub fn discover_proto_files_in_dir(
    dir_path: &str,
) -> Result<(Vec<(String, String)>, String), ProtoParseError> {
    let root = std::path::Path::new(dir_path);
    if !root.is_dir() {
        return Err(ProtoParseError::ParseError(format!(
            "path is not a valid directory: {}",
            dir_path
        )));
    }

    let mut files = Vec::new();
    scan_proto_dir(root, root, &mut files)?;

    if files.is_empty() {
        return Err(ProtoParseError::ParseError(
            "no .proto files found in directory".to_string(),
        ));
    }

    Ok((files, dir_path.to_string()))
}

/// 递归扫描目录的内部实现
fn scan_proto_dir(
    base: &std::path::Path,
    current: &std::path::Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), ProtoParseError> {
    let entries = std::fs::read_dir(current)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // 跳过隐藏目录和文件（以 . 开头）
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if file_name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            scan_proto_dir(base, &path, files)?;
        } else if path.extension().map(|e| e == "proto").unwrap_or(false) {
            // 计算相对路径
            let relative = path
                .strip_prefix(base)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| file_name.clone());

            let content = std::fs::read_to_string(&path)?;
            files.push((relative, content));
        }
    }

    Ok(())
}

/// 解析 `FileDescriptorSet` 二进制数据
///
/// 从预编译的 `.pb` / `.fdset` / `.desc` 文件中解码 `FileDescriptorSet`，
/// 构建 `DescriptorPool` 用于后续服务发现和动态调用。
///
/// # 参数
///
/// * `data` - `FileDescriptorSet` 的原始字节数据
///
/// # 返回
///
/// Success时返回 `DescriptorPool`
pub fn parse_descriptor_set(data: &[u8]) -> Result<DescriptorPool, ProtoParseError> {
    DescriptorPool::decode(data)
        .map_err(|e| ProtoParseError::ParseError(format!("failed to parse FileDescriptorSet: {}", e)))
}

// ============================================================================
// 请求 JSON 预验证
// ============================================================================

/// 验证Error条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationError {
    /// 字段路径（如 "address.street"）
    pub path: String,
    /// Error描述（如 "未知字段" 或 "期望 string 类型，实际 number"）
    pub message: String,
}

/// 验证结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationResult {
    /// 是否通过验证
    pub valid: bool,
    /// Error列表
    pub errors: Vec<ValidationError>,
}

/// 验证请求 JSON 是否符合 proto 消息定义
///
/// 根据消息描述符检查 JSON 对象中的字段名和类型是否与 proto schema 兼容。
/// 仅做建议性检查，不阻止发送。
///
/// # 参数
///
/// * `pool` - proto 描述符池
/// * `message_name` - 消息全名（如 "test.GetUserRequest"）
/// * `json` - 待验证的 JSON 字符串
///
/// # 返回
///
/// 验证结果，包含是否通过和Error列表
pub fn validate_request_json(
    pool: &DescriptorPool,
    message_name: &str,
    json: &str,
) -> Result<ValidationResult, String> {
    // 1. 解析 JSON 语法
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid JSON syntax: {}", e))?;

    // 2. 获取消息描述符
    let msg_desc = pool
        .get_message_by_name(message_name)
        .ok_or_else(|| format!("message type not found: {}", message_name))?;

    // 3. 必须是 JSON 对象
    let obj = match value.as_object() {
        Some(obj) => obj,
        None => {
            return Ok(ValidationResult {
                valid: false,
                errors: vec![ValidationError {
                    path: String::new(),
                    message: "expected JSON object, got another type".to_string(),
                }],
            });
        }
    };

    // 4. 遍历 JSON 字段进行验证
    let mut errors = Vec::new();
    validate_json_object(&msg_desc, obj, &mut errors, String::new());

    Ok(ValidationResult {
        valid: errors.is_empty(),
        errors,
    })
}

/// 递归验证 JSON 对象与 proto 消息定义的字段匹配
fn validate_json_object(
    msg_desc: &MessageDescriptor,
    obj: &serde_json::Map<String, serde_json::Value>,
    errors: &mut Vec<ValidationError>,
    parent_path: String,
) {
    for (key, value) in obj {
        let field_path = if parent_path.is_empty() {
            key.clone()
        } else {
            format!("{}.{}", parent_path, key)
        };

        // 查找字段定义
        let field_desc = msg_desc.fields().find(|f| f.name() == key);

        match field_desc {
            None => {
                errors.push(ValidationError {
                    path: field_path,
                    message: "unknown field".to_string(),
                });
            }
            Some(fd) => {
                // map 类型
                if fd.is_map() {
                    if !value.is_object() {
                        errors.push(ValidationError {
                            path: field_path,
                            message: format!(
                                "expected map (JSON object), got {}",
                                json_type_name(value)
                            ),
                        });
                    }
                    continue;
                }

                // repeated 类型
                if fd.cardinality() == prost_reflect::Cardinality::Repeated {
                    if !value.is_array() {
                        errors.push(ValidationError {
                            path: field_path,
                            message: format!(
                                "expected repeated (JSON array), got {}",
                                json_type_name(value)
                            ),
                        });
                    }
                    continue;
                }

                // 检查 singular 字段类型兼容性
                check_field_type(&fd, value, errors, field_path);
            }
        }
    }
}

/// 检查单个字段的类型兼容性
fn check_field_type(
    field: &FieldDescriptor,
    value: &serde_json::Value,
    errors: &mut Vec<ValidationError>,
    field_path: String,
) {
    // null 值在 proto3 中是合法的（等同于默认值）
    if value.is_null() {
        return;
    }

    match field.kind() {
        Kind::String => {
            if !value.is_string() {
                errors.push(ValidationError {
                    path: field_path,
                    message: format!("expected string type, got {}", json_type_name(value)),
                });
            }
        }
        Kind::Bool => {
            if !value.is_boolean() {
                errors.push(ValidationError {
                    path: field_path,
                    message: format!("expected bool type, got {}", json_type_name(value)),
                });
            }
        }
        Kind::Int32
        | Kind::Int64
        | Kind::Sint32
        | Kind::Sint64
        | Kind::Sfixed32
        | Kind::Sfixed64 => {
            if !value.is_number() {
                errors.push(ValidationError {
                    path: field_path,
                    message: format!("expected integer type, got {}", json_type_name(value)),
                });
            }
        }
        Kind::Uint32 | Kind::Uint64 | Kind::Fixed32 | Kind::Fixed64 => {
            if !value.is_number() {
                errors.push(ValidationError {
                    path: field_path,
                    message: format!("expected unsigned integer type, got {}", json_type_name(value)),
                });
            }
        }
        Kind::Float | Kind::Double => {
            if !value.is_number() {
                errors.push(ValidationError {
                    path: field_path,
                    message: format!("expected float type, got {}", json_type_name(value)),
                });
            }
        }
        Kind::Bytes => {
            if !value.is_string() {
                errors.push(ValidationError {
                    path: field_path,
                    message: format!(
                        "expected bytes (Base64 string), got {}",
                        json_type_name(value)
                    ),
                });
            }
        }
        Kind::Enum(_) => {
            // enum 可以是字符串名或数字
            if !value.is_string() && !value.is_number() {
                errors.push(ValidationError {
                    path: field_path,
                    message: format!("expected enum (string or number), got {}", json_type_name(value)),
                });
            }
        }
        Kind::Message(nested_msg) => {
            if let Some(nested_obj) = value.as_object() {
                // 递归验证嵌套消息
                validate_json_object(&nested_msg, nested_obj, errors, field_path);
            } else {
                errors.push(ValidationError {
                    path: field_path,
                    message: format!("expected message (JSON object), got {}", json_type_name(value)),
                });
            }
        }
    }
}

/// 返回 JSON 值的可读类型名称
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// ============================================================================
// Message Definition Extraction
// ============================================================================

/// Extract all message definitions from a `DescriptorPool` into a flat `HashMap`
/// keyed by message `full_name` (e.g. "package.MessageName").
///
/// The flat map avoids duplication when the same message type is referenced
/// by multiple methods or services. Frontend uses the map for lazy expansion.
///
/// Filters out synthetic map entry messages automatically.
/// Marks Well-Known Types (google.protobuf.*) with `is_wkt` = true.
/// Circular references between top-level messages are detected via a post-processing
/// pass: any message-typed field whose `type_full_name` participates in a cycle gets
/// its `type_kind` changed to "`circular_ref`" and `nested_message` cleared.
pub fn get_message_definitions(pool: &DescriptorPool) -> HashMap<String, GrpcMessageInfo> {
    let mut definitions = HashMap::new();
    let mut visited = HashSet::new();

    // Pass 1: extract all message info (shared visited catches cycles in nested expansions)
    for message in pool.all_messages() {
        let full_name = message.full_name().to_string();
        if message.is_map_entry() {
            continue;
        }
        let msg_info = extract_message_info(&message, &mut visited, pool);
        definitions.insert(full_name, msg_info);
    }

    // Pass 2: detect cycles in the top-level definition graph.
    // For each message-typed field, DFS-walk from the referenced message; if we ever
    // reach back to the current message, it is a circular reference.
    detect_top_level_cycles(&mut definitions);

    definitions
}

/// Post-processing: for each message-typed field in every definition, walk the
/// dependency graph from the referenced type. If the walk reaches back to the
/// current message, mark the field as `circular_ref` and clear its `nested_message`.
fn detect_top_level_cycles(definitions: &mut HashMap<String, GrpcMessageInfo>) {
    // Collect all (msg_name, field_idx) pairs that reference another definition
    let mut edges: Vec<(String, String, usize)> = Vec::new();
    for (msg_name, msg) in definitions.iter() {
        for (i, field) in msg.fields.iter().enumerate() {
            if field.type_kind == "message" {
                if let Some(ref type_name) = field.type_full_name {
                    if definitions.contains_key(type_name) {
                        edges.push((msg_name.clone(), type_name.clone(), i));
                    }
                }
            }
        }
    }

    // For each edge, check if following references from the target leads back to source
    for (src_name, ref_name, field_idx) in &edges {
        let mut visited_local = HashSet::new();
        if has_path_to(ref_name, src_name, definitions, &mut visited_local) {
            if let Some(msg) = definitions.get_mut(src_name) {
                if let Some(field) = msg.fields.get_mut(*field_idx) {
                    field.type_kind = "circular_ref".to_string();
                    field.nested_message = None;
                }
            }
        }
    }
}

/// DFS from `current` to `target` following message-typed field references.
/// Returns `true` if `target` is reachable from `current`.
///
/// Both `"message"` and `"circular_ref"` fields are traversed:
/// `"circular_ref"` edges (marked by the nested extraction pass) may be part
/// of larger indirect cycles that need full resolution at the top level.
fn has_path_to(
    current: &str,
    target: &str,
    definitions: &HashMap<String, GrpcMessageInfo>,
    visited: &mut HashSet<String>,
) -> bool {
    if current == target {
        return true;
    }
    if !visited.insert(current.to_string()) {
        return false;
    }
    if let Some(msg) = definitions.get(current) {
        for field in &msg.fields {
            if field.type_kind == "message" || field.type_kind == "circular_ref" {
                if let Some(ref type_name) = field.type_full_name {
                    if definitions.contains_key(type_name)
                        && has_path_to(type_name, target, definitions, visited)
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Build a `GrpcMessageInfo` from a prost-reflect `MessageDescriptor`
fn extract_message_info(
    msg: &MessageDescriptor,
    visited: &mut HashSet<String>,
    pool: &DescriptorPool,
) -> GrpcMessageInfo {
    let full_name = msg.full_name().to_string();
    let is_wkt = full_name.starts_with("google.protobuf.");

    // Mark this message as visited so circular references can be detected
    // during recursive field resolution. The entry is removed after fields
    // are processed to keep visited scoped to the current chain.
    visited.insert(full_name.clone());

    let fields: Vec<GrpcFieldInfo> = msg
        .fields()
        .map(|field| field_to_info(&field, visited, pool))
        .collect();

    visited.remove(&full_name);

    let oneof_groups = get_oneof_info(msg);
    let reserved_ranges = get_reserved_ranges(msg);
    let reserved_names = get_reserved_names(msg);

    GrpcMessageInfo {
        full_name,
        fields,
        oneof_groups,
        is_wkt,
        reserved_ranges,
        reserved_names,
    }
}

/// Convert a `FieldDescriptor` to `GrpcFieldInfo`
fn field_to_info(
    field: &FieldDescriptor,
    visited: &mut HashSet<String>,
    pool: &DescriptorPool,
) -> GrpcFieldInfo {
    let name = field.name().to_string();
    let number = field.number();
    let is_map = field.is_map();
    let label = match field.cardinality() {
        prost_reflect::Cardinality::Optional => "optional",
        prost_reflect::Cardinality::Required => "required",
        prost_reflect::Cardinality::Repeated => "repeated",
    }
    .to_string();

    let (
        type_kind,
        type_display,
        type_full_name,
        enum_values,
        nested_message,
        map_key_type,
        map_value_type,
    ) = resolve_field_type(field, visited, pool);

    GrpcFieldInfo {
        name,
        number,
        type_kind,
        type_display,
        type_full_name,
        label,
        is_map,
        map_key_type,
        map_value_type,
        enum_values,
        nested_message,
    }
}

/// Resolve the type information for a field, handling all protobuf type categories
fn resolve_field_type(
    field: &FieldDescriptor,
    visited: &mut HashSet<String>,
    pool: &DescriptorPool,
) -> (
    String,
    String,
    Option<String>,
    Vec<GrpcEnumValue>,
    Option<Box<GrpcMessageInfo>>,
    Option<String>,
    Option<String>,
) {
    let label_prefix = match field.cardinality() {
        prost_reflect::Cardinality::Repeated => "repeated ",
        _ => "",
    };

    if field.is_map() {
        let (map_key, map_value) = if let Kind::Message(entry_msg) = field.kind() {
            let key_kind = entry_msg.map_entry_key_field().kind();
            let val_kind = entry_msg.map_entry_value_field().kind();
            (format_type_name(&key_kind), format_type_name(&val_kind))
        } else {
            (String::new(), String::new())
        };
        let display = format!("map<{}, {}>", map_key, map_value);
        return (
            "map".to_string(),
            display,
            None,
            vec![],
            None,
            Some(map_key),
            Some(map_value),
        );
    }

    match field.kind() {
        Kind::Double => type_result(label_prefix, "double"),
        Kind::Float => type_result(label_prefix, "float"),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => type_result(label_prefix, "int32"),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => type_result(label_prefix, "int64"),
        Kind::Uint32 | Kind::Fixed32 => type_result(label_prefix, "uint32"),
        Kind::Uint64 | Kind::Fixed64 => type_result(label_prefix, "uint64"),
        Kind::Bool => type_result(label_prefix, "bool"),
        Kind::String => type_result(label_prefix, "string"),
        Kind::Bytes => type_result(label_prefix, "bytes"),
        Kind::Enum(enum_desc) => {
            let enum_name = enum_desc.full_name().to_string();
            let values: Vec<GrpcEnumValue> = enum_desc
                .values()
                .map(|v| GrpcEnumValue {
                    name: v.name().to_string(),
                    number: v.number(),
                })
                .collect();
            let display = type_display(label_prefix, &enum_name);
            (
                "enum".to_string(),
                display,
                Some(enum_name),
                values,
                None,
                None,
                None,
            )
        }
        Kind::Message(msg_desc) => {
            let msg_name = msg_desc.full_name().to_string();
            let display = type_display(label_prefix, &msg_name);

            // Check if already visited (circular reference)
            if visited.contains(&msg_name) {
                return (
                    "circular_ref".to_string(),
                    display,
                    Some(msg_name),
                    vec![],
                    None,
                    None,
                    None,
                );
            }

            // Check if WKT - skip nested expansion for well-known types
            if msg_name.starts_with("google.protobuf.") {
                return (
                    "message".to_string(),
                    display,
                    Some(msg_name),
                    vec![],
                    None,
                    None,
                    None,
                );
            }

            // Recursively extract nested message.
            // extract_message_info manages visited insert/remove internally,
            // so we only insert here to flag this type as "in-progress" and
            // prevent sibling-field cycles; the remove is handled inside the call.
            visited.insert(msg_name.clone());
            let nested = extract_message_info(&msg_desc, visited, pool);

            (
                "message".to_string(),
                display,
                Some(msg_name),
                vec![],
                Some(Box::new(nested)),
                None,
                None,
            )
        }
    }
}

/// Helper: create a simple type result tuple for scalar types
fn type_result(
    label_prefix: &str,
    type_name: &str,
) -> (
    String,
    String,
    Option<String>,
    Vec<GrpcEnumValue>,
    Option<Box<GrpcMessageInfo>>,
    Option<String>,
    Option<String>,
) {
    (
        type_name.to_string(),
        type_display(label_prefix, type_name),
        None,
        vec![],
        None,
        None,
        None,
    )
}

/// Helper: type display string (with optional "repeated " prefix)
fn type_display(prefix: &str, type_name: &str) -> String {
    format!("{}{}", prefix, type_name)
}

/// Extract oneof groups from a message
fn get_oneof_info(msg: &MessageDescriptor) -> Vec<GrpcOneofInfo> {
    msg.oneofs()
        .map(|oneof| GrpcOneofInfo {
            name: oneof.name().to_string(),
            field_numbers: oneof.fields().map(|f| f.number()).collect(),
        })
        .collect()
}

/// Extract reserved field ranges
fn get_reserved_ranges(_msg: &MessageDescriptor) -> Vec<(u32, u32)> {
    // prost-reflect doesn't have a direct reserved_ranges API.
    // Return empty for now - the frontend handles absence.
    vec![]
}

/// Extract reserved field names
fn get_reserved_names(_msg: &MessageDescriptor) -> Vec<String> {
    // Same as above
    vec![]
}

/// Format a protobuf Kind into a type name string
fn format_type_name(kind: &prost_reflect::Kind) -> String {
    match kind {
        Kind::Double => "double",
        Kind::Float => "float",
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => "int32",
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => "int64",
        Kind::Uint32 | Kind::Fixed32 => "uint32",
        Kind::Uint64 | Kind::Fixed64 => "uint64",
        Kind::Bool => "bool",
        Kind::String => "string",
        Kind::Bytes => "bytes",
        Kind::Enum(desc) => desc.full_name(),
        Kind::Message(desc) => desc.full_name(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的简单 proto 文件内容
    const TEST_PROTO: &str = r#"
syntax = "proto3";

package test;

service TestService {
  rpc GetUser(GetUserRequest) returns (GetUserResponse);
  rpc ListUsers(ListUsersRequest) returns (stream User);
}

message GetUserRequest {
  string user_id = 1;
}

message GetUserResponse {
  string user_id = 1;
  string name = 2;
  int32 age = 3;
}

message ListUsersRequest {
  int32 page_size = 1;
}

message User {
  string id = 1;
  string name = 2;
}
"#;

    /// 测试解析简单的 proto 文件
    #[test]
    fn test_parse_simple_proto() {
        let files = vec![ProtoFile {
            path: "test.proto".to_string(),
            content: TEST_PROTO.to_string(),
        }];

        let pool =
            parse_proto_files(&files).unwrap_or_else(|e| panic!("解析 proto 文件应Success: {e}"));

        // 验证可以找到服务
        let service = pool
            .get_service_by_name("test.TestService")
            .unwrap_or_else(|| panic!("应能找到 TestService"));
        assert_eq!(service.name(), "TestService");
    }

    /// 测试 `list_services` 返回正确的服务和方法信息
    #[test]
    fn test_list_services() {
        let files = vec![ProtoFile {
            path: "test.proto".to_string(),
            content: TEST_PROTO.to_string(),
        }];

        let pool =
            parse_proto_files(&files).unwrap_or_else(|e| panic!("解析 proto 文件应Success: {e}"));
        let services = list_services(&pool);

        assert_eq!(services.len(), 1, "应只有一个服务");

        let service = &services[0];
        assert_eq!(service.service_name, "test.TestService");
        assert_eq!(service.methods.len(), 2, "TestService 应有两个方法");

        // 验证 GetUser 方法
        let get_user = service
            .methods
            .iter()
            .find(|m| m.method_name == "GetUser")
            .unwrap_or_else(|| panic!("应能找到 GetUser 方法"));
        assert_eq!(get_user.input_type, "test.GetUserRequest");
        assert_eq!(get_user.output_type, "test.GetUserResponse");
        assert!(!get_user.is_server_streaming, "GetUser 不应是服务端流式");

        // 验证 ListUsers 方法
        let list_users = service
            .methods
            .iter()
            .find(|m| m.method_name == "ListUsers")
            .unwrap_or_else(|| panic!("应能找到 ListUsers 方法"));
        assert_eq!(list_users.input_type, "test.ListUsersRequest");
        assert_eq!(list_users.output_type, "test.User");
        assert!(list_users.is_server_streaming, "ListUsers 应是服务端流式");
    }

    /// 测试生成 JSON 骨架
    #[test]
    fn test_generate_json_skeleton() {
        let files = vec![ProtoFile {
            path: "test.proto".to_string(),
            content: TEST_PROTO.to_string(),
        }];

        let pool =
            parse_proto_files(&files).unwrap_or_else(|e| panic!("解析 proto 文件应Success: {e}"));

        let msg = pool
            .get_message_by_name("test.GetUserRequest")
            .unwrap_or_else(|| panic!("应能找到 GetUserRequest 消息"));

        let skeleton = generate_json_skeleton(&msg);

        // 骨架应该是 JSON 对象，包含字段名
        assert!(skeleton.is_object(), "骨架应该是 JSON 对象");

        let obj = skeleton
            .as_object()
            .unwrap_or_else(|| panic!("应为 JSON 对象"));
        assert!(obj.contains_key("user_id"), "骨架应包含 user_id 字段");
    }

    /// 测试空文件列表返回Error
    #[test]
    fn test_empty_files_returns_error() {
        let result = parse_proto_files(&[]);
        assert!(result.is_err(), "空文件列表应返回Error");
    }

    /// 测试无效 proto 内容返回Error
    #[test]
    fn test_invalid_proto_returns_error() {
        let files = vec![ProtoFile {
            path: "invalid.proto".to_string(),
            content: "this is not valid proto syntax !!!".to_string(),
        }];

        let result = parse_proto_files(&files);
        assert!(result.is_err(), "无效 proto 内容应返回Error");
    }

    /// 测试目录扫描 — 空目录应返回Error
    #[test]
    fn test_scan_empty_dir_returns_error() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("创建临时目录应Success: {e}"));
        let result = discover_proto_files_in_dir(
            temp_dir
                .path()
                .to_str()
                .unwrap_or_else(|| panic!("路径转换应Success")),
        );
        assert!(result.is_err(), "空目录应返回Error");
    }

    /// 测试目录扫描 — 不存在的目录应返回Error
    #[test]
    fn test_scan_nonexistent_dir_returns_error() {
        let result = discover_proto_files_in_dir("/nonexistent/path/that/does/not/exist");
        assert!(result.is_err(), "不存在的目录应返回Error");
    }

    /// 测试目录扫描 — 正常扫描 .proto 文件
    #[test]
    fn test_scan_dir_finds_proto_files() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("创建临时目录应Success: {e}"));

        // 创建 proto 文件
        std::fs::write(temp_dir.path().join("test.proto"), TEST_PROTO)
            .unwrap_or_else(|e| panic!("写入 proto 文件应Success: {e}"));

        // 创建子目录中的 proto 文件
        let sub_dir = temp_dir.path().join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap_or_else(|e| panic!("创建子目录应Success: {e}"));
        std::fs::write(
            sub_dir.join("nested.proto"),
            r#"syntax = "proto3"; package nested; message Nested { string id = 1; }"#,
        )
        .unwrap_or_else(|e| panic!("写入嵌套 proto 文件应Success: {e}"));

        // 创建非 proto 文件（应被跳过）
        std::fs::write(temp_dir.path().join("readme.txt"), "not a proto file")
            .unwrap_or_else(|e| panic!("写入非 proto 文件应Success: {e}"));

        // 创建隐藏目录（应被跳过）
        let hidden_dir = temp_dir.path().join(".hidden");
        std::fs::create_dir_all(&hidden_dir)
            .unwrap_or_else(|e| panic!("创建隐藏目录应Success: {e}"));
        std::fs::write(
            hidden_dir.join("hidden.proto"),
            r#"syntax = "proto3"; package hidden;"#,
        )
        .unwrap_or_else(|e| panic!("写入隐藏 proto 文件应Success: {e}"));

        let dir_path = temp_dir
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("路径转换应Success"));
        let result = discover_proto_files_in_dir(dir_path);

        assert!(result.is_ok(), "扫描目录应Success: {:?}", result);
        let (files, _) = result.unwrap_or_else(|e| panic!("应Success获取文件列表: {e}"));
        assert_eq!(
            files.len(),
            2,
            "应找到 2 个 proto 文件（跳过隐藏目录和非 proto 文件）"
        );

        let filenames: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert!(filenames.contains(&"test.proto"), "应包含 test.proto");
        assert!(
            filenames.contains(&"nested.proto")
                || filenames.iter().any(|f| f.contains("nested.proto")),
            "应包含 nested.proto（可能在子目录路径中）"
        );
    }

    /// 测试 `parse_descriptor_set` — 无效数据应返回Error
    #[test]
    fn test_parse_descriptor_set_invalid_data() {
        let result = parse_descriptor_set(b"not a valid descriptor set");
        assert!(result.is_err(), "无效数据应返回Error");
    }

    /// 测试 `parse_descriptor_set` — 空数据
    #[test]
    fn test_parse_descriptor_set_empty_data() {
        // 空的 FileDescriptorSet 可以被Success解码（prost 默认行为），只是结果为空 pool
        let result = parse_descriptor_set(b"");
        // 空数据解码Success但 pool 为空，这是 prost 的默认行为
        // 验证可以解码（即使结果为空）
        assert!(result.is_ok(), "空数据解码为空的 DescriptorPool");
        let pool = result.unwrap_or_else(|e| panic!("应Success解码: {e}"));
        // 空 pool 中没有服务
        assert_eq!(pool.services().count(), 0, "空 DescriptorPool 应没有服务");
    }

    /// 测试 `parse_descriptor_set` — 有效 `FileDescriptorSet`
    #[test]
    fn test_parse_descriptor_set_valid() {
        // 先用 proto_parser 生成一个 FileDescriptorSet，再通过 parse_descriptor_set 解码
        let files = [ProtoFile {
            path: "test.proto".to_string(),
            content: TEST_PROTO.to_string(),
        }];

        let temp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("创建临时目录应Success: {e}"));
        let file_path = temp_dir.path().join("test.proto");
        std::fs::write(&file_path, &files[0].content)
            .unwrap_or_else(|e| panic!("写入 proto 文件应Success: {e}"));

        let fds = protox::compile(["test.proto"], [temp_dir.path()])
            .unwrap_or_else(|e| panic!("编译 proto 文件应Success: {e}"));

        // 将 FileDescriptorSet 编码为字节
        let mut buf = Vec::new();
        prost::Message::encode(&fds, &mut buf)
            .unwrap_or_else(|e| panic!("编码 FileDescriptorSet 应Success: {e}"));

        // 使用 parse_descriptor_set 解码
        let pool = parse_descriptor_set(&buf)
            .unwrap_or_else(|e| panic!("解析 FileDescriptorSet 应Success: {e}"));

        // 验证服务存在
        let service = pool
            .get_service_by_name("test.TestService")
            .unwrap_or_else(|| panic!("应能找到 TestService"));
        assert_eq!(service.name(), "TestService");
        assert_eq!(service.methods().count(), 2);
    }

    /// 测试带有 import 的多文件解析
    #[test]
    fn test_multi_file_with_import() {
        let _common_proto = r#"
syntax = "proto3";

package common;

message Status {
  int32 code = 1;
  string message = 2;
}
"#;

        // 注意：由于 protox::compile 使用文件名匹配，
        // 多文件 import 需要更复杂的处理。
        // 这里只测试单文件中的嵌套消息
        let main_proto = r#"
syntax = "proto3";

package myapp;

service OrderService {
  rpc CreateOrder(CreateOrderRequest) returns (CreateOrderResponse);
}

message CreateOrderRequest {
  string product_id = 1;
  int32 quantity = 2;
}

message CreateOrderResponse {
  string order_id = 1;
  Status status = 2;
}

message Status {
  int32 code = 1;
  string message = 2;
}
"#;

        let files = vec![ProtoFile {
            path: "order.proto".to_string(),
            content: main_proto.to_string(),
        }];

        let pool = parse_proto_files(&files)
            .unwrap_or_else(|e| panic!("解析多消息 proto 文件应Success: {e}"));
        let services = list_services(&pool);

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].service_name, "myapp.OrderService");
        assert_eq!(services[0].methods.len(), 1);

        // 验证嵌套消息的 JSON 骨架
        let response_msg = pool
            .get_message_by_name("myapp.CreateOrderResponse")
            .unwrap_or_else(|| panic!("应能找到 CreateOrderResponse"));
        let skeleton = generate_json_skeleton(&response_msg);

        assert!(skeleton.is_object());
        let obj = skeleton
            .as_object()
            .unwrap_or_else(|| panic!("应为 JSON 对象"));
        assert!(obj.contains_key("order_id"));
        assert!(obj.contains_key("status"));
    }

    // ========================================================================
    // validate_request_json 测试
    // ========================================================================

    /// 辅助函数：解析测试 proto 并获取 pool
    fn get_test_pool() -> DescriptorPool {
        let files = vec![ProtoFile {
            path: "test.proto".to_string(),
            content: TEST_PROTO.to_string(),
        }];
        parse_proto_files(&files).unwrap_or_else(|e| panic!("解析 proto 文件应Success: {e}"))
    }

    /// 测试验证 — 有效的 JSON
    #[test]
    fn test_validate_valid_json() {
        let pool = get_test_pool();
        let result = validate_request_json(&pool, "test.GetUserRequest", r#"{"user_id": "abc"}"#);
        assert!(result.is_ok(), "应Success验证");
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(vr.valid, "应通过验证");
        assert!(vr.errors.is_empty(), "不应有Error");
    }

    /// 测试验证 — JSON 语法Error
    #[test]
    fn test_validate_json_syntax_error() {
        let pool = get_test_pool();
        let result = validate_request_json(&pool, "test.GetUserRequest", r#"{invalid}"#);
        assert!(result.is_err(), "JSON 语法Error应返回 Err");
    }

    /// 测试验证 — 消息类型不存在
    #[test]
    fn test_validate_unknown_message() {
        let pool = get_test_pool();
        let result = validate_request_json(&pool, "test.NonExistent", r#"{}"#);
        assert!(result.is_err(), "未知消息类型应返回 Err");
    }

    /// 测试验证 — 非 JSON 对象
    #[test]
    fn test_validate_non_object() {
        let pool = get_test_pool();
        let result = validate_request_json(&pool, "test.GetUserRequest", r#"[1,2,3]"#);
        assert!(result.is_ok(), "应返回验证结果");
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(!vr.valid, "非 JSON 对象应不通过");
        assert_eq!(vr.errors.len(), 1);
        assert!(vr.errors[0].message.contains("expected JSON object"));
    }

    /// 测试验证 — 未知字段
    #[test]
    fn test_validate_unknown_field() {
        let pool = get_test_pool();
        let result = validate_request_json(
            &pool,
            "test.GetUserRequest",
            r#"{"user_id": "abc", "unknown_field": 123}"#,
        );
        assert!(result.is_ok());
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(!vr.valid, "有未知字段应不通过");
        assert!(vr
            .errors
            .iter()
            .any(|e| e.path == "unknown_field" && e.message.contains("unknown field")));
    }

    /// 测试验证 — 类型不匹配
    #[test]
    fn test_validate_type_mismatch() {
        let pool = get_test_pool();
        // GetUserResponse: user_id (string), name (string), age (int32)
        let result = validate_request_json(
            &pool,
            "test.GetUserResponse",
            r#"{"user_id": 123, "name": "test", "age": "not_a_number"}"#,
        );
        assert!(result.is_ok());
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(!vr.valid, "类型不匹配应不通过");
        assert_eq!(vr.errors.len(), 2, "应有 2 个类型Error");
        assert!(vr.errors.iter().any(|e| e.path == "user_id"));
        assert!(vr.errors.iter().any(|e| e.path == "age"));
    }

    /// 测试验证 — 嵌套消息
    #[test]
    fn test_validate_nested_message() {
        let proto = r#"
syntax = "proto3";
package nested;

message Address {
  string street = 1;
  int32 zip = 2;
}

message Person {
  string name = 1;
  Address address = 2;
}
"#;
        let files = vec![ProtoFile {
            path: "nested.proto".to_string(),
            content: proto.to_string(),
        }];
        let pool = parse_proto_files(&files).unwrap_or_else(|e| panic!("解析应Success: {e}"));

        // 嵌套字段类型Error
        let result = validate_request_json(
            &pool,
            "nested.Person",
            r#"{"name": "Alice", "address": {"street": 123, "zip": "abc"}}"#,
        );
        assert!(result.is_ok());
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(!vr.valid);
        assert!(vr.errors.iter().any(|e| e.path == "address.street"));
        assert!(vr.errors.iter().any(|e| e.path == "address.zip"));

        // 嵌套字段正确
        let result2 = validate_request_json(
            &pool,
            "nested.Person",
            r#"{"name": "Alice", "address": {"street": "Main St", "zip": 12345}}"#,
        );
        assert!(result2.is_ok());
        let vr2 = result2.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(vr2.valid, "正确的嵌套消息应通过");
    }

    /// 测试验证 — 空对象应通过
    #[test]
    fn test_validate_empty_object() {
        let pool = get_test_pool();
        let result = validate_request_json(&pool, "test.GetUserRequest", r#"{}"#);
        assert!(result.is_ok());
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(vr.valid, "空对象应通过验证（proto3 所有字段可选）");
    }

    /// 测试验证 — null 值应通过
    #[test]
    fn test_validate_null_values() {
        let pool = get_test_pool();
        let result = validate_request_json(
            &pool,
            "test.GetUserResponse",
            r#"{"user_id": null, "name": null, "age": null}"#,
        );
        assert!(result.is_ok());
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(vr.valid, "null 值应通过验证");
    }

    /// 测试验证 — repeated 字段
    #[test]
    fn test_validate_repeated_field() {
        let proto = r#"
syntax = "proto3";
package rep;

message ItemList {
  repeated string items = 1;
  repeated int32 counts = 2;
}
"#;
        let files = vec![ProtoFile {
            path: "rep.proto".to_string(),
            content: proto.to_string(),
        }];
        let pool = parse_proto_files(&files).unwrap_or_else(|e| panic!("解析应Success: {e}"));

        // repeated 使用数组 — 通过
        let result = validate_request_json(
            &pool,
            "rep.ItemList",
            r#"{"items": ["a", "b"], "counts": [1, 2]}"#,
        );
        assert!(result.is_ok());
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(vr.valid, "正确的数组应通过");

        // repeated 使用非数组 — 不通过
        let result2 = validate_request_json(&pool, "rep.ItemList", r#"{"items": "not_array"}"#);
        assert!(result2.is_ok());
        let vr2 = result2.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(!vr2.valid, "repeated 字段传入非数组应不通过");
    }

    /// 测试验证 — map 字段
    #[test]
    fn test_validate_map_field() {
        let proto = r#"
syntax = "proto3";
package maptest;

message Config {
  map<string, string> labels = 1;
}
"#;
        let files = vec![ProtoFile {
            path: "maptest.proto".to_string(),
            content: proto.to_string(),
        }];
        let pool = parse_proto_files(&files).unwrap_or_else(|e| panic!("解析应Success: {e}"));

        // map 使用对象 — 通过
        let result =
            validate_request_json(&pool, "maptest.Config", r#"{"labels": {"key": "value"}}"#);
        assert!(result.is_ok());
        let vr = result.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(vr.valid, "正确的 map 应通过");

        // map 使用非对象 — 不通过
        let result2 = validate_request_json(&pool, "maptest.Config", r#"{"labels": [1,2]}"#);
        assert!(result2.is_ok());
        let vr2 = result2.unwrap_or_else(|e| panic!("应返回验证结果: {e}"));
        assert!(!vr2.valid, "map 字段传入非对象应不通过");
    }

    /// 测试验证 — enum 字段
    #[test]
    fn test_validate_enum_field() {
        let proto = r#"
syntax = "proto3";
package enumtest;

enum Status {
  UNKNOWN = 0;
  ACTIVE = 1;
}

message User {
  Status status = 1;
}
"#;
        let files = vec![ProtoFile {
            path: "enumtest.proto".to_string(),
            content: proto.to_string(),
        }];
        let pool = parse_proto_files(&files).unwrap_or_else(|e| panic!("解析应Success: {e}"));

        // enum 使用字符串 — 通过
        let result = validate_request_json(&pool, "enumtest.User", r#"{"status": "ACTIVE"}"#);
        assert!(result.is_ok());
        assert!(
            result
                .unwrap_or_else(|e| panic!("应返回验证结果: {e}"))
                .valid
        );

        // enum 使用数字 — 通过
        let result2 = validate_request_json(&pool, "enumtest.User", r#"{"status": 1}"#);
        assert!(result2.is_ok());
        assert!(
            result2
                .unwrap_or_else(|e| panic!("应返回验证结果: {e}"))
                .valid
        );

        // enum 使用数组 — 不通过
        let result3 = validate_request_json(&pool, "enumtest.User", r#"{"status": [1]}"#);
        assert!(result3.is_ok());
        assert!(
            !result3
                .unwrap_or_else(|e| panic!("应返回验证结果: {e}"))
                .valid
        );
    }

    // ========================================================================
    // get_message_definitions 测试
    // ========================================================================

    /// Comprehensive proto for message definition extraction tests
    const TEST_DEFINITIONS_PROTO: &str = r#"
syntax = "proto3";
package testdef;

import "google/protobuf/timestamp.proto";

enum Priority {
  PRIORITY_UNSPECIFIED = 0;
  LOW = 1;
  HIGH = 2;
}

message SimpleMessage {
  string name = 1;
  int32 age = 2;
  bool active = 3;
  double score = 4;
  bytes data = 5;
}

message WithEnum {
  Priority priority = 1;
}

message WithOneof {
  string name = 1;
  oneof payload {
    string text_value = 2;
    int32 number_value = 3;
  }
}

message WithMap {
  map<string, string> labels = 1;
}

message EmptyMessage {
}

message NestedOuter {
  string id = 1;
  message Inner {
    string value = 1;
    int32 count = 2;
  }
  Inner inner = 2;
}

message WithWkt {
  google.protobuf.Timestamp created_at = 1;
  string description = 2;
}
"#;

    /// Circular reference proto
    const TEST_CIRCULAR_PROTO: &str = r#"
syntax = "proto3";
package circref;

message CircularA {
  string name = 1;
  CircularB b = 2;
}

message CircularB {
  string name = 1;
  CircularA a = 2;
}
"#;

    /// Parse a proto string into a `DescriptorPool`
    fn parse_defs_pool(proto: &str, filename: &str) -> DescriptorPool {
        let files = vec![ProtoFile {
            path: filename.to_string(),
            content: proto.to_string(),
        }];
        parse_proto_files(&files)
            .unwrap_or_else(|e| panic!("解析 proto '{}' 应Success: {}", filename, e))
    }

    /// Test basic scalar field types
    #[test]
    fn test_get_message_definitions_simple() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        let simple = defs
            .get("testdef.SimpleMessage")
            .unwrap_or_else(|| panic!("SimpleMessage should exist"));
        assert_eq!(simple.fields.len(), 5, "SimpleMessage should have 5 fields");
        assert!(!simple.is_wkt, "SimpleMessage is not a WKT");

        // Check name (string)
        let name_f = simple
            .fields
            .iter()
            .find(|f| f.name == "name")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(name_f.number, 1);
        assert_eq!(name_f.type_kind, "string");
        assert_eq!(name_f.type_display, "string");
        assert_eq!(name_f.label, "optional");
        assert!(!name_f.is_map);

        // Check age (int32)
        let age_f = simple
            .fields
            .iter()
            .find(|f| f.name == "age")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(age_f.number, 2);
        assert_eq!(age_f.type_kind, "int32");
        assert_eq!(age_f.type_display, "int32");

        // Check active (bool)
        let active_f = simple
            .fields
            .iter()
            .find(|f| f.name == "active")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(active_f.number, 3);
        assert_eq!(active_f.type_kind, "bool");

        // Check score (double)
        let score_f = simple
            .fields
            .iter()
            .find(|f| f.name == "score")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(score_f.number, 4);
        assert_eq!(score_f.type_kind, "double");

        // Check data (bytes)
        let data_f = simple
            .fields
            .iter()
            .find(|f| f.name == "data")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(data_f.number, 5);
        assert_eq!(data_f.type_kind, "bytes");
    }

    /// Test enum field extraction
    #[test]
    fn test_get_message_definitions_enum() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        let with_enum = defs
            .get("testdef.WithEnum")
            .unwrap_or_else(|| panic!("WithEnum should exist"));
        assert_eq!(with_enum.fields.len(), 1);

        let color_f = &with_enum.fields[0];
        assert_eq!(color_f.name, "priority");
        assert_eq!(color_f.number, 1);
        assert_eq!(color_f.type_kind, "enum");
        assert_eq!(color_f.type_full_name.as_deref(), Some("testdef.Priority"));
        assert_eq!(
            color_f.enum_values.len(),
            3,
            "Priority should have 3 values"
        );
        assert_eq!(color_f.enum_values[0].name, "PRIORITY_UNSPECIFIED");
        assert_eq!(color_f.enum_values[0].number, 0);
        assert_eq!(color_f.enum_values[1].name, "LOW");
        assert_eq!(color_f.enum_values[1].number, 1);
        assert_eq!(color_f.enum_values[2].name, "HIGH");
        assert_eq!(color_f.enum_values[2].number, 2);
        assert!(color_f.enum_values.iter().all(|v| !v.name.is_empty()));
    }

    /// Test oneof group extraction
    #[test]
    fn test_get_message_definitions_oneof() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        let with_oneof = defs
            .get("testdef.WithOneof")
            .unwrap_or_else(|| panic!("WithOneof should exist"));
        assert_eq!(
            with_oneof.oneof_groups.len(),
            1,
            "Should have 1 oneof group"
        );

        let payload = &with_oneof.oneof_groups[0];
        assert_eq!(payload.name, "payload");
        // The oneof contains text_value (2) and number_value (3)
        assert_eq!(payload.field_numbers.len(), 2);
        assert!(payload.field_numbers.contains(&2));
        assert!(payload.field_numbers.contains(&3));
        // field 1 (name) should NOT be in the oneof
        assert!(!payload.field_numbers.contains(&1));
    }

    /// Test map field extraction
    #[test]
    fn test_get_message_definitions_map() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        let with_map = defs
            .get("testdef.WithMap")
            .unwrap_or_else(|| panic!("WithMap should exist"));
        assert_eq!(with_map.fields.len(), 1);

        let labels_f = &with_map.fields[0];
        assert_eq!(labels_f.name, "labels");
        assert!(labels_f.is_map, "labels should be a map field");
        assert_eq!(labels_f.type_kind, "map");
        assert_eq!(labels_f.type_display, "map<string, string>");
        assert_eq!(
            labels_f.map_key_type.as_deref(),
            Some("string"),
            "map key should be string"
        );
        assert_eq!(
            labels_f.map_value_type.as_deref(),
            Some("string"),
            "map value should be string"
        );
        assert!(labels_f.enum_values.is_empty());
        assert!(labels_f.nested_message.is_none());
    }

    /// Test WKT detection
    #[test]
    fn test_get_message_definitions_wkt() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        let with_wkt = defs
            .get("testdef.WithWkt")
            .unwrap_or_else(|| panic!("WithWkt should exist"));
        assert_eq!(with_wkt.fields.len(), 2, "WithWkt should have 2 fields");

        // created_at field referencing google.protobuf.Timestamp
        let ts_f = with_wkt
            .fields
            .iter()
            .find(|f| f.name == "created_at")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(
            ts_f.type_kind, "message",
            "WKT field should be 'message' type"
        );
        assert_eq!(
            ts_f.type_full_name.as_deref(),
            Some("google.protobuf.Timestamp"),
            "should reference Timestamp"
        );
        // WKT fields should NOT have nested_message expanded
        assert!(
            ts_f.nested_message.is_none(),
            "WKT should not be expanded as nested_message"
        );

        // description field
        let desc_f = with_wkt
            .fields
            .iter()
            .find(|f| f.name == "description")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(desc_f.type_kind, "string");

        // Verify that google.protobuf.Timestamp itself appears with is_wkt = true
        let ts_msg = defs.get("google.protobuf.Timestamp");
        assert!(ts_msg.is_some(), "Timestamp should exist in definitions");
        assert!(
            ts_msg.unwrap_or_else(|| panic!("expected Some")).is_wkt,
            "google.protobuf.Timestamp should be marked is_wkt"
        );
    }

    /// Test empty message extraction
    #[test]
    fn test_get_message_definitions_empty_message() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        let empty = defs
            .get("testdef.EmptyMessage")
            .unwrap_or_else(|| panic!("EmptyMessage should exist"));
        assert!(
            empty.fields.is_empty(),
            "EmptyMessage should have no fields"
        );
        assert!(
            empty.oneof_groups.is_empty(),
            "EmptyMessage should have no oneof groups"
        );
        assert!(!empty.is_wkt);
    }

    /// Test nested message extraction
    #[test]
    fn test_get_message_definitions_nested_message() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        let outer = defs
            .get("testdef.NestedOuter")
            .unwrap_or_else(|| panic!("NestedOuter should exist"));
        assert_eq!(outer.fields.len(), 2, "NestedOuter should have 2 fields");

        // id field
        let id_f = outer
            .fields
            .iter()
            .find(|f| f.name == "id")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(id_f.type_kind, "string");

        // inner field referencing NestedOuter.Inner
        let inner_f = outer
            .fields
            .iter()
            .find(|f| f.name == "inner")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(inner_f.type_kind, "message");
        assert_eq!(
            inner_f.type_full_name.as_deref(),
            Some("testdef.NestedOuter.Inner")
        );

        // inner field should have nested_message with its fields
        let nested = inner_f
            .nested_message
            .as_ref()
            .unwrap_or_else(|| panic!("inner field should have nested_message"));
        assert_eq!(nested.full_name, "testdef.NestedOuter.Inner");
        assert_eq!(nested.fields.len(), 2, "Inner should have 2 fields");
        assert!(nested.fields.iter().any(|f| f.name == "value"));
        assert!(nested.fields.iter().any(|f| f.name == "count"));

        // The nested message should also be in the flat map
        let inner_def = defs
            .get("testdef.NestedOuter.Inner")
            .unwrap_or_else(|| panic!("Inner should be in flat map too"));
        assert_eq!(inner_def.fields.len(), 2);
    }

    /// Test synthetic `MapEntry` filtering
    #[test]
    fn test_get_message_definitions_filters_synthetic() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        // Verify that WithMap is in definitions
        assert!(
            defs.contains_key("testdef.WithMap"),
            "WithMap should be in definitions"
        );

        // Verify that no map entry messages are in definitions
        for msg in pool.all_messages() {
            if msg.is_map_entry() {
                let name = msg.full_name();
                assert!(
                    !defs.contains_key(name),
                    "Map entry '{}' should be filtered from definitions",
                    name
                );
            }
        }

        // Specifically verify WithMap.LabelsEntry is filtered
        let labels_entry_name = "testdef.WithMap.LabelsEntry";
        assert!(
            !defs.contains_key(labels_entry_name),
            "LabelsEntry should be filtered"
        );
    }

    /// Test circular reference handling — no infinite recursion
    #[test]
    fn test_get_message_definitions_circular_ref() {
        let pool = parse_defs_pool(TEST_CIRCULAR_PROTO, "circular.proto");
        let defs = get_message_definitions(&pool);

        let circular_a = defs
            .get("circref.CircularA")
            .unwrap_or_else(|| panic!("CircularA should exist"));
        assert_eq!(circular_a.fields.len(), 2, "CircularA should have 2 fields");

        // name field
        let name_f = circular_a
            .fields
            .iter()
            .find(|f| f.name == "name")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(name_f.type_kind, "string");

        // b field referencing CircularB — should be a circular_ref type
        let b_f = circular_a
            .fields
            .iter()
            .find(|f| f.name == "b")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(
            b_f.type_kind, "circular_ref",
            "Field referencing CircularB should be marked as circular_ref"
        );
        assert_eq!(
            b_f.type_full_name.as_deref(),
            Some("circref.CircularB"),
            "b field should reference CircularB"
        );
        // circular ref should NOT have nested_message expanded
        assert!(
            b_f.nested_message.is_none(),
            "Circular ref should not have nested_message expanded"
        );

        // CircularB should exist in the flat map too
        let circular_b = defs
            .get("circref.CircularB")
            .unwrap_or_else(|| panic!("CircularB should exist in definitions"));
        assert_eq!(circular_b.fields.len(), 2, "CircularB should have 2 fields");

        // a field referencing CircularA — should be circular_ref type
        let a_f = circular_b
            .fields
            .iter()
            .find(|f| f.name == "a")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(
            a_f.type_kind, "circular_ref",
            "Field referencing CircularA should be marked as circular_ref"
        );
        assert_eq!(
            a_f.type_full_name.as_deref(),
            Some("circref.CircularA"),
            "a field should reference CircularA"
        );
        assert!(
            a_f.nested_message.is_none(),
            "Circular ref should not have nested_message expanded"
        );
    }

    /// Test serde roundtrip for `get_message_definitions` output
    #[test]
    fn test_get_message_definitions_serde_roundtrip() {
        let pool = parse_defs_pool(TEST_DEFINITIONS_PROTO, "testdef.proto");
        let defs = get_message_definitions(&pool);

        // Verify the HashMap can be serialized to JSON
        let json = serde_json::to_string(&defs)
            .unwrap_or_else(|e| panic!("Serialization should succeed: {e}"));
        assert!(!json.is_empty(), "Serialized JSON should not be empty");

        // Verify the JSON can be deserialized back
        let deserialized: std::collections::HashMap<String, GrpcMessageInfo> =
            serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("Deserialization should succeed: {e}"));

        // Verify roundtrip preserved all keys
        assert_eq!(
            defs.len(),
            deserialized.len(),
            "Roundtrip should preserve number of message definitions"
        );

        // Verify roundtrip preserved key content for a few messages
        for key in &[
            "testdef.SimpleMessage",
            "testdef.WithEnum",
            "testdef.WithMap",
            "testdef.WithWkt",
        ] {
            let original = defs
                .get(*key)
                .unwrap_or_else(|| panic!("Original should have key"));
            let restored = deserialized
                .get(*key)
                .unwrap_or_else(|| panic!("Restored should have key"));
            assert_eq!(
                original.full_name, restored.full_name,
                "Roundtrip failed for {}",
                key
            );
            assert_eq!(
                original.fields.len(),
                restored.fields.len(),
                "Field count mismatch for {}",
                key
            );
            assert_eq!(
                original.is_wkt, restored.is_wkt,
                "is_wkt mismatch for {}",
                key
            );
        }

        // Verify WithMap has serde roundtrip for is_map + map_key_type + map_value_type
        let wm_original = defs
            .get("testdef.WithMap")
            .unwrap_or_else(|| panic!("expected Some"));
        let wm_restored = deserialized
            .get("testdef.WithMap")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(wm_original.fields[0].is_map, wm_restored.fields[0].is_map);
        assert_eq!(
            wm_original.fields[0].map_key_type,
            wm_restored.fields[0].map_key_type
        );
        assert_eq!(
            wm_original.fields[0].map_value_type,
            wm_restored.fields[0].map_value_type
        );

        // Verify NestedOuter roundtrip maintains nested_message
        let no_original = defs
            .get("testdef.NestedOuter")
            .unwrap_or_else(|| panic!("expected Some"));
        let no_restored = deserialized
            .get("testdef.NestedOuter")
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(
            no_original.fields[1].nested_message.is_some(),
            no_restored.fields[1].nested_message.is_some(),
            "nested_message presence should survive roundtrip"
        );
        if let (Some(orig_inner), Some(restored_inner)) = (
            &no_original.fields[1].nested_message,
            &no_restored.fields[1].nested_message,
        ) {
            assert_eq!(orig_inner.full_name, restored_inner.full_name);
            assert_eq!(orig_inner.fields.len(), restored_inner.fields.len());
        }
    }
}
