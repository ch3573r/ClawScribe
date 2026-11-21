import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const names = list => Object.fromEntries(list.split(' ').map(name => [name, name]));
const jsx = (type, props) => ({ type, props });
function nodes(root) {
  if (Array.isArray(root)) return root.flatMap(nodes);
  if (!root || typeof root !== 'object') return [];
  return [root, ...nodes(root.props?.children)];
}

test('reviewed actions edit and save offline with stable IDs and provenance', async () => {
  const harness = createHookHarness();
  const suggested = { id: 'saved-id', title: 'Bericht senden', owner: null, dueDate: null, done: false, sourceKey: 'abc123', legacyAction: null };
  let persisted = [];
  let open = true;
  const calls = [];
  const { ReviewedActions } = loadTsModule('src/components/MeetingDetails/ReviewedActions.tsx', {
    react: harness.react,
    'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': names('Button'),
    '@/components/ui/checkbox': names('Checkbox'),
    '@/components/ui/input': names('Input'),
    '@/components/ui/label': names('Label'),
    '@/components/ui/dialog': names('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'get_meeting_export_history') return [{ actionId: 'saved-id', destination: 'Planner', status: 'succeeded' }];
      if (command === 'get_reviewed_meeting_actions') return persisted;
      if (command === 'suggest_meeting_actions') return [{ ...suggested, legacyAction: { id: 'action-1', titleHash: '0'.repeat(16) } }];
      if (command === 'save_reviewed_meeting_actions') persisted = args.actions;
    } },
  });
  const render = () => nodes(harness.render(() => ReviewedActions({ open, onOpenChange: value => { open = value; }, meetingId: 'meeting', getMarkdown: async () => '## Aufgaben\n- Bericht senden' })));
  render();
  await flush();
  const item = (type, label) => render().find(node => node.type === type && (node.props.children === label || node.props.id === label));

  item('Button', 'Add action').props.onClick();
  const manualInput = render().find(node => node.type === 'Input' && node.props.id.startsWith('action-title-'));
  assert.match(manualInput.props.id, /^action-title-[0-9a-f-]{36}$/);
  manualInput.props.onChange({ target: { value: 'Manual follow-up' } });
  item('Button', 'Add summary suggestions').props.onClick();
  await flush();
  item('Input', 'action-title-saved-id').props.onChange({ target: { value: 'Bericht morgen senden' } });
  item('Input', 'action-owner-saved-id').props.onChange({ target: { value: 'Dana' } });
  item('Input', 'action-date-saved-id').props.onChange({ target: { value: '2026-11-30' } });
  item('Checkbox', 'action-done-saved-id').props.onCheckedChange(true);
  render().find(node => node.type === 'form').props.onSubmit({ preventDefault() {} });
  await flush();
  assert.equal(open, false);
  assert.equal(persisted[0].title, 'Manual follow-up');
  assert.equal(persisted[0].legacyAction, null);
  assert.equal(persisted[1].id, 'saved-id');
  assert.equal(persisted[1].sourceKey, 'abc123');
  assert.equal(persisted[1].owner, 'Dana');
  assert.equal(persisted[1].dueDate, '2026-11-30');
  assert.equal(persisted[1].done, true);
  render();
  open = true;
  render();
  await flush();
  assert.equal(item('Input', 'action-title-saved-id').props.value, 'Bericht morgen senden');
  assert.equal(calls.filter(([command]) => command === 'get_meeting_export_history').length, 2);
  assert.ok(render().some(node => node.type === 'p' && node.props.children?.includes('Planner')));
  assert.deepEqual(calls.filter(([command]) => command !== 'get_meeting_export_history').map(([command]) => command), ['get_reviewed_meeting_actions', 'suggest_meeting_actions', 'save_reviewed_meeting_actions', 'get_reviewed_meeting_actions']);
});
