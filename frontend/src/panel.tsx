/**
 * Config-panel entry: renders the connect / call / close panel based on the
 * `init` message's `nodeType`, keeps the local config state in sync, and
 * reports changes via the debounced `configChanged` bridge message.
 */
import { StrictMode, useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import './styles.css';
import type { PluginIframeInitPayload } from './types';
import { initBridge, notifyConfig, postError } from './bridge';
import { setLocale } from './i18n';
import { ConnectPanel } from './panels/ConnectPanel';
import { CallPanel } from './panels/CallPanel';
import { ClosePanel } from './panels/ClosePanel';
import type { GrpcCallConfig, GrpcCloseConfig, GrpcConnectConfig } from './types';

type PanelState = {
  nodeType: string;
  config: Record<string, unknown>;
  nodes: PluginIframeInitPayload['nodes'];
  ready: boolean;
};

const initialState: PanelState = {
  nodeType: '',
  config: {},
  nodes: undefined,
  ready: false,
};

function PanelApp() {
  const [state, setState] = useState<PanelState>(initialState);

  // Subscribe to the bridge once on mount; `init` drives the render.
  useEffect(() => {
    return initBridge({
      onInit: (payload: PluginIframeInitPayload) => {
        setLocale(payload.locale);
        setState({
          nodeType: payload.nodeType || '',
          config: payload.config ?? {},
          nodes: payload.nodes,
          ready: true,
        });
      },
      onConfigUpdated: (next: unknown) => {
        if (next && typeof next === 'object') {
          setState((prev) => ({ ...prev, config: next as Record<string, unknown> }));
        }
      },
    });
  }, []);

  const updateConfig = useCallback((next: Record<string, unknown>) => {
    setState((prev) => ({ ...prev, config: next }));
    notifyConfig(next);
  }, []);

  if (!state.ready) {
    return null;
  }

  if (state.nodeType === 'grpc:connect') {
    return (
      <ConnectPanel
        config={state.config as unknown as GrpcConnectConfig}
        onChange={(c) => updateConfig(c as unknown as Record<string, unknown>)}
      />
    );
  }
  if (state.nodeType === 'grpc:call') {
    return (
      <CallPanel
        config={state.config as unknown as GrpcCallConfig}
        nodes={state.nodes ?? []}
        onChange={(c) => updateConfig(c as unknown as Record<string, unknown>)}
      />
    );
  }
  if (state.nodeType === 'grpc:close') {
    return (
      <ClosePanel
        config={state.config as unknown as GrpcCloseConfig}
        nodes={state.nodes ?? []}
        onChange={(c) => updateConfig(c as unknown as Record<string, unknown>)}
      />
    );
  }

  return (
    <div className="panel">
      <div className="alert alert-amber">
        <span>Unknown gRPC node type: {state.nodeType || '(empty)'}</span>
      </div>
    </div>
  );
}

function renderPanel(): void {
  const container = document.getElementById('root');
  if (!container) return;
  createRoot(container).render(
    <StrictMode>
      <PanelApp />
    </StrictMode>,
  );
}

function main(): void {
  // 全局错误上报：面板 JS 运行时异常直接 post('error') 给宿主 →
  // PluginIframe 显示 "Plugin panel failed to load: <message>"，无需 devtools。
  window.addEventListener('error', (event) => {
    postError(event.message || String(event.error ?? 'unknown panel error'));
  });
  window.addEventListener('unhandledrejection', (event) => {
    const reason = event.reason;
    postError(reason instanceof Error ? reason.message : String(reason ?? 'unhandled rejection'));
  });
  // The built single-file HTML inlines this bundle as a classic `<script>`,
  // which vite places in <head> — #root may not exist yet, so wait for it.
  const container = document.getElementById('root');
  if (container) {
    renderPanel();
    return;
  }
  document.addEventListener('DOMContentLoaded', renderPanel);
}

main();
