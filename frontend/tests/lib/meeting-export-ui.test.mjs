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

async function setup({ connected = true, rest = false, restFailure = false, save = async () => 'export.docx', readFailure = false } = {}) {
  const harness = createHookHarness();
  const calls = [], errors = [];
  const draft = loadTsModule('src/lib/confluenceDraft.ts');
  const { MeetingExportButtons } = loadTsModule('src/components/MeetingDetails/MeetingExportButtons.tsx', {
    react: harness.react,
    'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': names('Loader2 Upload ChevronDown'),
    '@/components/IntegrationIcons': names('ConfluenceIcon WordIcon OneNoteIcon OneDriveIcon PlannerIcon ToDoIcon'),
    '@/components/ui/button': names('Button'),
    '@/components/ui/input': names('Input'),
    '@/components/ui/label': names('Label'),
    '@/components/ui/dropdown-menu': names('DropdownMenu DropdownMenuContent DropdownMenuItem DropdownMenuLabel DropdownMenuTrigger'),
    '@/components/ui/dialog': names('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
    './ExportContentOptions': names('ExportContentOptions'),
    './PlannerExportPreview': names('PlannerExportPreview'),
    './ToDoExportPreview': names('ToDoExportPreview'),
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push({ kind: command, args });
      if (command === 'api_get_meeting') {
        if (readFailure) throw new Error('Synthetic read failure');
        return { transcripts: [{ id: 'saved', timestamp: '09:10:11', speaker: 'Speaker A', text: 'Saved words.' }] };
      }
    } },
    '@tauri-apps/plugin-dialog': { save: async args => { calls.push({ kind: 'save', args }); return save(); } },
    sonner: { toast: { success() {}, info() {}, warning() {}, error: (...args) => errors.push(args) } },
    '@/lib/exportDestinations': {
      getExportDestinations: () => ({ notebookId: 'notebook', notebookName: 'Saved notebook', confluenceMode: rest ? 'rest' : 'draft', confluenceBaseUrl: 'https://confluence.example.com', confluenceSpaceKey: 'TEST' }),
      setExportDestinations() {}, hasPlannerDestination: () => true, hasToDoDestination: () => true,
    },
    '@/services/microsoftExportService': {
      isOneNoteLargeLibraryError: () => false,
      microsoftExportService: {
        connectionStatus: async () => ({ state: connected ? 'connected' : 'not_connected' }),
        summaryHasActionItems: async () => true,
        listNotebooks: async () => [],
        exportMeetingToOneNoteSection: async (...args) => { calls.push({ kind: 'onenote', args }); return { overall: 'succeeded', items: [] }; },
        exportMeetingToOneDriveFiles: async args => { calls.push({ kind: 'onedrive', args }); return { files: [], destination: { name: 'Folder' } }; },
      },
    },
    '@/lib/confluenceDraft': { ...draft, writeConfluenceDraftToClipboard: async markdown => { calls.push({ kind: 'clipboard', markdown }); return 'rich'; } },
    '@/services/confluenceExportService': { confluenceExportService: { exportPage: async args => {
      calls.push({ kind: 'confluence', args });
      if (restFailure) throw new Error('Synthetic REST failure');
      return { title: 'Page' };
    } } },
  });
  const props = { meetingId: 'meeting', meetingTitle: 'Review', meetingCreatedAt: '2026-01-01', getMarkdown: async () => 'Edited notes' };
  const render = () => harness.render(() => MeetingExportButtons(props));
  render();
  await flush();
  return { render, calls, errors };
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

test('failed transcript reads prevent remote exports and leave the dialog open for retry', async () => {
  const view = await setup({ readFailure: true });
  find(view.render(), 'DropdownMenuItem', 'OneDrive DOCX/PDF').props.onClick();
  find(view.render(), 'ExportContentOptions').props.onChange({ content: 'both', speakers: true, timestamps: true });
  await find(view.render(), 'Button', 'Upload to OneDrive').props.onClick();
  assert.ok(!view.calls.some(call => call.kind === 'onedrive'));
  assert.equal(view.errors.length, 1);
  assert.ok(nodes(view.render()).some(node => node.type === 'Dialog' && node.props.open));
});

test('summary-only controls disable transcript formatting without clearing the choices', () => {
  const { ExportContentOptions } = loadTsModule('src/components/MeetingDetails/ExportContentOptions.tsx', { 'react/jsx-runtime': { jsx, jsxs: jsx } });
  for (const content of ['summary', 'transcript', 'both']) {
    const root = ExportContentOptions({ value: { content, speakers: true, timestamps: false }, onChange() {} });
    const inputs = nodes(root).filter(node => node.type === 'input');
    assert.equal(inputs.length, 2);
    assert.ok(inputs.every(input => input.props.disabled === (content === 'summary')));
    assert.equal(inputs[0].props.checked, true);
    assert.equal(inputs[1].props.checked, false);
  }
});
