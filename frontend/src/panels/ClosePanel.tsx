/**
 * gRPC Close node config panel (port of `GrpcCloseConfig.tsx`).
 */
import { useMemo } from 'react';
import type { GrpcCloseConfig, GrpcConnectConfig, GrpcNodeSnapshot } from '../types';
import { t } from '../i18n';
import { IconInfo, IconUnplug, Input, Label, Select, SelectItem } from '../lib/ui';

interface ClosePanelProps {
  config: GrpcCloseConfig;
  nodes: GrpcNodeSnapshot[];
  onChange: (config: GrpcCloseConfig) => void;
}

export function ClosePanel({ config, nodes, onChange }: ClosePanelProps) {
  const availableConnections = useMemo(() => {
    return nodes
      .filter(
        (n) =>
          n.type === 'grpc:connect' &&
          n.config &&
          typeof n.config === 'object' &&
          typeof (n.config as unknown as GrpcConnectConfig).url === 'string',
      )
      .map((n) => ({ uuid: n.uuid, name: n.label }));
  }, [nodes]);

  return (
    <div className="panel">
      {/* 主说明卡片 */}
      <div className="card">
        <h4 className="card-title">
          <IconUnplug size={16} />
          {t('panels.grpcClose.title')}
        </h4>
        <p className="card-desc">{t('panels.grpcClose.description')}</p>
      </div>

      {/* 连接选择 */}
      <div className="field">
        <Label htmlFor="connection_id" className="field-label">
          {t('panels.common.connection')} <span className="req">*</span>
        </Label>
        {availableConnections.length > 0 ? (
          <Select
            id="connection_id"
            value={config.connection_id || ''}
            onValueChange={(value) => onChange({ ...config, connection_id: value })}
            placeholder={t('panels.common.selectConnection')}
            className="mono"
          >
            {availableConnections.map((conn) => (
              <SelectItem key={conn.uuid} value={conn.uuid}>
                {conn.name} ({conn.uuid.substring(0, 8)}...)
              </SelectItem>
            ))}
          </Select>
        ) : (
          <Input
            id="connection_id"
            value={config.connection_id ?? ''}
            onChange={(e) => onChange({ ...config, connection_id: e.target.value })}
            placeholder={t('panels.common.enterConnectionId')}
            className="mono"
          />
        )}
        {availableConnections.length === 0 && (
          <p className="hint">{t('panels.grpcClose.noConnectNodeDetected')}</p>
        )}
      </div>

      {/* 提示信息 */}
      <div className="help-card">
        <span className="alert-icon">
          <IconInfo size={14} className="muted" />
        </span>
        <div className="help-body">
          <p className="help-title">{t('panels.common.closeInfo')}</p>
          <p>{t('panels.grpcClose.closeInfoLine1')}</p>
          <p>{t('panels.common.closeInfoLine2')}</p>
        </div>
      </div>
    </div>
  );
}
