/**
 * 轻量级 Proto 文件解析器
 *
 * 从 .proto 文件内容中提取消息定义、字段类型和枚举值。
 * 用于表单化请求编辑器渲染对应的输入控件。
 *
 * 仅支持 proto3 语法中常见的字段声明模式，不做完整语法解析。
 */

// ============================================================================
// 类型定义
// ============================================================================

/** Proto 字段的基数 */
export type ProtoCardinality = 'singular' | 'repeated';

/** Proto 字段基础类型分类 */
export type ProtoFieldTypeCategory =
  | 'string'
  | 'int'
  | 'uint'
  | 'sint'
  | 'fixed'
  | 'sfixed'
  | 'float'
  | 'double'
  | 'bool'
  | 'bytes'
  | 'enum'
  | 'message'
  | 'map'
  | 'unknown';

/** 解析后的单个字段描述 */
export interface ProtoFieldDescriptor {
  /** 字段名 */
  name: string;
  /** 原始类型字符串（如 "string", "int32", "UserStatus", "map<string, string>"） */
  rawType: string;
  /** 分类后的类型类别 */
  typeCategory: ProtoFieldTypeCategory;
  /** 基数 */
  cardinality: ProtoCardinality;
  /** 字段编号 */
  number: number;
  /** 对于 enum 类型：可选的枚举值列表 */
  enumValues?: string[];
  /** 对于 enum 类型：可选的枚举值到数字的映射 */
  enumNumberMap?: Record<string, number>;
  /** 对于 message 类型：嵌套消息的全名 */
  messageName?: string;
  /** 对于 map 类型：key 类型 */
  mapKeyType?: string;
  /** 对于 map 类型：value 类型 */
  mapValueType?: string;
  /** 注释（行尾 // 注释） */
  comment?: string;
}

/** 解析后的枚举定义 */
export interface ProtoEnumDescriptor {
  /** 枚举全名（如 "package.Status"） */
  fullName: string;
  /** 枚举值列表 */
  values: string[];
  /** 枚举值到数字的映射 */
  numberMap: Record<string, number>;
}

/** 解析后的消息定义 */
export interface ProtoMessageDescriptor {
  /** 消息全名（如 "package.GetUserRequest"） */
  fullName: string;
  /** 消息简称 */
  shortName: string;
  /** 字段列表 */
  fields: ProtoFieldDescriptor[];
}

/** 完整的 Proto 文件解析结果 */
export interface ProtoParseResult {
  /** 消息定义列表 */
  messages: ProtoMessageDescriptor[];
  /** 枚举定义列表 */
  enums: ProtoEnumDescriptor[];
  /** 包名 */
  packageName: string;
}

// ============================================================================
// 正则表达式
// ============================================================================

/** 匹配 package 声明 */
const PACKAGE_RE = /^\s*package\s+([\w.]+)\s*;/;

/** 匹配 message 开始 */
const MESSAGE_START_RE = new RegExp('^\\s*message\\s+(\\w+)\\s*\\{');

/** 匹配 enum 开始 */
const ENUM_START_RE = new RegExp('^\\s*enum\\s+(\\w+)\\s*\\{');

/** 匹配普通字段声明：[optional] type name = number; // comment */
const FIELD_RE = /^\s*(optional\s+)?(map\s*<[^>]+>|[\w.]+)\s+(\w+)\s*=\s*(\d+)\s*;?\s*(?:\/\/\s*(.*))?$/;

/** 匹配 repeated 字段声明 */
const REPEATED_FIELD_RE = /^\s*repeated\s+([\w.]+)\s+(\w+)\s*=\s*(\d+)\s*;?\s*(?:\/\/\s*(.*))?$/;

/** 匹配 map 字段声明 */
const MAP_FIELD_RE = /^\s*map\s*<\s*([\w.]+)\s*,\s*([\w.]+)\s*>\s+(\w+)\s*=\s*(\d+)\s*;?\s*(?:\/\/\s*(.*))?$/;

