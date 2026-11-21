import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const names = list => Object.fromEntries(list.split(' ').map(name => [name, name]));
const jsx = (type, props) => ({ type, props });
function nodes(root) {
  if (!root || typeof root !== 'object') return [];
  if (Array.isArray(root)) return root.flatMap(nodes);
  return [root, ...nodes(root.props?.children)];
}
const text = root => typeof root === 'string' ? root : Array.isArray(root) ? root.map(text).join('') : text(root?.props?.children ?? '');
const find = (root, type, label) => nodes(root).find(node => node.type === type && (label === undefined || text(node) === label));

async function setup({ connected = true, rest = false, restFailure = false, restOutcome = 'published', save = async () => 'export.docx', readFailure = false, resumes = [], history = [], candidates = [] } = {}) {
  const harness = createHookHarness();
  const calls = [], errors = [], notices = [];
  const draft = loadTsModule('src/lib/confluenceDraft.ts');
  const { MeetingExportButtons } = loadTsModule('src/components/MeetingDetails/MeetingExportButtons.tsx', {
    react: harness.react,
    'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': names('Loader2 Upload ChevronDown'),
    '@/components/IntegrationIcons': names('ConfluenceIcon WordIcon OneNoteIcon OneDriveIcon PlannerIcon ToDoIcon'),
    '@/components/ui/button': names('Button'),
    '@/components/ui/input': names('Input'),
    '@/components/ui/textarea': names('Textarea'),
    '@/components/ui/select': names('Select SelectContent SelectItem SelectTrigger SelectValue'),
    '@/components/ui/label': names('Label'),
    '@/components/ui/dropdown-menu': names('DropdownMenu DropdownMenuContent DropdownMenuItem DropdownMenuLabel DropdownMenuTrigger'),
    '@/components/ui/dialog': names('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
    './ExportContentOptions': names('ExportContentOptions'),
    './PlannerExportPreview': names('PlannerExportPreview'),
    './ToDoExportPreview': names('ToDoExportPreview'),
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'pick_word_save_path') {
        // The core opens the dialog: this is the one place the user picks the file.
        calls.push({ kind: 'save', args });
        return save();
      }
      calls.push({ kind: command, args });
      if (command === 'api_get_meeting') {
        if (readFailure) throw new Error('Synthetic read failure');
        return { transcripts: [{ id: 'saved', timestamp: '09:10:11', speaker: 'Speaker A', text: 'Saved words.' }] };
      }
    } },
    sonner: { toast: { success: (...args) => notices.push(['success', ...args]), info: (...args) => notices.push(['info', ...args]), warning: (...args) => notices.push(['warning', ...args]), error: (...args) => errors.push(args) } },
    '@/lib/exportDestinations': {
      getExportDestinations: () => ({ notebookId: 'notebook', notebookName: 'Saved notebook', confluenceMode: rest ? 'rest' : 'draft', confluenceBaseUrl: 'https://confluence.example.com', confluenceSpaceKey: 'TEST' }),
      setExportDestinations() {}, hasPlannerDestination: () => true, hasToDoDestination: () => true,
    },
    '@/services/microsoftExportService': {
      isOneNoteLargeLibraryError: () => false,
      microsoftExportService: {
        connectionStatus: async () => ({ state: connected ? 'connected' : 'not_connected' }),
        listOneNoteResumes: async () => resumes,
        getMeetingExportHistory: async () => history,
        listExportRecoveryCandidates: async (...args) => { calls.push({ kind: 'candidates', args }); return { items: candidates, truncated: false }; },
        reconcileMeetingExport: async (...args) => { calls.push({ kind: 'reconcile', args }); },
        listNotebooks: async () => [],

        exportMeetingToOneNoteSection: async (...args) => { calls.push({ kind: 'onenote', args }); return { overall: 'succeeded', items: [] }; },
        exportMeetingToOneDriveFiles: async args => { calls.push({ kind: 'onedrive', args }); return { files: [], destination: { name: 'Folder' } }; },
      },
    },
    '@/lib/confluenceDraft': { ...draft, writeConfluenceDraftToClipboard: async markdown => { calls.push({ kind: 'clipboard', markdown }); return 'rich'; } },
    '@/services/confluenceExportService': { confluenceExportService: { exportPage: async args => {
      calls.push({ kind: 'confluence', args });
      if (restFailure) throw new Error(typeof restFailure === 'string' ? restFailure : 'Synthetic REST failure');
      return { title: 'Page', outcome: restOutcome, webUrl: 'https://confluence.example.com/pages/viewpage.action?pageId=321' };
    } } },
  });
  const props = { meetingId: 'meeting', meetingTitle: 'Review', meetingCreatedAt: '2026-01-01', getMarkdown: async () => 'Edited notes' };
  const render = () => harness.render(() => MeetingExportButtons(props));
  render();
  await flush();
  return { render, calls, errors, notices };
}

