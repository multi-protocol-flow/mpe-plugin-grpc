import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'node:path';

/**
 * Panel entry build → `dist-panel/panel.html` (config panels: connect/call/close).
 * A single entry per build produces one self-contained JS chunk (no shared
 * chunks, no `import` statements) which `scripts/inline.mjs` then inlines into
 * the HTML. See vite.config.viewer.ts for the viewer page.
 */
export default defineConfig({
  plugins: [react()],
  build: {
    target: 'es2018',
    minify: false,
    cssCodeSplit: false,
    assetsInlineLimit: 1000000000,
    chunkSizeWarningLimit: 2000,
    outDir: 'dist-panel',
    emptyOutDir: true,
    rollupOptions: {
      input: {
        panel: resolve(__dirname, 'panel.html'),
      },
      output: {
        entryFileNames: 'assets/[name].js',
        assetFileNames: 'assets/[name][extname]',
      },
    },
  },
});