/** 匹配枚举值 */
const ENUM_VALUE_RE = /^\s*(\w+)\s*=\s*(-?\d+)\s*;?\s*(?:\/\/\s*(.*))?$/;

// ============================================================================
// 类型分类工具
// ============================================================================

const INT_TYPES = new Set(['int32', 'int64']);
const UINT_TYPES = new Set(['uint32', 'uint64']);
const SINT_TYPES = new Set(['sint32', 'sint64']);
const FIXED_TYPES = new Set(['fixed32', 'fixed64']);
const SFIXED_TYPES = new Set(['sfixed32', 'sfixed64']);

/**
 * 根据原始类型字符串判断字段类型类别
 */
function categorizeType(rawType: string, knownEnums: Set<string>, knownMessages: Set<string>): ProtoFieldTypeCategory {
  if (rawType === 'string') return 'string';
  if (rawType === 'bool') return 'bool';
  if (rawType === 'bytes') return 'bytes';
  if (rawType === 'float') return 'float';
  if (rawType === 'double') return 'double';
  if (INT_TYPES.has(rawType)) return 'int';
  if (UINT_TYPES.has(rawType)) return 'uint';
  if (SINT_TYPES.has(rawType)) return 'sint';
  if (FIXED_TYPES.has(rawType)) return 'fixed';
  if (SFIXED_TYPES.has(rawType)) return 'sfixed';
  if (knownEnums.has(rawType)) return 'enum';
  if (knownMessages.has(rawType)) return 'message';
  // 带包名前缀的引用
  if (rawType.includes('.')) {
    const shortName = rawType.split('.').pop() || '';
    if (knownEnums.has(shortName)) return 'enum';
    if (knownMessages.has(shortName)) return 'message';
  }
  return 'unknown';
}

// ============================================================================
// 解析器
// ============================================================================

/**
 * 解析多个 proto 文件内容，返回合并后的解析结果
 */
export function parseProtoFiles(files: Array<{ path: string; content: string }>): ProtoParseResult {
  const merged: ProtoParseResult = {
    messages: [],
    enums: [],
    packageName: '',
  };

  for (const file of files) {
    const result = parseProtoContent(file.content);
    if (result.packageName && !merged.packageName) {
      merged.packageName = result.packageName;
    }
    merged.messages.push(...result.messages);
    merged.enums.push(...result.enums);
  }

  // 解析后补充枚举引用和消息引用
  const enumFullNames = new Set(merged.enums.map(e => e.fullName));
  const enumShortNames = new Set(merged.enums.map(e => e.fullName.split('.').pop()!));
  const msgFullNames = new Set(merged.messages.map(m => m.fullName));
  const msgShortNames = new Set(merged.messages.map(m => m.fullName.split('.').pop()!));

  const allEnumNames = new Set([...enumFullNames, ...enumShortNames]);
  const allMsgNames = new Set([...msgFullNames, ...msgShortNames]);

  for (const msg of merged.messages) {
    for (const field of msg.fields) {
      // 为 enum 类型填充枚举值
      if (field.typeCategory === 'enum' || field.rawType.includes('.')) {
        const enumDef = merged.enums.find(e =>
          e.fullName === field.rawType || e.fullName.endsWith('.' + field.rawType)
        );
        if (enumDef) {
          field.typeCategory = 'enum';
          field.enumValues = enumDef.values;
          field.enumNumberMap = enumDef.numberMap;
        }
      }
      // 为 message 类型填充消息名
      if (field.typeCategory === 'message' || (field.typeCategory === 'unknown' && field.rawType.includes('.'))) {
        const msgDef = merged.messages.find(m =>
          m.fullName === field.rawType || m.fullName.endsWith('.' + field.rawType)
        );
        if (msgDef) {
          field.typeCategory = 'message';
          field.messageName = msgDef.fullName;
        }
      }
      // 重新分类 unknown 类型
      if (field.typeCategory === 'unknown') {
        field.typeCategory = categorizeType(field.rawType, allEnumNames, allMsgNames);
      }
    }
  }

  return merged;
}

