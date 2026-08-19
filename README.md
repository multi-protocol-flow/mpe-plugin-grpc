# MPE gRPC Plugin

> **Positioning**: this plugin is one of MPE's **official protocol plugins** — it
> depends on none of the host repository's code, only the public
> `mpe-plugin-sdk` (sidecar process + JSON-RPC over stdio, git tag `v0.2.2`).
> The host scans its plugin directory at startup, runs the `describe` handshake,
> registers the node types, and calls this plugin process via the `execute` RPC.
>
> This repository is independent of the host (the host repository's `.gitignore`
> ignores `/plugins/` and the host never builds it). It is built and released by
> this repository's own CI (GitHub Actions → Release artifacts).

---

## 0. How the plugin works in one minute

```
Host (mpe / mpe-cli)                    Plugin process (this crate)
   │  scans plugins/ dir                       │
   │  ── describe ───────────────────────────► │  returns node descriptions (type, ports, config schema)
   │  ◄─────────── node list ─────────────────  │
   │  ── execute(config, execution_id) ──────► │  runs the gRPC operation
   │  ◄─────────── result / stream events ────  │
   │  ── flowEnded(execution_id) ────────────► │  releases the per-execution connection pool
```

- **Transport**: stdin/stdout, one JSON document per line (JSON-RPC 2.0, LF-framed)
- **Resident**: `capabilities.streaming: true` → the process stays alive, the
  connection pool is reused across executions
- **Single-node verification**: `grpc:connect` declares
  `capabilities.single_node: true` → the host's `mpe run-node` / GUI test button
  can verify reachability without any host code change
- **No shared memory**: the plugin is a separate process; the host can only pass
  values as JSON

## 1. Project structure

```
mpe-plugin-grpc/
├── Cargo.toml            # standalone package, no host workspace dependency
├── plugin.json           # manifest scanned by the host (launch description, residency mode)
├── build.rs              # compiles echo.proto for the test server (dev-tool only)
├── .github/workflows/ci.yml  # 4-platform build + tests + Release packaging
├── src/
│   ├── main.rs           # binary entry point (rustls install + SDK event loop)
│   ├── lib.rs            # GrpcPlugin: Plugin trait impl + node descriptions
│   ├── i18n.rs           # MPE_LOCALE-driven zh-CN / en-US copy
│   ├── pool.rs           # per-execution connection pool (execution_id → manager)
│   ├── types.rs          # gRPC domain types (service/method/message/stream/error)
│   ├── codec.rs          # compression + metadata codec helpers
│   ├── error_detail.rs   # structured gRPC error details
│   ├── grpc_connect_executor.rs  # grpc:connect
│   ├── grpc_call_executor.rs     # grpc:call
│   ├── grpc_close_executor.rs    # grpc:close
│   ├── proto_parser.rs           # proto file parser
│   ├── reflection.rs             # Server Reflection v1 client
│   ├── tls.rs                    # rustls TLS config helpers
│   ├── helpers.rs                # misc helpers
│   └── ui.rs                     # config panel UI helpers
├── frontend/
│   ├── package.json
│   ├── vite.config.ts
│   ├── vite.config.viewer.ts
│   ├── src/
│   │   ├── viewer.tsx / panel.tsx / bridge.ts / styles.css
│   │   ├── i18n.ts / types.ts / cn.ts
│   │   ├── panels/        # config panels (ConnectPanel / CallPanel / ClosePanel / shared)
│   │   ├── viewer/        # report viewer (ViewerApp + shared)
│   │   └── lib/           # proto field parser, grpcurl generator, UI helpers, GrpcFormEditor, MessageSchemaView
│   ├── viewer.html
│   ├── panel.html
│   └── scripts/inline.mjs
├── tests/
│   ├── roundtrip.rs      # offline stdio roundtrip tests (describe / failure paths)
│   └── integration.rs    # real-server lifecycle tests (unary / streaming / reflection / cancel)
└── src/bin/
    ├── test_server.rs    # echo test server used by integration tests
    └── echo.proto        # proto definition for the test server
```