const destinations = [
  ['Word document (.docx)', 'Save as Word document', 'export_local_word'],
  ['Confluence', 'Export to Confluence', 'clipboard'],
  ['OneDrive DOCX/PDF', 'Upload to OneDrive', 'onedrive'],
  ['OneNote', 'Export page', 'onenote'],
];

test('Word export uses its integration icon', async () => {
  const view = await setup();
  const item = find(view.render(), 'DropdownMenuItem', 'Word document (.docx)');
  assert.ok(find(item, 'WordIcon'));
});

for (const [menuLabel, buttonLabel, kind] of destinations) {
  test(`${menuLabel} reviews content before exporting and honors all inclusion modes`, async () => {
    for (const content of ['summary', 'transcript', 'both']) {
      const view = await setup();
      await find(view.render(), 'DropdownMenuItem', menuLabel).props.onClick();
      let dialog = nodes(view.render()).find(node => node.type === 'Dialog' && node.props.open);
      assert.ok(dialog);
      assert.equal(view.calls.length, 0, 'Opening a menu item must not export or copy anything');
      find(dialog, 'ExportContentOptions').props.onChange({ content, speakers: false, timestamps: false });
      dialog = nodes(view.render()).find(node => node.type === 'Dialog' && node.props.open);
      await find(dialog, 'Button', buttonLabel).props.onClick();
      assert.equal(view.errors.length, 0);
      const result = view.calls.find(call => call.kind === kind);
      assert.ok(result, `No ${kind} export`);
      if (kind === 'export_local_word') {
        assert.equal(result.args.includeTranscript, content !== 'summary');
        assert.equal(result.args.markdown, content === 'transcript' ? '' : 'Edited notes');
        assert.equal(result.args.includeSpeakers, false);
        assert.equal(result.args.includeTimestamps, false);
      } else {
        const body = kind === 'clipboard' ? result.markdown : kind === 'onenote' ? result.args[2] : `${result.args.markdown}\n${result.args.transcript ?? ''}`;
        assert.equal(body.includes('Edited notes'), content !== 'transcript');
        assert.equal(body.includes('Saved words.'), content !== 'summary');
        assert.ok(!body.includes('Speaker A'));
        assert.ok(!body.includes('09:10:11'));
        assert.equal(view.calls.some(call => call.kind === 'api_get_meeting'), content !== 'summary');
      }
    }
  });
}

test('Word remains in the menu while Microsoft is disconnected; cancel and duplicate clicks do not write', async () => {
  const wait = deferred();
  const view = await setup({ connected: false, save: () => wait.promise });
  assert.equal(find(view.render(), 'DropdownMenuItem', 'OneDrive DOCX/PDF'), undefined);
  find(view.render(), 'DropdownMenuItem', 'Word document (.docx)').props.onClick();
  const button = find(view.render(), 'Button', 'Save as Word document');
  const first = button.props.onClick();
  await button.props.onClick();
  await flush();
  assert.equal(view.calls.filter(call => call.kind === 'save').length, 1);
  wait.resolve(null);
  await first;
  assert.ok(!view.calls.some(call => call.kind === 'export_local_word'));
  assert.equal(view.errors.length, 0);
});