/**
 * 解析单个 proto 文件内容
 */
export function parseProtoContent(content: string): ProtoParseResult {
  const lines = content.split('\n');
  const result: ProtoParseResult = {
    messages: [],
    enums: [],
    packageName: '',
  };

  // 第一遍：提取包名、枚举短名集合（用于分类）
  let packageName = '';
  for (const line of lines) {
    const pkgMatch = PACKAGE_RE.exec(line);
    if (pkgMatch) {
      packageName = pkgMatch[1];
      break;
    }
  }
  result.packageName = packageName;

  // 收集顶层枚举短名
  const topEnumShortNames = new Set<string>();
  for (const line of lines) {
    const enumMatch = ENUM_START_RE.exec(line);
    if (enumMatch) {
      topEnumShortNames.add(enumMatch[1]);
    }
  }

  // 收集顶层消息短名
  const topMsgShortNames = new Set<string>();
  for (const line of lines) {
    const msgMatch = MESSAGE_START_RE.exec(line);
    if (msgMatch) {
      topMsgShortNames.add(msgMatch[1]);
    }
  }

  // 第二遍：解析消息和枚举定义
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];

    // 检测 message 开始
    const msgMatch = MESSAGE_START_RE.exec(line);
    if (msgMatch) {
      const msgName = msgMatch[1];
      const fullName = packageName ? `${packageName}.${msgName}` : msgName;
      const { fields, endLine } = parseMessageBlock(lines, i + 1, packageName, topEnumShortNames, topMsgShortNames);
      result.messages.push({
        fullName,
        shortName: msgName,
        fields,
      });
      i = endLine + 1;
      continue;
    }

    // 检测 enum 开始
    const enumMatch = ENUM_START_RE.exec(line);
    if (enumMatch) {
      const enumName = enumMatch[1];
      const fullName = packageName ? `${packageName}.${enumName}` : enumName;
      const { values, numberMap, endLine } = parseEnumBlock(lines, i + 1);
      result.enums.push({
        fullName,
        values,
        numberMap,
      });
      i = endLine + 1;
      continue;
    }

    i++;
  }

  return result;
}

/**
 * 解析 message 块中的字段
 */
function parseMessageBlock(
  lines: string[],
  startLine: number,
  _packageName: string,
  knownEnums: Set<string>,
  knownMessages: Set<string>
): { fields: ProtoFieldDescriptor[]; endLine: number } {
  const fields: ProtoFieldDescriptor[] = [];
  let depth = 1; // 已经进入了一层 {
  let currentLine = startLine;

  while (currentLine < lines.length && depth > 0) {
    const line = lines[currentLine];

    // 统计花括号深度
    const opens = (line.match(new RegExp('\\{', 'g')) || []).length;
    const closes = (line.match(new RegExp('\\}', 'g')) || []).length;
    depth += opens - closes;

    if (depth <= 0) break;

    // 跳过嵌套 message/enum/service/oneof 声明（不做递归解析）
    if (MESSAGE_START_RE.test(line) || ENUM_START_RE.test(line) || /^\s*oneof\s+/.test(line) || /^\s*service\s+/.test(line)) {
      currentLine++;
      continue;
    }

    // 尝试匹配 map 字段
    const mapMatch = MAP_FIELD_RE.exec(line);
    if (mapMatch) {
      const [, keyType, valueType, name, numStr, comment] = mapMatch;
      fields.push({
        name,
        rawType: `map<${keyType}, ${valueType}>`,
        typeCategory: 'map',
        cardinality: 'repeated',
        number: parseInt(numStr, 10),
        mapKeyType: keyType,
        mapValueType: valueType,
        comment: comment || undefined,
      });
      currentLine++;
      continue;
    }

    // 尝试匹配 repeated 字段
    const repeatedMatch = REPEATED_FIELD_RE.exec(line);
    if (repeatedMatch) {
      const [, rawType, name, numStr, comment] = repeatedMatch;
      const typeCategory = categorizeType(rawType, knownEnums, knownMessages);
      fields.push({
        name,
        rawType,
        typeCategory,
        cardinality: 'repeated',
        number: parseInt(numStr, 10),
        messageName: typeCategory === 'message' ? rawType : undefined,
        comment: comment || undefined,
      });
      currentLine++;
      continue;
    }

    // 尝试匹配普通字段
    const fieldMatch = FIELD_RE.exec(line);
    if (fieldMatch) {
      const [, , rawType, name, numStr, comment] = fieldMatch;
      const typeCategory = categorizeType(rawType, knownEnums, knownMessages);
      fields.push({
        name,
        rawType,
        typeCategory,
        cardinality: 'singular',
        number: parseInt(numStr, 10),
        messageName: typeCategory === 'message' ? rawType : undefined,
        comment: comment || undefined,
      });
      currentLine++;
      continue;
    }

    currentLine++;
  }

  return { fields, endLine: currentLine };
}

