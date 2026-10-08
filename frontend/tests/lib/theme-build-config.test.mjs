import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';

const require = createRequire(import.meta.url);
const postcss = require('postcss');
const tailwind = require('tailwindcss');
const loadConfig = require('tailwindcss/loadConfig');
const { resolveDefaultConfigPath } = require('tailwindcss/lib/util/resolveConfigPath');
const { findConfigPath } = require('next/dist/lib/find-config');
const frontend = fileURLToPath(new URL('../../', import.meta.url));

test('the canonical configs retain application fonts, sidebar colors, animations and CSS prefixing', async () => {
  assert.equal(path.resolve(resolveDefaultConfigPath()), path.join(frontend, 'tailwind.config.ts'));
  const config = loadConfig(path.join(frontend, 'tailwind.config.ts'));
  assert.equal(config.theme.extend.fontFamily.sans[0], 'var(--font-app-sans)');
  assert.equal(config.theme.extend.fontFamily.mono[0], 'var(--font-plex-mono)');
  assert.match(config.theme.extend.colors.sidebar.active, /--sidebar-active/);
  assert.equal(config.theme.extend.animation['accordion-down'], 'accordion-down 0.2s ease-out');
  assert.equal(config.theme.extend.animation['accordion-up'], 'accordion-up 0.2s ease-out');
  assert.ok(config.plugins.includes(require('tailwindcss-animate')));
  const postcssPath = await findConfigPath(frontend, 'postcss');
  assert.equal(postcssPath, path.join(frontend, 'postcss.config.mjs'));
  const { default: postcssConfig } = await import(pathToFileURL(postcssPath));
  assert.deepEqual(Object.keys(postcssConfig.plugins), ['tailwindcss', 'autoprefixer']);
});

test('semantic status utilities compile with the active config, including foreground, border and opacity', async () => {
  const classes = ['success', 'warning', 'error', 'info'].flatMap(color => [
    `bg-${color}`, `text-${color}-foreground`, `border-${color}-border`,
    `bg-${color}/50`, `bg-${color}-foreground/25`, `border-${color}-border/30`,
  ]);
  const result = await postcss([tailwind({ ...loadConfig(path.join(frontend, 'tailwind.config.ts')), content: [{ raw: classes.join(' ') }] })])
    .process('@tailwind utilities;', { from: undefined });
  const selectors = new Set();
  result.root.walkRules(rule => selectors.add(rule.selector));
  for (const name of classes) {
    assert.ok(selectors.has(`.${name.replaceAll('/', '\\/')}`), `Missing semantic utility: ${name}`);
  }
  for (const color of ['success', 'warning', 'error']) {
    assert.ok(result.css.includes(`var(--theme-${color}-bg)`));
    assert.ok(result.css.includes(`var(--theme-${color}-fg)`));
  }
  assert.ok(result.css.includes('var(--theme-blue-50)'));
  assert.ok(result.css.includes('var(--theme-blue-700)'));
});
