/**
 * Report-viewer entry: receives `init` with `node_report`, subscribes to the
 * realtime `stream` bridge messages, and renders the ViewerApp.
 */
import { StrictMode, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import './styles.css';
import type { NodeReport, PluginIframeInitPayload, PluginStreamMessage } from './types';
import { initBridge, postError } from './bridge';
import { setLocale } from './i18n';
import { ViewerApp } from './panels/viewer/ViewerApp';

function ViewerRoot() {
  const [report, setReport] = useState<NodeReport | null>(null);
  const [streams, setStreams] = useState<PluginStreamMessage[]>([]);

  useEffect(() => {
    return initBridge({
      onInit: (payload: PluginIframeInitPayload) => {
        setLocale(payload.locale);
        setReport(payload.node_report ?? null);
      },
      onStream: (payload: PluginStreamMessage) => {
        setStreams((prev) => [...prev, payload]);
      },
    });
  }, []);

  return <ViewerApp report={report} streams={streams} />;
}

function renderViewer(): void {
  const container = document.getElementById('root');
  if (!container) return;
  createRoot(container).render(
    <StrictMode>
      <ViewerRoot />
    </StrictMode>,
  );
}

function main(): void {
  // 全局错误上报：viewer JS 异常经 post('error') 回宿主（宿主侧暂可忽略，
  // 便于未来报告页直接显示错误原因）。
  window.addEventListener('error', (event) => {
    postError(event.message || String(event.error ?? 'unknown viewer error'));
  });
  window.addEventListener('unhandledrejection', (event) => {
    const reason = event.reason;
    postError(reason instanceof Error ? reason.message : String(reason ?? 'unhandled rejection'));
  });
  // The built single-file HTML inlines this bundle as a classic `<script>`,
  // which vite places in <head> — #root may not exist yet, so wait for it.
  const container = document.getElementById('root');
  if (container) {
    renderViewer();
    return;
  }
  document.addEventListener('DOMContentLoaded', renderViewer);
}

main();