/**
 * 解析 enum 块中的值
 */
function parseEnumBlock(
  lines: string[],
  startLine: number
): { values: string[]; numberMap: Record<string, number>; endLine: number } {
  const values: string[] = [];
  const numberMap: Record<string, number> = {};
  let depth = 1;
  let currentLine = startLine;

  while (currentLine < lines.length && depth > 0) {
    const line = lines[currentLine];

    const opens = (line.match(new RegExp('\\{', 'g')) || []).length;
    const closes = (line.match(new RegExp('\\}', 'g')) || []).length;
    depth += opens - closes;

    if (depth <= 0) break;

    const enumMatch = ENUM_VALUE_RE.exec(line);
    if (enumMatch) {
      const [, name, numStr] = enumMatch;
      // 跳过 proto 默认的 UNDEFINED = 0 保留值
      if (name === '_UNDEFINED' || name === 'UNDEFINED') {
        currentLine++;
        continue;
      }
      values.push(name);
      numberMap[name] = parseInt(numStr, 10);
    }

    currentLine++;
  }

  return { values, numberMap, endLine: currentLine };
}

// ============================================================================
// 查找工具
// ============================================================================

/**
 * 根据 service_name + method_name 查找方法的 input message 描述
 *
 * @param parseResult 解析结果
 * @param inputType 方法输入类型全名（如 "package.GetUserRequest"）
 * @returns 消息描述，找不到返回 null
 */
export function findMessageDescriptor(
  parseResult: ProtoParseResult,
  inputType: string
): ProtoMessageDescriptor | null {
  // 精确匹配
  const exact = parseResult.messages.find(m => m.fullName === inputType);
  if (exact) return exact;

  // 按短名匹配（处理无包名的情况）
  const shortName = inputType.split('.').pop() || inputType;
  return parseResult.messages.find(m => m.shortName === shortName) || null;
}

/**
 * 根据 proto 字段描述符生成默认 JSON 值
 */
export function fieldDefaultValue(field: ProtoFieldDescriptor): unknown {
  if (field.cardinality === 'repeated' && field.typeCategory !== 'map') {
    return [];
  }
  if (field.typeCategory === 'map') {
    return {};
  }
  switch (field.typeCategory) {
    case 'string': return '';
    case 'bool': return false;
    case 'int': case 'uint': case 'sint': case 'fixed': case 'sfixed':
    case 'float': case 'double':
      return 0;
    case 'enum':
      // 枚举默认值取第一个非零值的名称，没有则返回 0
      if (field.enumValues && field.enumValues.length > 0) {
        return field.enumValues[0];
      }
      return 0;
    case 'message':
      return {};
    default:
      return '';
  }
}
