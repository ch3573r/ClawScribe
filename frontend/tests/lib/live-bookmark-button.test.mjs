import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function setup(props = {}) {
  const hooks = createHookHarness();
  const request = deferred();
  const calls = [];
  const { LiveBookmarkButton } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/MeetingBookmarks.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: (...args) => { calls.push(args); return request.promise; } },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    'lucide-react': { BookmarkPlus: 'BookmarkPlus', PencilLine: 'PencilLine' },
    sonner: { toast: { success() {}, error() {} } },
    '@/components/ui/button': { Button: 'button' },
    './ActionIcon': { ActionIcon: 'icon-button' },
    '@/components/ui/dialog': { Dialog: 'dialog', DialogContent: 'dialog-content', DialogHeader: 'dialog-header', DialogTitle: 'dialog-title', DialogDescription: 'dialog-description' },
    '@/components/ui/input': { Input: 'input' },
    '@/components/ui/switch': { Switch: 'switch' },
    '@/components/ui/accordion': { Accordion: 'accordion', AccordionContent: 'accordion-content', AccordionItem: 'accordion-item', AccordionTrigger: 'accordion-trigger' },
  });
  return { hooks, request, calls, render: () => hooks.render(() => LiveBookmarkButton({ sessionId: 'session-a', ...props })) };
}

test('live bookmark renders an accessible icon with the recording bar class', () => {
  const { render, hooks } = setup({ className: 'h-10 w-10 rounded-full' });
  const button = render();
  assert.equal(button.type, 'button');
  assert.match(button.props.className, /(?:^| )h-10(?: |$)/);
  assert.match(button.props.className, /(?:^| )w-10(?: |$)/);
  assert.match(button.props.className, /(?:^| )rounded-full(?: |$)/);
  assert.equal(button.props['aria-label'], 'Bookmark this moment');
  assert.equal(button.props.children.type, 'BookmarkPlus');
  assert.equal(button.props.children.props.size, 16);
  assert.equal(button.props.children.props.children, undefined);
  hooks.unmount();
});

test('live bookmark coalesces clicks while saving and shows a disabled spinner', async () => {
  const { render, hooks, calls, request } = setup();
  const button = render();
  const saving = button.props.onClick();
  await button.props.onClick();
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'add_meeting_bookmark');
  assert.equal(calls[0][1].meetingId, null);
  assert.equal(calls[0][1].seconds, null);
  assert.equal(calls[0][1].sessionId, 'session-a');
  assert.equal(calls[0][1].label, 'Review this');
  const busy = render();
  assert.equal(busy.props.disabled, true);
  assert.match(busy.props.children.props.className, /animate-spin/);
  assert.equal(busy.props.children.props.children, undefined);
  request.resolve();
  await saving;
  assert.equal(Boolean(render().props.disabled), false);
  hooks.unmount();
});

test('live bookmark respects the disabled prop', () => {
  const { render, hooks } = setup({ disabled: true });
  assert.equal(render().props.disabled, true);
  hooks.unmount();
});

function findNode(tree, predicate) {
  if (Array.isArray(tree)) return tree.map(node => findNode(node, predicate)).find(Boolean);
  if (!tree || typeof tree !== 'object') return undefined;
  return predicate(tree) ? tree : findNode(tree.props?.children, predicate);
}

test('my notes lists timestamps and saves text once for the current recording session', async () => {
  const hooks = createHookHarness();
  const request = deferred();
  const calls = [];
  let notes = [{ id: 'marker', seconds: 65, label: 'Review this' }];
  let consent = false;
  const { LiveRecordingNotes } = loadTsModule('src/components/LiveRecordingNotes.tsx', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async (...args) => {
      calls.push(args);
      if (args[0] === 'list_live_recording_bookmarks') return { bookmarks: notes, include_in_summary: consent };
      if (args[0] === 'set_live_summary_bookmarks') { consent = args[1].enabled; return; }
      return request.promise;
    } },
    'lucide-react': { PencilLine: 'PencilLine' },
    sonner: { toast: { success() {}, error() {} } },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/input': { Input: 'input' },
    '@/components/ui/switch': { Switch: 'switch' },
    '@/components/ui/popover': { Popover: 'popover', PopoverTrigger: 'popover-trigger', PopoverContent: 'popover-content' },
  });
  const renderNote = () => hooks.render(() => LiveRecordingNotes({ sessionId: 'session-a', disabled: false }));
  const find = predicate => findNode(renderNote(), predicate);
  find(node => node.type === 'popover').props.onOpenChange(true);
  renderNote();
  await flush();
  assert.deepEqual({ ...calls[0][1] }, { sessionId: 'session-a' });
  const row = find(node => node.type === 'li');
  assert.deepEqual(Array.from(row.props.children, child => child.props.children), ['00:01:05', 'Review this']);
  const toggle = find(node => node.type === 'switch');
  assert.equal(toggle.props.checked, false);
  await toggle.props.onCheckedChange(true);
  assert.deepEqual({ ...calls.find(([name]) => name === 'set_live_summary_bookmarks')[1] }, { sessionId: 'session-a', enabled: true });
  assert.equal(find(node => node.type === 'switch').props.checked, true);
  find(node => node.type === 'input').props.onChange({ target: { value: '  Follow up on the budget  ' } });
  const form = find(node => node.type === 'form');
  const saving = form.props.onSubmit({ preventDefault() {} });
  await form.props.onSubmit({ preventDefault() {} });
  const writes = calls.filter(([command]) => command === 'add_meeting_bookmark');
  assert.equal(writes.length, 1);
  assert.deepEqual({ ...writes[0][1] }, { meetingId: null, seconds: null, sessionId: 'session-a', label: 'Follow up on the budget' });
  notes = [{ id: 'note', seconds: 75, label: 'Follow up on the budget' }, ...notes];
  request.resolve();
  await saving;
  assert.equal(find(node => node.type === 'popover').props.open, true);
  assert.equal(find(node => node.type === 'input').props.value, '');
  await flush();
  assert.equal(find(node => node.type === 'li').props.children[1].props.children, 'Follow up on the budget');
  hooks.unmount();
});
