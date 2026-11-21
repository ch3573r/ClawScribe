import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

test('action navigation keeps the meeting and action identities separate from route directives', () => {
  const { meetingActionUrl } = loadTsModule('src/lib/meeting-follow-through.ts');
  const url = new URL(meetingActionUrl('meeting&archiveAction=files', 'action#one'), 'https://example.com');
  assert.equal(url.pathname, '/meeting-details');
  assert.deepEqual(Object.fromEntries(url.searchParams), {
    id: 'meeting&archiveAction=files', from: 'archive', archiveAction: 'actions', actionId: 'action#one',
  });
});

test('series destination shortcuts require an explicit launch selection', () => {
  const { meetingActionUrl } = loadTsModule('src/lib/meeting-follow-through.ts');
  const url = new URL(meetingActionUrl('meeting', 'action'), 'https://example.com');
  assert.equal(url.searchParams.has('destination'), false);
});

const jsx = (type, props) => ({ type, props });
const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node && typeof node === 'object'
  ? [node, ...nodes(node.props?.children)] : [];
const primitives = names => Object.fromEntries(names.split(' ').map(name => [name, name]));

function emailReview(close) {
  const hooks = createHookHarness();
  const calls = [];
  const { FollowUpEmail } = loadTsModule('src/components/MeetingDetails/FollowUpEmail.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, { ...args }]);
      if (command === 'get_reviewed_follow_up_draft') return {
        subject: 'Follow-up', body: 'Review the plan',
        recipients: [{ name: 'Dana', email: 'dana@example.com' }, { name: 'Pat', email: 'pat@example.com' }],
      };
    } },
    '@/components/ui/button': primitives('Button'),
    '@/components/ui/checkbox': primitives('Checkbox'),
    '@/components/ui/input': primitives('Input'),
    '@/components/ui/label': primitives('Label'),
    '@/components/ui/textarea': primitives('Textarea'),
    '@/components/ui/dialog': primitives('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
  });
  const render = () => nodes(hooks.render(() => FollowUpEmail({ meetingId: 'selected-meeting', close })));
  return { hooks, calls, render, find: (type, id) => render().find(node => node.type === type && (!id || node.props.id === id)) };
}

test('canceling a reviewed draft makes no mail client request', async () => {
  let closed = false;
  const review = emailReview(() => { closed = true; });
  try {
    review.render();
    await flush();
    assert.ok(review.render().filter(node => node.type === 'Checkbox').every(node => node.props.checked === false));
    review.find('Dialog').props.onOpenChange(false);
    assert.equal(closed, true);
    assert.deepEqual(review.calls.map(([command]) => command), ['get_reviewed_follow_up_draft']);
  } finally { review.hooks.unmount(); }
});

test('the mail client receives only the checked attendee and the edited draft on submit', async () => {
  const review = emailReview(() => {});
  try {
    review.render();
    await flush();
    review.find('Checkbox', 'follow-up-recipient-0').props.onCheckedChange(true);
    review.find('Textarea', 'follow-up-body').props.onChange({ target: { value: 'Updated reviewed note.' } });
    assert.equal(review.calls.length, 1);
    review.find('form').props.onSubmit({ preventDefault() {} });
    await flush();
    assert.equal(review.calls[1][0], 'open_reviewed_follow_up_draft');
    assert.deepEqual({ ...review.calls[1][1], recipients: [...review.calls[1][1].recipients] }, {
      meetingId: 'selected-meeting', recipients: ['dana@example.com'], subject: 'Follow-up', body: 'Updated reviewed note.',
    });
  } finally { review.hooks.unmount(); }
});

test('Planner preview sends selected action text for AI polishing only after an explicit request', async () => {
  const hooks = createHookHarness();
  const calls = [];
  const items = [
    { localId: 'first', title: 'Send the report', owner: 'Dana', dueDate: null },
    { localId: 'second', title: 'Check the plan', owner: null, dueDate: null },
  ];
  const { PlannerExportPreview } = loadTsModule('src/components/MeetingDetails/PlannerExportPreview.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': {}, sonner: { toast: { info() {} } },
    '@/contexts/ConfigContext': { useConfig: () => ({ modelConfig: { provider: 'openai', model: 'synthetic' } }) },
    '@/lib/exportDestinations': { getExportDestinations: () => ({ plannerAiPolish: true }) },
    '@/services/microsoftExportService': {
      microsoftExportService: {
        previewPlannerTasks: async () => ({ items, candidates: [], suggestionCount: 0 }),
        listBuckets: async () => [{ id: 'bucket', name: 'Bucket' }],
        polishPlannerTasks: async (provider, model, actions) => {
          calls.push([provider, model, JSON.parse(JSON.stringify(actions))]);
          return [{ title: 'Send reviewed report', details: 'Check the attachment.' }];
        },
      },
    },
    '@/components/ui/button': primitives('Button'),
    '@/components/ui/checkbox': primitives('Checkbox'),
    '@/components/ui/input': primitives('Input'),
    '@/components/ui/textarea': primitives('Textarea'),
    '@/components/ui/select': primitives('Select SelectContent SelectItem SelectTrigger SelectValue'),
    '@/components/ui/dialog': primitives('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
  });
  const props = {
    open: true, onOpenChange() {}, meetingId: 'meeting', meetingTitle: 'Meeting',
    planId: 'plan', defaultBucketId: 'bucket', getMarkdown: async () => '', onReport() {},
  };
  const render = () => nodes(hooks.render(() => PlannerExportPreview(props)));
  try {
    render();
    await flush();
    assert.equal(calls.length, 0);
    render().filter(node => node.type === 'Checkbox')[1].props.onCheckedChange(false);
    render().find(node => node.type === 'Button' && node.props.children === 'Polish selected with AI').props.onClick();
    await flush();
    assert.deepEqual(calls, [['openai', 'synthetic', [{ title: 'Send the report', owner: 'Dana', dueDate: null }]]]);
    assert.deepEqual(render().filter(node => node.type === 'Input').map(node => node.props.value), [
      'Send reviewed report', 'Check the plan',
    ]);
  } finally { hooks.unmount(); }
});
