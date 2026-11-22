import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node && typeof node === 'object' ? [node, ...nodes(node.props?.children)] : [];
const text = node => Array.isArray(node) ? node.map(text).join('') : typeof node === 'string' || typeof node === 'number' ? String(node) : node ? text(node.props?.children) : '';
const primitives = {
  'react/jsx-runtime': { jsx, jsxs: jsx },
  'lucide-react': {},
  '@/components/ui/button': { Button: 'button' },
  '@/components/ui/input': { Input: 'input' },
  '@/components/ui/textarea': { Textarea: 'textarea' },
  '@/components/ui/tooltip': { Tooltip: 'tooltip', TooltipTrigger: 'tooltip-trigger', TooltipContent: 'tooltip-content' },
  '@/components/ui/dropdown-menu': {
    DropdownMenu: 'menu', DropdownMenuTrigger: 'menu-trigger', DropdownMenuContent: 'menu-content', DropdownMenuItem: 'menu-item',
    DropdownMenuSub: 'menu-sub', DropdownMenuSubTrigger: 'menu-sub-trigger', DropdownMenuSubContent: 'menu-sub-content', DropdownMenuSeparator: 'separator',
  },
  '@/components/ui/dialog': { Dialog: 'dialog', DialogContent: 'dialog-content', DialogTitle: 'dialog-title' },
  '@/components/ui/visually-hidden': { VisuallyHidden: 'visually-hidden' },
  '@/components/ui/popover': { Popover: 'popover', PopoverTrigger: 'popover-trigger', PopoverContent: 'popover-content' },
  './ActionIcon': { ActionIcon: 'action-icon' },
  './MeetingDetails/ActionIcon': { ActionIcon: 'action-icon' },
  './ProjectTags': { ProjectTags: 'project-tags' },
  './MeetingExportButtons': { MeetingExportButtons: 'export-buttons' },
  './ReviewedActions': { ReviewedActions: 'reviewed-actions' },
  './SeriesPreparation': { SavedSeriesReview: 'series-review' },
  './FollowUpEmail': { FollowUpEmail: 'follow-up-email' },
  './SavedLiveAnswers': { SavedLiveAnswers: 'saved-live-answers' },
  './MeetingChat': { MeetingChat: 'meeting-chat' },
  './SummarySources': { SummarySources: 'summary-sources' },
  './TextReplaceDialog': { TextReplaceDialog: 'replace-dialog' },
  '@/components/AISummary/BlockNoteSummaryView': { BlockNoteSummaryView: 'summary-editor' },
  '@/components/AISummary/SummarySurface': { SummarySurface: 'summary-surface' },
  '@/components/ModelSettingsModal': { ModelSettingsModal: 'model-settings' },
  '@/components/LanguagePickerPopover': { LanguagePickerPopover: 'language-picker' },
  '@/hooks/useRecentLanguages': { useRecentLanguages: () => ({ addRecent() {} }) },
  '@/lib/summary-language-preferences': { readMeetingSummaryLanguage: async () => ({ language: null, storage: 'metadata' }) },
  '@/lib/summary-languages': { labelForCode: code => code },
  '@/lib/openExternal': { openExternal() {} },
  '@/lib/utils': { isOllamaNotInstalledError: () => false },
  '@/lib/analytics': { default: { trackButtonClick() {} } },
  '@tauri-apps/api/core': { invoke: async () => {} },
  sonner: { toast: {} },
};