test('Confluence REST and clipboard fallback preserve the same selected transcript', async () => {
  const view = await setup({ rest: true, restFailure: true });
  find(view.render(), 'DropdownMenuItem', 'Confluence').props.onClick();
  find(view.render(), 'ExportContentOptions').props.onChange({ content: 'transcript', speakers: true, timestamps: true });
  await find(view.render(), 'Button', 'Export to Confluence').props.onClick();
  const html = view.calls.find(call => call.kind === 'confluence').args.bodyStorage;
  const markdown = view.calls.find(call => call.kind === 'clipboard').markdown;
  for (const body of [html, markdown]) {
    assert.match(body, /09:10:11/);
    assert.match(body, /Speaker A: Saved words\./);
    assert.ok(!body.includes('Edited notes'));
  }
  assert.equal(view.errors.length, 1, 'REST failure remains visible after copying a fallback');
});

test('Confluence does not copy a browser draft when the direct publish may have landed', async () => {
  const view = await setup({ rest: true, restFailure: 'Check Confluence and resolve the outcome in Export history.' });
  find(view.render(), 'DropdownMenuItem', 'Confluence').props.onClick();
  await find(view.render(), 'Button', 'Export to Confluence').props.onClick();
  assert.deepEqual(view.calls.map(call => call.kind), ['confluence']);
});

test('One Confluence page can be opened after a repeat or an edited draft without claiming new success', async () => {
  for (const [restOutcome, headline] of [['alreadyPublished', 'Already published to Confluence'], ['draftChanged', 'Confluence page already exists']]) {
    const view = await setup({ rest: true, restOutcome });
    find(view.render(), 'DropdownMenuItem', 'Confluence').props.onClick();
    await find(view.render(), 'Button', 'Export to Confluence').props.onClick();
    assert.deepEqual(view.notices.map(([kind, title]) => [kind, title]), [['info', headline]]);
    assert.equal(view.notices[0][2].action.label, 'Open');
    assert.deepEqual(view.calls.map(call => call.kind), ['confluence']);
  }
});

test('Planner reports selected tasks that had no attempt when the batch stopped', async () => {
  const view = await setup();
  find(view.render(), 'DropdownMenuItem', 'Planner action items').props.onClick();
  find(view.render(), 'PlannerExportPreview').props.onReport({
    overall: 'partial',
    items: [
      { status: 'unknownaftersubmit', graphCalled: true, code: 'submission_unconfirmed' },
      { status: 'skipped', graphCalled: false, code: 'export_stopped_before_attempt' },
      { status: 'skipped', graphCalled: false, code: 'export_stopped_before_attempt' },
    ],
  });
  const [[kind, title, detail]] = view.notices;
  assert.equal(kind, 'warning');
  assert.equal(title, 'Planner export finished with issues');
  assert.match(detail.description, /2 tasks were not attempted/);
});

test('failed transcript reads prevent remote exports and leave the dialog open for retry', async () => {
  const view = await setup({ readFailure: true });
  find(view.render(), 'DropdownMenuItem', 'OneDrive DOCX/PDF').props.onClick();
  find(view.render(), 'ExportContentOptions').props.onChange({ content: 'both', speakers: true, timestamps: true });
  await find(view.render(), 'Button', 'Upload to OneDrive').props.onClick();
  assert.ok(!view.calls.some(call => call.kind === 'onedrive'));
  assert.equal(view.errors.length, 1);
  assert.ok(nodes(view.render()).some(node => node.type === 'Dialog' && node.props.open));
});

test('summary-only controls hide speakers and keep timestamps enabled without clearing choices', () => {
  const { ExportContentOptions } = loadTsModule('src/components/MeetingDetails/ExportContentOptions.tsx', {
    'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/checkbox': names('Checkbox'),
    '@/components/ui/select': names('Select SelectContent SelectItem SelectTrigger SelectValue'),
  });
  for (const content of ['summary', 'transcript', 'both']) {
    const root = ExportContentOptions({ value: { content, speakers: true, timestamps: false }, onChange() {} });
    const inputs = nodes(root).filter(node => node.type === 'Checkbox');
    assert.equal(inputs.length, content === 'summary' ? 1 : 2);
    assert.ok(inputs.every(input => input.props.disabled === false));
    assert.equal(text(root).includes('Speaker labels'), content !== 'summary');
    assert.match(text(root), /Timestamps/);
    assert.match(text(root), /Source times in the summary and line times in the transcript/);
    assert.equal(inputs.at(-1).props.checked, false);
    if (content !== 'summary') assert.equal(inputs[0].props.checked, true);
  }
});

