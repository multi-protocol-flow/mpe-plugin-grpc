# gRPC plugin frontend

Self-contained React + Vite + TypeScript frontend for the gRPC sidecar plugin.
Two single-file HTML builds are embedded into the plugin binary via
`include_str!` (see `plugins/grpc/src/lib.rs`):

| entry         | output               | purpose                                          |
|---------------|----------------------|--------------------------------------------------|
| `src/panel.tsx`  | `dist/panel.html`  | connect / call / close config panels (dispatched by `init.nodeType`) |
| `src/viewer.tsx` | `dist/viewer.html` | report viewer (node_report + realtime `stream` messages) |

## Build

```bash
cd plugins/grpc/frontend
npm install
npm run build        # vite build (panel) + vite build (viewer) + inline assets
npx tsc --noEmit     # strict typecheck
```

`npm run build` produces the two fully self-contained HTML files in `dist/`
(all JS/CSS inlined into a single attribute-free `<script>`/`<style>` per file —
required by the plugin's Rust test `panel_and_viewer_scripts_have_bridge_and_balanced_js`).

**IMPORTANT:** after changing anything under `frontend/`, you MUST re-run
`npm run build` and then re-run the plugin tests (`cd plugins/grpc && cargo test`)
— the Rust tests embed the built HTML (`include_str!`), so stale `dist/` files
cause false failures.

## Notes / constraints

- Only the host's `src/lib/protoFieldParser.ts` and `src/lib/grpcurlGenerator.ts`
  are imported (copied into `src/lib/`); everything else is ported locally.
- No host imports (`@/components/*`, `@/stores/*`, `@/services/*`, `@/i18n/*`),
  no Tailwind, no UI framework — dependencies are react/react-dom/typescript/vite
  /@vitejs/plugin-react only.
- The built bundle runs as a **classic** `<script>` (vite places it in `<head>`;
  `main()` waits for `#root` — see `panel.tsx`/`viewer.tsx`).
- The Rust balance checker cannot parse regex literals or quoted template
  literals; `protoFieldParser.ts` / `grpcurlGenerator.ts` therefore build the
  few brace-containing regexes via `new RegExp('…')` (semantics identical) and
  `shellQuote` uses string concatenation.
