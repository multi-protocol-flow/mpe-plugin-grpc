# MPE gRPC 插件

> **定位**：本仓库是 MPE 的**官方协议插件之一**——它不依赖宿主仓库的任何代码，只依赖公开的 `mpe-plugin-sdk`（sidecar 进程 + stdio 上的 JSON-RPC，git tag `v0.2.2`）。宿主在启动时扫描插件目录，执行 `describe` 握手，注册节点类型，再通过 `execute` RPC 调用本插件进程。
>
> 本仓库与宿主相互独立（宿主仓库的 `.gitignore` 忽略了 `/plugins/`，且宿主从不构建它）。它由本仓库自己的 CI（GitHub Actions → Release artifacts）完成构建与发布。

---

## 0. 一分钟了解插件工作方式

```
Host (mpe / mpe-cli)                    Plugin process (本 crate)
   │  scans plugins/ dir                       │
   │  ── describe ───────────────────────────► │  返回节点描述（type、ports、config schema）
   │  ◄─────────── node list ─────────────────  │
   │  ── execute(config, execution_id) ──────► │  执行 gRPC 操作
   │  ◄─────────── result / stream events ────  │
   │  ── flowEnded(execution_id) ────────────► │  释放该次执行的连接池
```

- **传输**：stdin/stdout，每行一个 JSON 文档（JSON-RPC 2.0，LF 帧）
- **驻留模式**：`capabilities.streaming: true` → 进程常驻，连接池跨多次执行复用
- **单节点连通性验证**：`grpc:connect` 声明了 `capabilities.single_node: true`，宿主会在 `mpe run-node` / GUI 测试按钮中放行，无需宿主侧额外代码
- **无共享内存**：插件是独立进程，宿主只能通过 JSON 传参

## 1. 项目结构

```
mpe-plugin-grpc/
├── Cargo.toml            # 独立包，不依赖宿主 workspace
├── plugin.json           # 宿主扫描的清单文件（启动描述、驻留模式）
├── .github/workflows/ci.yml  # 4 平台构建 + 测试 + Release 打包
├── src/
│   ├── main.rs           # 二进制入口（安装 rustls + SDK 事件循环）
│   ├── lib.rs            # GrpcPlugin：Plugin trait 实现 + 节点描述
│   ├── i18n.rs           # 基于 MPE_LOCALE 的中/英文案
│   ├── pool.rs           # 按 execution_id 管理的连接池
│   ├── types.rs          # gRPC 领域类型（service/method/message/stream/error）
│   ├── codec.rs          # 压缩 + metadata 编解码辅助
│   ├── error_detail.rs   # 结构化 gRPC error details
│   ├── grpc_connect_executor.rs  # grpc:connect
│   ├── grpc_call_executor.rs     # grpc:call
│   ├── grpc_close_executor.rs    # grpc:close
│   ├── proto_parser.rs           # proto 文件解析
│   ├── reflection.rs             # Server Reflection v1 客户端
│   ├── tls.rs                    # rustls TLS 配置辅助
│   ├── helpers.rs                # 杂项辅助
│   └── ui.rs                     # 配置面板 UI 辅助
├── frontend/
│   ├── package.json
│   ├── vite.config.ts
│   ├── vite.config.viewer.ts
│   ├── src/
│   │   ├── viewer.tsx / panel.tsx / bridge.ts / styles.css
│   │   ├── i18n.ts / types.ts / cn.ts
│   │   ├── panels/        # 配置面板（ConnectPanel / CallPanel / ClosePanel / shared）
│   │   ├── viewer/        # 报告查看器（ViewerApp + shared）
│   │   └── lib/           # proto 字段解析、grpcurl 生成、UI 辅助、GrpcFormEditor、MessageSchemaView
│   ├── viewer.html
│   ├── panel.html
│   └── scripts/inline.mjs
└── tests/
    └── roundtrip.rs      # 离线 stdio roundtrip 测试（describe / 失败路径）
```

## 2. 节点类型

| type_id | ports | 说明 |
|---------|-------|------|
| `grpc:connect` | in/true/false | 建立 gRPC 连接（url/tls/reflection/proto/metadata/keepalive/retry），连接池按 `(execution_id, 节点实例)` 复用；声明 `single_node: true` |
| `grpc:call` | in/true/false | 调用 gRPC 方法（unary / server streaming / client streaming / bidi），服务和方法来源支持 proto 发现或手动填写 |
| `grpc:close` | in/out | 按 `connection_id` 显式释放单条连接 |