// Model the close-focus boundary Radix exposes: unless prevented, focus returns
// to the overflow trigger after the selected action has focused its destination.
function closeMenu(content, restoreTriggerFocus) {
  const event = { defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
  content.props.onCloseAutoFocus?.(event);
  if (!event.defaultPrevented) restoreTriggerFocus();
}

test('Rename retains title focus after overflow closes and normal dismissal still restores the trigger', () => {
  const headerHooks = createHookHarness();
  const titleHooks = createHookHarness();
  const { MeetingHeader } = loadTsModule('src/components/MeetingDetails/MeetingHeader.tsx', { ...primitives, react: headerHooks.react });
  const { EditableTitle } = loadTsModule('src/components/EditableTitle.tsx', { ...primitives, react: titleHooks.react });
  let editing = false, finished = 0, active = 'overflow';
  const props = {
    meeting: { id: 'example', created_at: '2026-01-01T12:00:00Z' }, title: 'Review', segmentCount: 2, disabled: false,
    onStartEditing: () => { editing = true; }, onFinishEditing: () => { finished++; }, onChange() {}, getMarkdown: async () => '', onOpenFolder: async () => {},
  };
  const render = () => headerHooks.render(() => MeetingHeader({ ...props, isEditing: editing }));
  let tree = render();
  nodes(tree).find(node => node.type === 'menu-item' && text(node) === 'Rename meeting').props.onSelect();
  tree = render();
  const titleProps = nodes(tree).find(node => node.type === EditableTitle || node.props?.meetingTitle)?.props;
  const input = titleHooks.render(() => EditableTitle(titleProps));
  assert.equal(input.type, 'textarea');
  if (input.props.autoFocus) active = 'title';
  const content = nodes(tree).find(node => node.type === 'menu-content');
  closeMenu(content, () => { active = 'overflow'; input.props.onBlur(); });
  assert.equal(active, 'title');
  assert.equal(finished, 0, 'closing the menu must not blur and finish the title editor');
  closeMenu(content, () => { active = 'overflow'; });
  assert.equal(active, 'overflow', 'suppression applies only to the Rename close');
  headerHooks.unmount(); titleHooks.unmount();
});

test('the saved header preserves attendance and returns focus after reviewing meeting actions', () => {
  const hooks = createHookHarness();
  const { MeetingHeader } = loadTsModule('src/components/MeetingDetails/MeetingHeader.tsx', { ...primitives, react: hooks.react });
  const attendance = jsx('attendance', {});
  const props = {
    meeting: { id: 'example', created_at: '2026-01-01T12:00:00Z' }, title: 'Review', segmentCount: 2, disabled: false,
    onStartEditing() {}, onFinishEditing() {}, onChange() {}, getMarkdown: async () => '', onOpenFolder: async () => {}, attendance,
  };
  const render = () => nodes(hooks.render(() => MeetingHeader(props)));
  let tree = render();
  assert.ok(tree.includes(attendance));
  let focused = false;
  tree.find(node => node.type === 'action-icon').props.ref.current = { focus() { focused = true; } };
  tree.find(node => node.type === 'menu-item' && text(node) === 'Meeting actions').props.onSelect();
  tree = render();
  closeMenu(tree.find(node => node.type === 'menu-content'), () => assert.fail('menu steals dialog focus'));
  const dialog = tree.find(node => node.type === 'reviewed-actions');
  assert.equal(dialog.props.open, true);
  dialog.props.onReturnFocus();
  assert.equal(focused, true);
  hooks.unmount();
});

const summaryProps = {
  saveState: 'saved', hasPreviousSummary: false, summaryRevision: 0, onRestorePrevious: async () => {}, onSummaryDraft() {},
  meeting: { id: 'example', title: 'Review', created_at: '2026-01-01T12:00:00Z' }, meetingTitle: 'Review', isTitleDirty: false,
  summaryRef: { current: null }, isSaving: false, onSaveAll: async () => {}, onCopySummary: async () => {}, onOpenFolder: async () => {},
  aiSummary: { markdown: 'Meeting notes' }, summaryStatus: 'completed', transcripts: [{ id: 'one', text: 'Example' }],
  modelConfig: { provider: 'example', model: 'example' }, setModelConfig() {}, onSaveModelConfig: async () => {}, onGenerateSummary: async () => {}, onStopGeneration() {},
  customPrompt: '', contextLoading: false, onPromptChange() {}, onSaveSummary: async () => {}, onSummaryChange() {}, onDirtyChange() {}, summaryError: null,
  getSummaryStatusMessage: () => '', availableTemplates: [], selectedTemplate: '', onTemplateSelect() {},
};

test('Find retains input focus after summary overflow closes and resets the suppression', () => {
  const previousWindow = globalThis.window;
  const frames = [];
  globalThis.window = { requestAnimationFrame: callback => frames.push(callback) };
  const panelHooks = createHookHarness(), generatorHooks = createHookHarness();
  try {
    const { SummaryPanel } = loadTsModule('src/components/MeetingDetails/SummaryPanel.tsx', { ...primitives, react: panelHooks.react });
    const { SummaryGeneratorButtonGroup } = loadTsModule('src/components/MeetingDetails/SummaryGeneratorButtonGroup.tsx', { ...primitives, react: generatorHooks.react });
    const render = () => panelHooks.render(() => SummaryPanel(summaryProps));
    let tree = render();
    const generatorProps = nodes(tree).find(node => node.props?.actionsSlot)?.props;
    const toolbar = generatorHooks.render(() => SummaryGeneratorButtonGroup(generatorProps));
    const actions = generatorProps.actionsSlot.type(generatorProps.actionsSlot.props);
    nodes(actions).find(node => node.type === 'menu-item' && text(node) === 'Find in summary').props.onSelect();
    tree = render();
    const input = nodes(tree).find(node => node.type === 'input' && node.props.placeholder === 'Find in summary');
    assert.ok(input, 'Find must reveal the search input');
    let active = 'overflow';
    input.props.ref.current = { focus() { active = 'find'; }, select() {} };
    frames.splice(0).forEach(callback => callback());
    assert.equal(active, 'find');
    const content = nodes(toolbar).find(node => node.type === 'menu-content');
    closeMenu(content, () => { active = 'overflow'; });
    assert.equal(active, 'find', 'menu close must leave the revealed search input focused');
    closeMenu(content, () => { active = 'overflow'; });
    assert.equal(active, 'overflow', 'later menu dismissal must restore normal focus');
  } finally {
    panelHooks.unmount(); generatorHooks.unmount(); globalThis.window = previousWindow;
  }
});

test('summary save status appears only when a summary exists', () => {
  const hooks = createHookHarness();
  const { SummaryPanel } = loadTsModule('src/components/MeetingDetails/SummaryPanel.tsx', { ...primitives, react: hooks.react });
  for (const [saveState, label] of [['saved', 'Saved'], ['saving', 'Saving…'], ['error', 'Save failed — edits retained'], ['dirty', 'Unsaved changes']]) {
    const empty = hooks.render(() => SummaryPanel({ ...summaryProps, saveState, aiSummary: null, summaryStatus: 'idle' }));
    assert.equal(nodes(empty).filter(node => node.props?.role === 'status').length, 0, 'an empty summary must not announce a save status');
    const populated = hooks.render(() => SummaryPanel({ ...summaryProps, saveState }));
    assert.equal(text(nodes(populated).find(node => node.props?.role === 'status')), label);
  }
  hooks.unmount();
});

test('meeting title finishes on Enter with or without Shift and preserves blur and Escape behavior', () => {
  const hooks = createHookHarness();
  const { EditableTitle } = loadTsModule('src/components/EditableTitle.tsx', { ...primitives, react: hooks.react });
  let finished = 0, prevented = 0;
  const input = hooks.render(() => EditableTitle({ title: 'Review', meetingTitle: true, isEditing: true, onStartEditing() {}, onFinishEditing: () => { finished++; }, onChange() {} }));
  for (const [key, shiftKey] of [['Enter', false], ['Enter', true], ['Escape', false]]) {
    input.props.onKeyDown({ key, shiftKey, preventDefault() { prevented++; } });
  }
  assert.equal(finished, 3);
  assert.equal(prevented, 3, 'neither Enter variant may insert a line break');
  input.props.onBlur();
  assert.equal(finished, 4);
  hooks.unmount();
});

test('meeting title strips pasted line breaks without trimming ordinary spaces', () => {
  const hooks = createHookHarness();
  const { EditableTitle } = loadTsModule('src/components/EditableTitle.tsx', { ...primitives, react: hooks.react });
  const values = [];
  const input = hooks.render(() => EditableTitle({ title: 'Review', meetingTitle: true, isEditing: true, onStartEditing() {}, onFinishEditing() {}, onChange: value => values.push(value) }));
  input.props.onChange({ target: { value: '  First\r\nsecond\nthird\rfourth\u2028fifth\u2029sixth  ' } });
  assert.deepEqual(values, ['  First second third fourth fifth sixth  ']);
  hooks.unmount();
});

test('the empty summary view offers the one primary Generate action, and the toolbar steps back', () => {
  const hooks = createHookHarness();
  const { SummaryPanel } = loadTsModule('src/components/MeetingDetails/SummaryPanel.tsx', { ...primitives, '@/lib/analytics': { __esModule: true, default: { trackButtonClick() {} } }, react: hooks.react });
  const render = props => hooks.render(() => SummaryPanel({ ...summaryProps, ...props }));
  const group = tree => nodes(tree).find(node => node.props && 'secondaryGenerate' in node.props);
  const generate = tree => nodes(tree).find(node => node.type === 'button' && text(node) === 'Generate summary');

  const empty = render({ aiSummary: null, summaryStatus: 'idle' });
  assert.ok(generate(empty), 'the empty view has the action');
  assert.equal(group(empty).props.secondaryGenerate, true, 'so the toolbar one is secondary');
  let started = 0;
  group(empty).props.generateRef.current = () => { started++; };
  generate(empty).props.onClick();
  assert.equal(started, 1, 'it runs the same action as the toolbar');
  assert.doesNotMatch(text(empty), /toolbar above/);

  const noTranscript = render({ aiSummary: null, summaryStatus: 'idle', transcripts: [] });
  assert.equal(generate(noTranscript), undefined, 'nothing to generate from');
  assert.equal(group(noTranscript).props.secondaryGenerate, false);

  const generating = render({ aiSummary: null, summaryStatus: 'processing' });
  assert.equal(generate(generating), undefined, 'no Generate while one is running');
  assert.equal(group(generating).props.secondaryGenerate, false);

  const populated = render({});
  assert.equal(group(populated).props.secondaryGenerate, false, 'with a summary the toolbar action is the only one');
  hooks.unmount();
});