test('OneNote resumes with the saved content options and explicit section identity', async () => {
  const options = { content: 'transcript', speakers: false, timestamps: false };
  const view = await setup({ resumes: [{
    id: 'saved-section-attempt', pageTitle: 'Original title', sectionName: 'Original section',
    startedAt: '2026-01-01T12:00:00Z', options, canResume: true,
  }] });
  await find(view.render(), 'DropdownMenuItem', 'OneNote').props.onClick();
  const dialog = nodes(view.render()).find(node => node.type === 'Dialog' && node.props.open);
  assert.ok(find(dialog, 'SelectItem', 'Create another section'));
  assert.equal(find(dialog, 'ExportContentOptions'), undefined);
  await find(dialog, 'Button', 'Resume export').props.onClick();
  const call = view.calls.find(call => call.kind === 'onenote');
  assert.equal(call.args[1], 'Original title');
  assert.equal(call.args[2], '## Transcript\n\nSaved words.');
  assert.deepEqual(call.args.slice(5), [options, 'saved-section-attempt', false]);
});

test('OneNote edits are reviewed before updating the saved section', async () => {
  const options = { content: 'summary', speakers: false, timestamps: false };
  const view = await setup({ resumes: [{
    id: 'saved', pageTitle: 'Original title', sectionName: 'Original section',
    startedAt: '2026-01-01T12:00:00Z', options, canResume: true,
  }] });
  await find(view.render(), 'DropdownMenuItem', 'OneNote').props.onClick();
  const select = nodes(view.render()).find(node => node.type === 'Select' && node.props.value === 'resume');
  select.props.onValueChange('update');
  await find(view.render(), 'Button', 'Review changes').props.onClick();
  assert.ok(!view.calls.some(call => call.kind === 'onenote'));
  const draft = find(view.render(), 'Textarea');
  assert.equal(draft.props.value, 'Edited notes');
  draft.props.onChange({ target: { value: 'Only these reviewed notes' } });
  await find(view.render(), 'Button', 'Update exported page').props.onClick();
  const { args } = view.calls.find(call => call.kind === 'onenote');
  assert.equal(args[2], 'Only these reviewed notes');
  assert.deepEqual(args.slice(5), [options, 'saved', true]);
});

test('offline export history shows results and offers safe available links', async () => {
  const view = await setup({ connected: false, history: [
    { destination: 'OneNote', status: 'succeeded', at: '2026-01-01T12:00:00Z', webUrl: 'https://www.onenote.com/page' },
    { entryKey: 'opaque-key', destination: 'Microsoft To Do', status: 'unknown_after_submit', at: null, webUrl: null },
  ] });
  find(view.render(), 'DropdownMenuItem', 'Export history').props.onClick();
  await flush();
  const dialog = nodes(view.render()).find(node => node.type === 'Dialog' && node.props.open);
  assert.match(text(dialog), /OneNote · Exported/);
  assert.match(text(dialog), /Microsoft To Do · Outcome unknown/);
  assert.equal(find(dialog, 'Button', 'Open').props['aria-label'], 'Open in OneNote');
});

