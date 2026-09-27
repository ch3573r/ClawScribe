import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

test('tag picker adds suggestions and pasted tags, removes chips and saves an empty list', async () => {
  const hooks = createHookHarness(), calls = [];
  const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
  const jsx = (type, props) => ({ type, props });
  const nodes = root => Array.isArray(root) ? root.flatMap(nodes) : root && typeof root === 'object'
    ? [root, ...nodes(root.props?.children)] : [];
  const text = root => typeof root === 'string' ? root : Array.isArray(root) ? root.map(text).join('') : text(root?.props?.children ?? '');
  const { ProjectTags } = loadTsModule('src/components/MeetingDetails/ProjectTags.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({
      projectTags: [{ meeting_id: 'other', tag: 'Project Atlas' }],
      projectTagsLoading: false, projectTagsError: null, refreshProjectTags: async () => {},
    }) },
    '@/components/ui/button': names('Button'),
    '@/components/ui/dialog': names('Dialog DialogContent DialogHeader DialogTitle DialogDescription DialogFooter'),
    '@/components/ui/popover': names('Popover PopoverTrigger PopoverContent'),
    '@/components/ui/command': names('Command CommandInput CommandList CommandItem CommandEmpty'),
    '@tauri-apps/api/core': { invoke: async (command, args) => calls.push({ command, args }) },
    sonner: { toast: { success() {}, error(message) { assert.fail(message); } } },
  });
  const render = () => hooks.render(() => ProjectTags({ meetingId: 'meeting' }));
  const find = (type, predicate = () => true) => nodes(render()).find(node => node.type === type && predicate(node));
  find('Button', node => text(node) === 'Project tags').props.onClick();
  find('CommandItem', node => text(node) === 'Project Atlas').props.onSelect();
  find('CommandInput').props.onPaste({ preventDefault() {}, clipboardData: { getData: () => 'a, b, a' } });
  await find('Button', node => text(node) === 'Save tags').props.onClick();
  assert.deepEqual(Array.from(calls[0].args.tags), ['Project Atlas', 'a', 'b']);
  find('CommandInput').props.onKeyDown({ key: 'Backspace', nativeEvent: {}, preventDefault() {} });
  assert.equal(find('Button', node => node.props['aria-label'] === 'Remove b'), undefined);
  find('Button', node => node.props['aria-label'] === 'Remove a').props.onClick();
  find('Button', node => node.props['aria-label'] === 'Remove Project Atlas').props.onClick();
  find('CommandInput').props.onValueChange('x'.repeat(61));
  find('CommandItem').props.onSelect();
  // The first item is the Create action because the long query matches no saved tag.
  assert.match(text(render()), /60 characters or fewer/);
  await find('Button', node => text(node) === 'Save tags').props.onClick();
  assert.equal(calls.at(-1).command, 'set_meeting_tags');
  assert.deepEqual(Array.from(calls.at(-1).args.tags), []);
  hooks.unmount();
});
