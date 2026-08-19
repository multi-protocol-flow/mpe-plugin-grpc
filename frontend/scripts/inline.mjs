/**
 * Post-build inliner: turns the two per-entry vite outputs into two fully
 * self-contained single-file HTML documents (dist/panel.html, dist/viewer.html).
 *
 * - Inlines every `<link rel="stylesheet" href="...">` into `<style>`
 * - Inlines every `<script ... src="...">` into a plain `<script>` block
 * - Removes the temporary build directories afterwards
 *
 * The attribute-free `<script>` tag is intentional: the plugin's Rust test
 * `panel_and_viewer_scripts_have_bridge_and_balanced_js` locates the first
 * literal `<script>` and checks its body for balanced brackets/strings.
 */
import { readFileSync, writeFileSync, existsSync, mkdirSync, rmSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = resolve(__dirname, '..');

function resolveAsset(htmlPath, href) {
  // vite emits relative hrefs like "./assets/panel-abc123.js" (or "/assets/...")
  const clean = href.replace(/^\.?\//, '');
  return resolve(dirname(htmlPath), clean);
}

function inlineFile(htmlPath, outPath) {
  let html = readFileSync(htmlPath, 'utf8');
  let inlined = 0;

  html = html.replace(
    /<link\b[^>]*\brel=["']stylesheet["'][^>]*\bhref=["']([^"']+)["'][^>]*>/g,
    (m, href) => {
      const cssPath = resolveAsset(htmlPath, href);
      if (!existsSync(cssPath)) return m;
      const css = readFileSync(cssPath, 'utf8');
      inlined += 1;
      return '<style>' + css + '</style>';
    },
  );

  html = html.replace(
    /<script\b[^>]*\bsrc=["']([^"']+)["'][^>]*><\/script>/g,
    (m, src) => {
      const jsPath = resolveAsset(htmlPath, src);
      if (!existsSync(jsPath)) return m;
      const js = readFileSync(jsPath, 'utf8');
      inlined += 1;
      return '<script>' + js + '</script>';
    },
  );

  // inline module scripts (e.g. preload polyfills) -> plain script
  html = html.replace(
    /<script\b[^>]*\btype=["']module["'][^>]*>([\s\S]*?)<\/script>/g,
    (m, code) => {
      if (!code || code.trim().length === 0) return m;
      inlined += 1;
      return '<script>' + code + '</script>';
    },
  );

  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, html);
  return inlined;
}

let failed = false;

const jobs = [
  { in: resolve(root, 'dist-panel', 'panel.html'), out: resolve(root, 'dist', 'panel.html') },
  { in: resolve(root, 'dist-viewer', 'viewer.html'), out: resolve(root, 'dist', 'viewer.html') },
];

for (const job of jobs) {
  if (!existsSync(job.in)) {
    console.error('inline.mjs: missing ' + job.in);
    failed = true;
    continue;
  }
  const n = inlineFile(job.in, job.out);
  console.log('inline.mjs: ' + job.out + ' inlined ' + n + ' asset(s)');
}

for (const dir of ['dist-panel', 'dist-viewer']) {
  const p = resolve(root, dir);
  if (existsSync(p)) {
    rmSync(p, { recursive: true, force: true });
    console.log('inline.mjs: removed ' + dir + '/');
  }
}

if (failed) process.exit(1);