test('export history requires an explicit finding before allowing another submission', async () => {
  const view = await setup({ history: [
    { entryKey: 'opaque-key', destination: 'Confluence', status: 'unknown_after_submit', at: null, webUrl: null, canResolve: true, canConfirmAbsence: true },
  ] });
  await find(view.render(), 'DropdownMenuItem', 'Export history').props.onClick();
  await flush();
  find(view.render(), 'Button', 'Resolve outcome').props.onClick();
  assert.equal(find(view.render(), 'Button', 'Save outcome').props.disabled, true);
  const select = nodes(view.render()).find(node => node.type === 'Select' && node.props.value === '');
  select.props.onValueChange('absent');
  assert.equal(find(view.render(), 'Button', 'Allow retry').props.disabled, false);
  find(view.render(), 'form').props.onSubmit({ preventDefault() {} });
  await flush();
  assert.equal(view.calls.length, 1);
  const [{ kind, args }] = view.calls;
  assert.equal(kind, 'reconcile');
  assert.deepEqual(args.slice(0, 3), ['meeting', 'opaque-key', null]);
  assert.equal(args[3].baseUrl, 'https://confluence.example.com');
  assert.equal(args[3].spaceKey, 'TEST');
});

test('export history can review a confirmed link using candidates from the recorded destination', async () => {
  const view = await setup({ history: [
    { entryKey: 'opaque-key', destination: 'Microsoft To Do', status: 'partial_failure', at: null, webUrl: null, canResolve: true, canConfirmAbsence: true },
  ], candidates: [{ id: 'chosen-id', title: 'Review numbers', createdAt: null, webUrl: null }] });
  await find(view.render(), 'DropdownMenuItem', 'Export history').props.onClick();
  await flush();
  find(view.render(), 'Button', 'Review link').props.onClick();
  let dialog = view.render();
  nodes(dialog).find(node => node.type === 'Select' && node.props.value === '').props.onValueChange('exists');
  view.render();
  await flush();
  dialog = view.render();
  assert.ok(find(dialog, 'SelectItem', 'Review numbers'));
  assert.equal(find(dialog, 'Input'), undefined);
  nodes(dialog).find(node => node.type === 'Select' && node.props.value === '').props.onValueChange('chosen-id');
  find(view.render(), 'form').props.onSubmit({ preventDefault() {} });
  await flush();
  assert.deepEqual(view.calls, [
    { kind: 'candidates', args: ['meeting', 'opaque-key'] },
    { kind: 'reconcile', args: ['meeting', 'opaque-key', 'chosen-id', null] },
  ]);
});

test('OneDrive recovery shows what a manually linked file will be used for', async () => {
  for (const [destination, title, uses] of [
    ['OneDrive Word document', 'Revised notes.docx', 'unfinished PDF conversion and Word sharing'],
    ['OneDrive PDF', 'Revised notes.pdf', 'PDF sharing'],
  ]) {
    const view = await setup({
      history: [{ entryKey: 'opaque-key', destination, status: 'succeeded', at: null, webUrl: null, canResolve: true, canConfirmAbsence: true }],
      candidates: [{ id: 'chosen-id', title, createdAt: null, webUrl: null }],
    });
    await find(view.render(), 'DropdownMenuItem', 'Export history').props.onClick();
    await flush();
    find(view.render(), 'Button', 'Review link').props.onClick();
    nodes(view.render()).find(node => node.type === 'Select' && node.props.value === '').props.onValueChange('exists');
    view.render();
    await flush();
    nodes(view.render()).find(node => node.type === 'Select' && node.props.value === '').props.onValueChange('chosen-id');
    const form = find(view.render(), 'form');
    assert.ok(text(form).includes(`A repeat export of this draft will use “${title}” for ${uses}, if requested.`));
    assert.equal(view.calls.filter(call => call.kind === 'reconcile').length, 0);
  }
});

test('unnamed Confluence history offers a manual recovery path', async () => {
  const view = await setup({ history: [
    { entryKey: 'opaque-key', destination: 'Confluence', status: 'unknown_after_submit', at: null, webUrl: null, canResolve: true, canConfirmAbsence: false },
  ] });
  await find(view.render(), 'DropdownMenuItem', 'Export history').props.onClick();
  await flush();
  find(view.render(), 'Button', 'Resolve outcome').props.onClick();
  const form = find(view.render(), 'form');
  assert.ok(text(form).includes('This export has no saved page title.'));
  assert.ok(text(form).includes('review a browser draft before publishing manually'));
  assert.ok(find(form, 'SelectItem', 'I found the exported item'));
  assert.equal(find(form, 'SelectItem', 'I confirmed the item is missing'), undefined);
});
