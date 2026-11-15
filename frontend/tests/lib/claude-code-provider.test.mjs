import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const element = tag => ({ children, collapsible, ...props }) => React.createElement(tag, props, children);
const controls = {
  '@/components/ui/button': { Button: element('button') },
  '@/components/ui/input': { Input: element('input') },
  '@/components/ui/label': { Label: element('label') },
  '@/components/ui/select': Object.fromEntries(['Select', 'SelectTrigger', 'SelectValue', 'SelectContent', 'SelectItem'].map(name => [name, element('div')])),
  '@/components/ui/accordion': Object.fromEntries(['Accordion', 'AccordionItem', 'AccordionTrigger', 'AccordionContent'].map(name => [name, element('div')])),
};
const card = (status, extra = {}, platform = 'windows') => {
  const { ClaudeCodeProviderCard } = loadTsModule('src/components/ClaudeCodeProviderCard.tsx', {
    ...controls, '@/lib/openExternal': { openExternal() {} },
    '@tauri-apps/plugin-os': { platform: () => platform },
  });
  return renderToStaticMarkup(React.createElement(ClaudeCodeProviderCard, {
    status, busy: false, programPath: '', model: 'sonnet', onProgramPathChange() {}, onModelChange() {}, onCheck() {}, ...extra,
  }));
};

test('Claude Code card explains installation and provides copyable commands without login UI', () => {
  const html = card({ state: 'not_installed' });
  assert.match(html, /Uses the Claude Code you installed and signed in to\. Your Claude plan&#x27;s limits apply\./);
  assert.match(html, /winget install Anthropic\.ClaudeCode/);
  assert.match(html, /readOnly/);
  assert.match(html, /Open install guide/);
  assert.match(html, /Check again/);
  assert.doesNotMatch(html, /Sign in with|API key|<img/);
});

test('Claude Code installation command is Windows-only, with the guide on every platform', () => {
  for (const platform of ['macos', 'linux', null]) {
    const html = card({ state: 'not_installed' }, {}, platform);
    assert.doesNotMatch(html, /winget|Install command/);
    assert.match(html, /Open install guide/);
  }
});

test('Claude Code signed-out and ready states expose only applicable information', () => {
  assert.match(card({ state: 'not_signed_in' }), /claude auth login/);
  const ready = card({ state: 'ready', version: '2.1.0', plan: 'Max' });
  assert.match(ready, /2\.1\.0/);
  assert.match(ready, /Max/);
  for (const [value, label] of [['sonnet', 'Sonnet'], ['opus', 'Opus'], ['haiku', 'Haiku']]) {
    assert.match(ready, new RegExp(`value="${value}">${label}</div>`));
  }
  assert.match(ready, /Advanced/);
  assert.match(ready, /Program path/);
  assert.doesNotMatch(ready, /winget install|claude auth login/);
  assert.match(card({ state: 'not_installed' }), /Program path/, 'a broken path can be repaired before reaching ready');
});

test('Claude Code loading, disabled and error states are visible and actionable', () => {
  assert.match(card(null, { busy: true }), /Checking Claude Code/);
  assert.match(card(null, { busy: true }), /disabled/);
  assert.match(card({ state: 'error', message: 'Claude Code timed out' }), /Claude Code timed out/);
});

test('Claude Code settings ignore late checks and never expose raw invoke failures', async () => {
  const hooks = createHookHarness();
  const pending = deferred();
  const calls = [];
  const { useClaudeCodeSettings } = loadTsModule('src/components/ClaudeCodeProviderCard.tsx', {
    ...controls, react: hooks.react, '@/lib/openExternal': { openExternal() {} },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'claude_code_get_config') return { programPath: null };
      return pending.promise;
    } },
  });
  let active = true;
  const render = () => hooks.render(() => useClaudeCodeSettings(active));
  render(); await flush();
  assert.equal(render().busy, true);
  active = false; render();
  pending.resolve({ state: 'ready', version: '2.1.0', plan: null }); await flush();
  assert.equal(render().status, null);
  hooks.unmount();
  assert.equal(calls.filter(([name]) => /login|install|api_key/.test(name)).length, 0);
});

test('Claude Code checks use the draft path, coalesce clicks and save only when requested', async () => {
  const hooks = createHookHarness();
  const calls = [];
  let fail = false;
  const { useClaudeCodeSettings } = loadTsModule('src/components/ClaudeCodeProviderCard.tsx', {
    ...controls, react: hooks.react, '@/lib/openExternal': { openExternal() {} },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'claude_code_get_config') return { programPath: null };
      if (fail) throw new Error('synthetic private CLI output');
      return { state: 'ready', version: '2.1.0', plan: 'Pro' };
    } },
  });
  const render = () => hooks.render(() => useClaudeCodeSettings(true));
  render(); await flush();
  render().setProgramPath('  custom program.cmd  ');
  const settings = render();
  const count = calls.length;
  await Promise.all([settings.check(), settings.check()]);
  assert.equal(calls.length, count + 1);
  assert.equal(calls.at(-1)[1].config.programPath, 'custom program.cmd');
  assert.equal(calls.some(([command]) => command === 'claude_code_save_config'), false);
  await render().save();
  assert.equal(calls.at(-1)[0], 'claude_code_save_config');
  fail = true;
  await render().check();
  assert.equal(render().status.state, 'error');
  assert.doesNotMatch(render().status.message, /synthetic private/);
  hooks.unmount();
});

test('summary settings and meeting details never request an API key for Claude Code', async () => {
  for (const provider of ['claude-code', 'claude']) {
    for (const surface of ['settings', 'meeting']) {
      const hooks = createHookHarness();
      const calls = [];
      const mocks = {
        react: hooks.react,
        '@tauri-apps/api/core': { invoke: async command => {
          calls.push(command);
          if (command === 'api_get_model_config') return { provider, model: provider === 'claude-code' ? 'sonnet' : 'api-model', whisperModel: 'large-v3', apiKey: null };
          if (command === 'api_get_api_key') return 'synthetic-placeholder';
          throw new Error('Unexpected command');
        } },
        '@tauri-apps/api/event': { listen: async () => () => {}, emit: async () => {} },
        '@/components/ModelSettingsModal': { ModelSettingsModal: 'model-settings' },
        '@/components/SummaryLanguageSettings': { SummaryLanguageSettings: 'summary-language' },
        '@/contexts/ConfigContext': { useConfig: () => ({ isAutoSummary: false, toggleIsAutoSummary() {} }) },
        './ui/switch': { Switch: 'switch' },
        '@/lib/analytics': { __esModule: true, default: {} },
        sonner: { toast: { error() {}, success() {} } },
      };
      if (surface === 'settings') {
        const { SummaryModelSettings } = loadTsModule('src/components/SummaryModelSettings.tsx', mocks);
        hooks.render(() => SummaryModelSettings({}));
      } else {
        const { useModelConfiguration } = loadTsModule('src/hooks/meeting-details/useModelConfiguration.ts', mocks);
        hooks.render(() => useModelConfiguration({ serverAddress: null }));
      }
      await flush();
      assert.equal(calls.includes('api_get_api_key'), provider === 'claude');
      hooks.unmount();
    }
  }
});
