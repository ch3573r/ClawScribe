import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const css = fs.readFileSync(new URL('../../src/app/globals.css', import.meta.url), 'utf8');
const sonner = fs.readFileSync(new URL('../../node_modules/sonner/dist/styles.css', import.meta.url), 'utf8');

// Sonner's stylesheet is appended to <head> after ours, so equal specificity
// would let its fixed light/dark palettes win. Our overrides must outrank them.
test('the toast colour overrides outrank sonner whatever the stylesheet order', () => {
  const rule = css.indexOf('[data-sonner-toaster][data-sonner-theme] {');
  assert.ok(rule > 0, 'the override block exists');
  assert.equal(css.slice(rule - 6, rule), ':root ');
  assert.match(sonner, /\[data-sonner-toaster\]\[data-sonner-theme='light'\] \{\s+--normal-bg: #fff;/, 'the selector to beat still exists in sonner');
  assert.ok(css.includes(":root [data-rich-colors='true'][data-sonner-toast][data-type='default']"));
});
