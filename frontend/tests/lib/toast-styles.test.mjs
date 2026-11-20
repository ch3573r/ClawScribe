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

// sonner's base close button uses --gray4 / --gray5 / --gray2 and --gray12, which
// stay light in the light theme. Default and loading toasts take app tokens.
test('the close button of a default toast uses app tokens, including on hover', () => {
  assert.match(sonner, /\[data-sonner-toast\]\[data-styled='true'\] \[data-close-button\] \{[^}]*border: 1px solid var\(--gray4\)/, 'the grey to beat still exists in sonner');
  const rule = selector => {
    const at = css.indexOf(selector);
    assert.ok(at > 0, `missing rule: ${selector}`);
    return css.slice(at, css.indexOf('}', at));
  };
  const plain = ":root [data-sonner-toast][data-styled='true']:not([data-type='success']):not([data-type='info']):not([data-type='warning']):not([data-type='error'])";
  const base = rule(plain + ' [data-close-button] {');
  assert.match(base, /background: var\(--normal-bg\)/);
  assert.match(base, /border-color: var\(--normal-border\)/);
  assert.match(base, /color: var\(--normal-text\)/);
  const hover = rule(plain + ':hover [data-close-button]:hover {');
  assert.match(hover, /background: var\(--normal-bg-hover\)/);
  assert.match(hover, /border-color: var\(--normal-border-hover\)/);
  assert.doesNotMatch(base + hover, /--gray/, 'no sonner grey is used');
});
