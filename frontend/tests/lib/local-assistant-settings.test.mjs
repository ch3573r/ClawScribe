import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
function nodes(root) {
  if (Array.isArray(root)) return root.flatMap(nodes);
  return root?.props ? [root, ...nodes(root.props.children)] : [];
}

test('local assistant configuration appears only after a successful opt-in and disappears on revocation', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  let grant = false;
  let followThrough = false;
  const save = deferred();
  const serverTest = deferred();
  const calls = [];
  const command = String.raw`C:\Audio tools\Übung\ClawScribe.exe`;
  const { LocalMcpSettings } = loadTsModule('src/components/LocalMcpSettings.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'Button' }, '@/components/ui/label': { Label: 'Label' },
    '@/components/ui/select': { Select: 'Select', SelectTrigger: 'SelectTrigger', SelectValue: 'SelectValue', SelectContent: 'SelectContent', SelectItem: 'SelectItem' },
    '@/components/ui/page-section': { PageSection: 'PageSection' }, '@/components/ui/switch': { Switch: 'Switch' },
    '@/components/ui/textarea': { Textarea: 'Textarea' }, '@/components/ui/input': { Input: 'Input' }, 'lucide-react': {},
    '@tauri-apps/api/core': { invoke: async (name, args) => {
      calls.push(name);
      if (name === 'test_mcp_server') return serverTest.promise;
      if (name === 'set_mcp_access') {
        if (args.enabled) await save.promise;
        grant = args.enabled;
      }
      if (name === 'set_mcp_follow_through_access') followThrough = args.enabled;
      return { enabled: grant, follow_through: followThrough, command };
    } },
  });
  const find = kind => nodes(hooks.render(LocalMcpSettings)).find(node => node.type === kind);
  const findButton = label => nodes(hooks.render(LocalMcpSettings)).find(node =>
    node.type === 'Button' && node.props.children === label);
  assert.equal(find('Switch').props.disabled, true);
  await flush();
  assert.equal(find('Textarea'), undefined);
  assert.equal(findButton('Test MCP server'), undefined);
  assert.ok(nodes(hooks.render(LocalMcpSettings)).some(node => /may send what it reads to its own provider/.test(String(node.props.children))), 'the opt-in discloses provider sharing');
  const enable = find('Switch').props.onCheckedChange;
  enable(true);
  enable(true);
  assert.equal(find('Textarea'), undefined);
  assert.equal(calls.filter(name => name === 'set_mcp_access').length, 1);
  save.resolve();
  await flush();
  const config = JSON.parse(find('Textarea').props.value);
  assert.deepEqual(config.mcpServers.clawscribe, { command, args: ['--mcp-stdio'] });
  find('Select').props.onValueChange('codex');
  const toml = find('Textarea').props.value.split('\n');
  assert.deepEqual(toml, ['[mcp_servers.clawscribe]', 'command = ' + JSON.stringify(command), 'args = ["--mcp-stdio"]']);
  assert.equal(calls.filter(name => name === 'set_mcp_access').length, 1);
  find('Switch').props.onCheckedChange(false);
  await flush();
  assert.equal(find('Switch').props.checked, false);
  assert.equal(find('Textarea'), undefined);
  const followThroughSwitch = () => nodes(hooks.render(LocalMcpSettings)).find(node => node.props.id === 'mcp-follow-through');
  followThroughSwitch().props.onCheckedChange(true);
  await flush();
  assert.equal(find('Switch').props.checked, false);
  assert.equal(followThroughSwitch().props.checked, true);
  assert.ok(find('Textarea'));
  assert.ok(nodes(hooks.render(LocalMcpSettings)).some(node => String(node.props.children).includes('Copy meeting reference')));
  const testServer = findButton('Test MCP server').props.onClick;
  testServer();
  testServer();
  assert.equal(calls.filter(name => name === 'test_mcp_server').length, 1);
  assert.equal(findButton('Testing…').props.disabled, true);
  serverTest.resolve();
  await flush();
  assert.ok(nodes(hooks.render(LocalMcpSettings)).some(node => node.props.children === 'The local MCP server responded.'));
  followThroughSwitch().props.onCheckedChange(false);
  await flush();
  assert.equal(find('Textarea'), undefined);
});
