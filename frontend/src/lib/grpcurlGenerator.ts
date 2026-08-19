/**
 * grpcurl 命令生成器
 * 根据当前 gRPC Call 配置生成等效的 grpcurl CLI 命令
 */

import type { GrpcMetadataEntry, GrpcConnectConfig, ProtoFile } from '../types';

/** grpcurl 生成器输入参数 */
export interface GrpcurlGeneratorParams {
  /** 服务器地址（来自 grpc_connect 节点的 url） */
  serverAddress: string;
  /** 是否使用 TLS（来自 grpc_connect 节点） */
  useTls?: boolean;
  /** 是否跳过 TLS 证书验证 */
  tlsSkipVerify?: boolean;
  /** TLS CA 证书路径 */
  tlsCaCert?: string;
  /** TLS 客户端证书路径 */
  tlsClientCert?: string;
  /** TLS 客户端密钥路径 */
  tlsClientKey?: string;
  /** 服务名称（如 "package.ServiceName"） */
  serviceName: string;
  /** 方法名称 */
  methodName: string;
  /** 请求 JSON */
  requestJson?: string;
  /** 调用级 metadata */
  callMetadata?: GrpcMetadataEntry[];
  /** 连接级默认 metadata */
  defaultMetadata?: GrpcMetadataEntry[];
  /** Proto 文件列表 */
  protoFiles?: ProtoFile[];
  /** 是否启用 Server Reflection */
  enableReflection?: boolean;
}

/**
 * 将 gRPC 配置转换为 grpcurl 命令字符串
 *
 * grpcurl 格式: grpcurl [options] [address] [service]/[method]
 */
export function generateGrpcurlCommand(params: GrpcurlGeneratorParams): string {
  const parts: string[] = ['grpcurl'];

  // TLS 配置
  if (!params.useTls) {
    // 明文模式
    parts.push('-plaintext');
  } else if (params.tlsSkipVerify) {
    // 跳过证书验证
    parts.push('-insecure');
  }

  // TLS 证书
  if (params.tlsCaCert) {
    parts.push('-cacert', shellQuote(params.tlsCaCert));
  }
  if (params.tlsClientCert) {
    parts.push('-cert', shellQuote(params.tlsClientCert));
  }
  if (params.tlsClientKey) {
    parts.push('-key', shellQuote(params.tlsClientKey));
  }

  // Metadata（合并连接级和调用级）
  const mergedMetadata = mergeMetadata(params.defaultMetadata, params.callMetadata);
  for (const entry of mergedMetadata) {
    if (entry.key.trim()) {
      parts.push('-H', shellQuote(`${entry.key}: ${entry.value}`));
    }
  }

  // 请求体
  if (params.requestJson && params.requestJson.trim() !== '' && params.requestJson.trim() !== '{}') {
    parts.push('-d', shellQuote(params.requestJson));
  }

  // Proto 文件配置（非 Reflection 模式）
  if (!params.enableReflection && params.protoFiles && params.protoFiles.length > 0) {
    // 提取公共目录路径和文件名
    const paths = params.protoFiles.map(f => f.path);
    const importPath = findCommonDir(paths);
    if (importPath) {
      parts.push('-import-path', shellQuote(importPath));
    }
    // 添加每个 proto 文件
    for (const pf of params.protoFiles) {
      const relativePath = importPath
        ? pf.path.slice(importPath.length + 1)
        : pf.path;
      parts.push('-proto', shellQuote(relativePath));
    }
  }

  // 服务器地址
  let address = params.serverAddress;
  // grpcurl 需要 host:port 格式，去掉协议前缀
  // (RegExp built from a string so the inlined bundle stays bracket-balanced:
  //  the literal /^https?:\/\// would look like a `//` comment to the plugin's
  //  HTML balance checker and swallow the rest of the line)
  address = address.replace(new RegExp('^https?:\\/\\/'), '');
  parts.push(shellQuote(address));

  // 服务/方法
  parts.push(`${params.serviceName}/${params.methodName}`);

  return parts.join(' ');
}