`grpc:call` 和 `grpc:close` 使用宿主 `x-node-selector` 渲染出的 `connection_id` 字段绑定到同一流程中的 `grpc:connect` 节点；宿主在运行时把 connect 节点的 instance id 注入到 config。

## 3. Cargo.toml（独立构建）

```toml
[package]
name = "mpe-plugin-grpc"
version = "0.1.0"
edition = "2021"
description = "gRPC protocol sidecar plugin: connect/call/close"

[dependencies]
mpe-plugin-sdk = { git = "https://github.com/multi-protocol-flow/mpe-plugin-sdk.git", tag = "v0.2.2" }
tonic = { version = "0.12", default-features = false, features = ["prost", "transport", "tls", "tls-webpki-roots", "gzip"] }
prost = "0.13"
prost-reflect = { version = "0.14", features = ["serde"] }
prost-types = "0.13"
protox = "0.7"
tonic-reflection = "0.12"
tonic-health = "0.12"
tokio-stream = { version = "0.1", features = ["net"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "logging", "std", "tls12"] }
rustls-pemfile = "2"
tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "logging", "tls12"] }
webpki-roots = "0.26"
hyper-util = "0.1"
tower = { version = "0.4", default-features = false }
http = "1"
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }
tokio = { version = "1", features = ["full"] }
log = "0.4"
thiserror = "1.0"
chrono = "0.4"
rand = "0.8"
tokio-util = "0.7"
tempfile = "3"

[[bin]]
name = "mpe_plugin_grpc"
path = "src/main.rs"
```

## 4. 构建与测试

> **plugin.json `entry.command`**：采用 marketplace 约定的相对路径形式，即 `./mpe_plugin_grpc`。当前宿主以自身 CWD 解析该命令（不会切换到插件目录）。在本仓库开发时，如需让宿主直接启动插件，可将 `command` 改为你机器上的绝对路径，或者从插件目录启动宿主。

```bash
# 构建 release 二进制（宿主通过 plugin.json entry.command 启动它）
cargo build --release

# 离线单元测试 + stdio roundtrip 测试（不需要 gRPC 服务器）
cargo test


# 前端构建（配置面板 + 报告查看器）
cd frontend && npm install && npm run build
```

## 5. 安装与验证（宿主侧）

```bash
# 开发：构建到仓库根目录（宿主扫描 MPE_PLUGIN_DIR 或其数据目录下的 plugins 目录）
cargo build --release
cd frontend && npm run build

# 单节点连通性验证（能力驱动，宿主侧无需白名单变更）
mpe run-node '{"type":"grpc:connect","url":"localhost:50051"}'

# 端到端流程：connect → call → close
mpe run-flow tests/fixtures/flows/grpc_plugin_e2e.json
```

## 6. 设计说明

- **动态 proto / 反射**：`grpc:connect` 可从 config 加载 `.proto` 文件，也支持通过 Server Reflection v1 发现服务；发现结果缓存到 `discovered_services` 并展示在配置面板。
- **流式**：unary / server / client / bidi 均由同一个 `grpc:call` executor 处理；流式响应通过 `grpc.stream` backpressure 通知实时推给报告查看器渲染。
- **连接池**：键为 `(execution_id, connection_uuid)`；并发执行互不干扰。通过 `flow_ended` 钩子和 `grpc:close` 节点双重释放；释放不存在连接返回错误（与 `flow_ended` 的 no-op 路径不同）。
- **TLS**：使用 `rustls` + `ring` 后端（与宿主及 MCP 插件一致）。支持单向 TLS、mTLS（`tls_client_cert` / `tls_client_key`）、自定义 CA（`tls_ca_cert`）和 `tls_server_name_override`。
- **重试 / keepalive**：连接级可配，调用级可覆盖；基于 tonic 内置的 retry + keepalive 设置。
- **国际化**：Rust 侧节点描述与前端配置/viewer 页面均内置 `zh-CN` / `en-US`，由宿主注入的 `MPE_LOCALE` 环境变量决定。

## 7. 与宿主的关系

- 依赖：仅依赖 `mpe-plugin-sdk`（`mpe-plugin-sdk::protocol` wire contract）——绝不引入宿主类型，如 `flow-engine-core`。
- 交互：describe/execute/validate/shutdown RPC + `grpc.stream` 通知 + `flowEnded` 通知。
- 宿主适配点：`capabilities.single_node` 由宿主的 `execute_single_node` / `mpe run-node` 能力门控读取（`NodePlugin::supports_single_node()` + `SidecarPlugin` passthrough）；插件无需了解宿主实现细节。
