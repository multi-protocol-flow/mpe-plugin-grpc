/**
 * postMessage bridge between the plugin iframe and the host.
 *
 * Protocol matches the host's `src/types/plugin-iframe.ts`:
 *
 *   host -> iframe : `init` {config, variables, host_api, nodeType, locale, nodes?, node_report?}
 *                    `configUpdated` {config}
 *                    `actionResult` {requestId, result, error?}
 *                    `stream` {call_id, kind: 'message'|'error', data}   (viewer only)
 *   iframe -> host : `ready` {}, `configChanged` {config}, `resize` {height},
 *                    `error` {message}, `requestAction` {requestId, action, params}
 *
 * `requestAction` whitelist: "testConnection", "openFileDialog", "uiCall".
 * `uiCall(method, payload)` correlates responses by requestId; rejects when
 * the host reports an `error` field. `configChanged` is debounced 300ms;
 * panel height changes are reported via ResizeObserver.
 */

import type {
  PluginIframeInitPayload,
  PluginStreamMessage,
} from './types';

export interface BridgeOptions {
  onInit: (payload: PluginIframeInitPayload) => void;
  onConfigUpdated?: (config: unknown) => void;
  onStream?: (payload: PluginStreamMessage) => void;
}

interface PendingAction {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
  /** action timeout handle (browser `setTimeout` returns number) */
  timer: number;
}

const ACTION_TIMEOUT_MS = 30000;
const CONFIG_DEBOUNCE_MS = 300;

let locale = 'en-US';
let nodeType = '';
let nodes: PluginIframeInitPayload['nodes'] = undefined;
let initConfig: Record<string, unknown> = {};

const pendingActions = new Map<string, PendingAction>();

// --- outbound -------------------------------------------------------------

function post(type: string, payload: Record<string, unknown> = {}): void {
  try {
    window.parent.postMessage({ type, payload }, '*');
  } catch (err) {
    // Never let a bridge failure break the panel.
    console.error('[grpc] failed to post message', type, err);
  }
}

/** iframe -> host: `ready` — panel initialized (sent after `init`). */
function postReady(): void {
  post('ready');
}

/** iframe -> host: `resize` — panel height changed. */
function postResize(height: number): void {
  post('resize', { height });
}

/** iframe -> host: `error` — panel internal error report. */
function postError(message: string): void {
  post('error', { message });
}

/** iframe -> host: `requestAction` — ask the host for a whitelisted action. */
function requestAction(action: string, params: unknown): Promise<unknown> {
  return new Promise<unknown>((resolve, reject) => {
    const requestId =
      typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
        ? crypto.randomUUID()
        : String(Date.now()) + '-' + Math.random().toString(36).slice(2);
    const timer = setTimeout(() => {
      pendingActions.delete(requestId);
      reject(new Error('requestAction timed out: ' + action));
    }, ACTION_TIMEOUT_MS);
    pendingActions.set(requestId, { resolve, reject, timer });
    post('requestAction', { requestId, action, params });
  });
}

/**
 * Call a design-time plugin sub-method (`grpc.discover`, `grpc.validate`, ...).
 * The host relays the request to the plugin's `ui_call` handler and returns
 * the payload via `actionResult`.
 */
export function uiCall(method: string, payload: unknown): Promise<unknown> {
  return requestAction('uiCall', { nodeType, method, payload });
}

/**
 * Ask the host to open a native file/directory picker.
 * Returns `string | string[] | null` (null when cancelled).
 */
export function openFileDialog(params: {
  multiple?: boolean;
  filters?: Array<{ name: string; extensions: string[] }>;
  directory?: boolean;
}): Promise<string | string[] | null> {
  return requestAction('openFileDialog', params) as Promise<string | string[] | null>;
}

// --- configChanged (debounced) ---------------------------------------------

let configTimer: number | null = null;

/** iframe -> host: `configChanged` — full config snapshot, debounced 300ms. */
export function notifyConfig(config: unknown): void {
  clearTimeout(configTimer ?? undefined);
  configTimer = setTimeout(() => {
    configTimer = null;
    post('configChanged', { config });
  }, CONFIG_DEBOUNCE_MS);
}

/** Immediately flush any pending configChanged (unmount safety). */
export function flushConfig(): void {
  if (configTimer !== null) {
    clearTimeout(configTimer);
    configTimer = null;
  }
}

// --- inbound ---------------------------------------------------------------

/**
 * Install the window message listener. Call once at startup; `ready` is
 * posted as soon as `init` arrives and the handlers have run. Returns an
 * unsubscribe function (removes the listener + resize observer).
 */
export function initBridge(options: BridgeOptions): () => void {
  let registered = false;
  if (registered) return () => undefined;
  registered = true;

  const handleMessage = (event: MessageEvent) => {
    const data = event.data;
    if (!data || typeof data !== 'object' || typeof data.type !== 'string') {
      return;
    }
    const payload = data.payload && typeof data.payload === 'object' ? data.payload : {};

    switch (data.type) {
      case 'init': {
        const initPayload = payload as PluginIframeInitPayload;
        locale = typeof initPayload.locale === 'string' ? initPayload.locale : 'en-US';
        nodeType = typeof initPayload.nodeType === 'string' ? initPayload.nodeType : '';
        nodes = Array.isArray(initPayload.nodes) ? initPayload.nodes : undefined;
        initConfig =
          initPayload.config && typeof initPayload.config === 'object'
            ? initPayload.config
            : {};
        options.onInit(initPayload);
        postReady();
        break;
      }
      case 'configUpdated': {
        if (options.onConfigUpdated) {
          options.onConfigUpdated(payload.config);
        }
        break;
      }
      case 'actionResult': {
        const requestId =
          typeof payload.requestId === 'string' ? payload.requestId : '';
        const pending = pendingActions.get(requestId);
        if (!pending) return;
        pendingActions.delete(requestId);
        clearTimeout(pending.timer);
        if (typeof payload.error === 'string' && payload.error.length > 0) {
          pending.reject(new Error(payload.error));
        } else {
          pending.resolve(payload.result);
        }
        break;
      }
      case 'stream': {
        if (options.onStream) {
          const streamPayload = payload as unknown as PluginStreamMessage;
          options.onStream(streamPayload);
        }
        break;
      }
      default:
        break;
    }
  };

  window.addEventListener('message', handleMessage);

  // Panel auto-resize: report document.body height changes to the host.
  let observer: ResizeObserver | null = null;
  if (typeof ResizeObserver !== 'undefined') {
    observer = new ResizeObserver(() => {
      const height = Math.ceil(document.body.scrollHeight);
      if (Number.isFinite(height) && height > 0) {
        postResize(height);
      }
    });
    observer.observe(document.body);
  }

  return () => {
    window.removeEventListener('message', handleMessage);
    if (observer) {
      observer.disconnect();
      observer = null;
    }
    registered = false;
  };
}

// --- accessors -------------------------------------------------------------

export function getLocale(): string {
  return locale;
}

export function getNodeType(): string {
  return nodeType;
}

export function getNodes(): PluginIframeInitPayload['nodes'] {
  return nodes;
}

export function getInitConfig(): Record<string, unknown> {
  return initConfig;
}

export { postReady, postResize, postError };