/**
 * 从 grpc_connect 节点配置中提取 grpcurl 所需的参数
 */
export function extractGrpcurlParamsFromConnectConfig(
  connectConfig: GrpcConnectConfig
): Pick<GrpcurlGeneratorParams,
  | 'serverAddress' | 'useTls' | 'tlsSkipVerify'
  | 'tlsCaCert' | 'tlsClientCert' | 'tlsClientKey'
  | 'defaultMetadata' | 'protoFiles' | 'enableReflection'
> {
  return {
    serverAddress: connectConfig.url,
    useTls: connectConfig.use_tls,
    tlsSkipVerify: connectConfig.tls_skip_verify,
    // 注意: grpc_connect 中 TLS 证书存储的是 PEM 内容而非路径
    // grpcurl 需要文件路径，这里显示占位提示
    tlsCaCert: connectConfig.tls_ca_cert ? '<ca-cert-path>' : undefined,
    tlsClientCert: connectConfig.tls_client_cert ? '<client-cert-path>' : undefined,
    tlsClientKey: connectConfig.tls_client_key ? '<client-key-path>' : undefined,
    defaultMetadata: connectConfig.default_metadata,
    protoFiles: connectConfig.proto_files,
    enableReflection: connectConfig.enable_reflection,
  };
}

/**
 * 合并连接级和调用级 metadata（调用级覆盖同名键）
 */
function mergeMetadata(
  defaultMetadata?: GrpcMetadataEntry[],
  callMetadata?: GrpcMetadataEntry[]
): GrpcMetadataEntry[] {
  const result: GrpcMetadataEntry[] = [];

  // 添加连接级 metadata
  if (defaultMetadata) {
    for (const entry of defaultMetadata) {
      if (entry.key.trim()) {
        result.push({ key: entry.key, value: entry.value });
      }
    }
  }

  // 添加调用级 metadata（覆盖同名的连接级）
  if (callMetadata) {
    for (const entry of callMetadata) {
      if (entry.key.trim()) {
        const existingIdx = result.findIndex(r => r.key === entry.key);
        if (existingIdx >= 0) {
          result[existingIdx] = { key: entry.key, value: entry.value };
        } else {
          result.push({ key: entry.key, value: entry.value });
        }
      }
    }
  }

  return result;
}

/**
 * Shell 引号包裹
 * 使用单引号包裹，内部单引号转义为 '\''
 * (string concatenation instead of a template literal so the inlined bundle
 *  stays bracket-balanced — the balance checker scans '...' as strings and a
 *  backtick template containing quotes would throw it off)
 */
function shellQuote(str: string): string {
  // 如果字符串不含特殊字符，可以不加引号
  if (/^[a-zA-Z0-9_./:@-]+$/.test(str)) {
    return str;
  }
  // 使用单引号包裹，转义内部单引号: ' -> '\''
  const escaped = str.split("'").join("'\\''");
  return "'" + escaped + "'";
}

/**
 * 查找一组文件路径的公共父目录
 */
function findCommonDir(paths: string[]): string | null {
  if (paths.length === 0) return null;
  if (paths.length === 1) {
    // 单文件时取其所在目录
    const lastSlash = Math.max(paths[0].lastIndexOf('/'), paths[0].lastIndexOf('\\'));
    return lastSlash >= 0 ? paths[0].substring(0, lastSlash) : null;
  }

  const normalizedPaths = paths.map(p => p.replace(/\\/g, '/'));
  const firstParts = normalizedPaths[0].split('/');

  let commonLength = firstParts.length - 1; // 去掉文件名
  for (let i = 1; i < normalizedPaths.length; i++) {
    const parts = normalizedPaths[i].split('/');
    let matchLen = 0;
    const maxLen = Math.min(commonLength, parts.length - 1);
    for (let j = 0; j < maxLen; j++) {
      if (firstParts[j] === parts[j]) {
        matchLen = j + 1;
      } else {
        break;
      }
    }
    commonLength = Math.min(commonLength, matchLen);
  }

  if (commonLength <= 0) return null;
  return firstParts.slice(0, commonLength).join('/');
}
