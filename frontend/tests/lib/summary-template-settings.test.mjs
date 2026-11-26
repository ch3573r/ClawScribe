import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

function editor(t, { personal, bundled, search }) {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const selected = { id: 'fixture', name: 'Example template', personal, bundled };
  const confirmations = [], removals = [];
  let confirmed = false;
  let removalError = false;
  const stub = (...names) => Object.fromEntries(names.map(name => [name, name]));
  const jsx = (type, props) => ({ type, props });
  const previousWindow = globalThis.window;
  if (search) globalThis.window = { location: { search } };
  const { SummaryTemplateSettings } = loadTsModule('src/components/SummaryTemplateSettings.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'api_get_template_editor') return { name: selected.name, description: 'Example', sections: [] };
      if (command === 'api_remove_template') {
        removals.push(args.templateId);
        if (removalError) throw new Error('Could not write');
        return;
      }
      assert.fail(`Unexpected command ${command}`);
    } },
    '@tauri-apps/plugin-dialog': { confirm: async (_message, options) => { confirmations.push(options.title); return confirmed; } },
    'sonner': { toast: { success() {} } },
    '@/components/ui/button': stub('Button'),
    '@/components/ui/input': stub('Input'),
    '@/components/ui/textarea': stub('Textarea'),
    '@/components/ui/dialog': stub('Dialog', 'DialogContent', 'DialogDescription', 'DialogHeader', 'DialogTitle'),
    '@/components/ui/select': stub('Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue'),
    '@/components/ui/settings-layout': stub('SettingsFooter', 'SettingsRow'),
    '@/components/settings/settings-parts': { ROW_SELECT: '' },
    '@/components/settings/summary-provider-parts': { ...stub('SaveMark'), withError: () => null },
    '@/hooks/meeting-details/useTemplates': { useTemplates: () => ({ availableTemplates: [selected], defaultTemplate: selected.id }) },
  });
  globalThis.window = previousWindow;
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node && typeof node === 'object'
    ? [node, ...nodes(node.props.children), ...nodes(node.props.actions)] : [];
  const find = predicate => nodes(hooks.render(SummaryTemplateSettings)).find(predicate);
  const button = label => find(node => node.type === 'Button' && node.props.children === label);
  const open = async () => {
    button('Edit templates').props.onClick();
    find(node => node.type === 'Select' && node.props.value === '__clawscribe-empty-template-selection__').props.onValueChange(selected.id);
    await flush();
  };
  const click = async label => { button(label).props.onClick(); await flush(); };
  return {
    open, find, click, button, confirmations, removals,
    confirm: () => { confirmed = true; },
    failRemoval: () => { removalError = true; },
  };
}

test('the manage templates deep link opens the editor', t => {
  const ui = editor(t, { personal: false, bundled: true, search: '?tab=summary&manageTemplates=1' });
  ui.find(() => false);
  assert.equal(ui.find(node => node.type === 'Dialog').props.open, true);
});

test('unmodified bundled templates have no removal action', async t => {
  const ui = editor(t, { personal: false, bundled: true });
  await ui.open();
  assert.equal(ui.button('Reset template'), undefined);
  assert.equal(ui.button('Delete template'), undefined);
});

test('reset of an override requires confirmation', async t => {
  const ui = editor(t, { personal: true, bundled: true });
  await ui.open();
  await ui.click('Reset template');
  assert.deepEqual(ui.confirmations, ['Reset template']);
  assert.deepEqual(ui.removals, []);
  ui.confirm();
  await ui.click('Reset template');
  assert.deepEqual(ui.removals, ['fixture']);
});

test('failed deletion keeps the personal template open for retry', async t => {
  const ui = editor(t, { personal: true, bundled: false });
  await ui.open();
  ui.confirm();
  ui.failRemoval();
  await ui.click('Delete template');
  assert.deepEqual(ui.removals, ['fixture']);
  assert.ok(ui.button('Delete template'));
  assert.equal(ui.find(node => node.props.role === 'alert').props.children, 'Could not delete the template. Try again.');
});