## 2. Node types

| type_id | ports | description |
|---------|-------|-------------|
| `grpc:connect` | in/true/false | establish a connection (url/tls/reflection/proto files/metadata/keepalive/retry), pool keyed by `(execution_id, node instance)`; declares `single_node: true` |
| `grpc:call` | in/true/false | invoke a gRPC method (unary / server streaming / client streaming / bidi), service/method from proto discovery or manual entry |
| `grpc:close` | in/out | release one connection by `connection_id` |

`grpc:call` and `grpc:close` use the `connection_id` field rendered by the
host's `x-node-selector` to bind to a `grpc:connect` node in the same flow;
the host injects the connect node's instance id into the config at runtime.

## 3. Cargo.toml (standalone build)

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

[build-dependencies]
tonic-build = "0.12"

[[bin]]
name = "mpe_plugin_grpc"
path = "src/main.rs"

[[bin]]
name = "grpc_test_server"
path = "src/bin/test_server.rs"
```

## 4. Build and test

> **plugin.json `entry.command`**: a relative path in marketplace-contract form,
> `./mpe_plugin_grpc`. The current host resolves the command against its own CWD
> (it does not chdir into the plugin directory). When developing in this
> repository, if you want the host to launch the plugin directly, either change
> `command` to an absolute path on your machine, or start the host from the
> plugin directory.

```bash
# build the release binary (the host launches it via plugin.json entry.command)
cargo build --release

# offline unit tests + stdio roundtrip tests (no gRPC server needed)
cargo test

# integration tests (needs the echo test server; skipped if server binary is absent)
cargo test --test integration

# frontend build (config panel + viewer)
cd frontend && npm install && npm run build
```

## 5. Install and verify (host side)

```bash
# development: build into the repository root (the host scans MPE_PLUGIN_DIR or
# the plugins directory under its data dir)
cargo build --release
cd frontend && npm run build

# single-node reachability verification (capability-driven, no host whitelist change)
mpe run-node '{"type":"grpc:connect","url":"localhost:50051"}'

# full flow: connect → call → close
mpe run-flow tests/fixtures/flows/grpc_plugin_e2e.json
```

## 6. Design notes

- **Dynamic proto / reflection**: `grpc:connect` can load `.proto` files from
  the config or discover services via Server Reflection v1; discovered services
  are cached in `discovered_services` and surfaced in the config panel.
- **Streaming**: unary / server / client / bidi are all handled by the same
  `grpc:call` executor; streaming responses emit `grpc.stream` backpressure
  notifications so the report viewer can render messages in real time.
- **Connection pool**: keyed by `(execution_id, connection_uuid)`; concurrent
  executions never interfere. Released through both the `flow_ended` hook and
  the `grpc:close` node; releasing a missing connection returns an error (unlike
  the `flow_ended` no-op path).
- **TLS**: uses `rustls` with the `ring` backend (mirrors the host and the MCP
  plugin). Supports one-way TLS, mTLS (`tls_client_cert`/`tls_client_key`),
  custom CA (`tls_ca_cert`), and `tls_server_name_override`.
- **Retries / keepalive**: configurable at connect level and overridable at
  call level; uses tonic's built-in retry + keepalive settings.
- **i18n**: both Rust-side node descriptions and the frontend config/viewer
  pages ship `zh-CN` and `en-US` strings, selected by the host-injected
  `MPE_LOCALE` environment variable.

## 7. Relationship with the host

- Dependency: only `mpe-plugin-sdk` (`mpe-plugin-sdk::protocol` wire contract)
  — **never** host types such as `flow-engine-core`.
- Interaction: describe/execute/validate/shutdown RPCs + `grpc.stream`
  notifications + `flowEnded` notifications.
- Host adaptation point: `capabilities.single_node` is read by the host's
  `execute_single_node` / `mpe run-node` capability gate
  (`NodePlugin::supports_single_node()` + `SidecarPlugin` passthrough); the
  plugin never needs to know host implementation details.
# trigger CI
