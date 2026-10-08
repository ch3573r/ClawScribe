import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred } from './hook-harness.mjs';

function setup(props = {}) {
  const hooks = createHookHarness();
  const request = deferred();
  const calls = [];
  const { LiveBookmarkButton } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/MeetingBookmarks.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: (...args) => { calls.push(args); return request.promise; } },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    'lucide-react': { BookmarkPlus: 'BookmarkPlus' },
    sonner: { toast: { success() {}, error() {} } },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/input': { Input: 'input' },
    '@/components/ui/accordion': { Accordion: 'accordion', AccordionContent: 'accordion-content', AccordionItem: 'accordion-item', AccordionTrigger: 'accordion-trigger' },
  });
  return { hooks, request, calls, render: () => hooks.render(() => LiveBookmarkButton(props)) };
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
