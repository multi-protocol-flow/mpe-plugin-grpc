import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'node:path';

/**
 * Viewer entry build → `dist-viewer/viewer.html` (report viewer page).
 * Mirrors vite.config.ts but with the viewer HTML entry so each page becomes
 * a single self-contained chunk (see scripts/inline.mjs).
 */
export default defineConfig({
  plugins: [react()],
  build: {
    target: 'es2018',
    minify: false,
    cssCodeSplit: false,
    assetsInlineLimit: 1000000000,
    chunkSizeWarningLimit: 2000,
    outDir: 'dist-viewer',
    emptyOutDir: true,
    rollupOptions: {
      input: {
        viewer: resolve(__dirname, 'viewer.html'),
      },
      output: {
        entryFileNames: 'assets/[name].js',
        assetFileNames: 'assets/[name][extname]',
      },
    },
  },
});
