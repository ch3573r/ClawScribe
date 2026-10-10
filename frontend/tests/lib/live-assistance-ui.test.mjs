import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = node => !node || typeof node !== 'object' ? [] : Array.isArray(node) ? node.flatMap(nodes) : [node, ...nodes(node.props?.children), ...nodes(node.props?.actions), ...nodes(node.props?.title)];
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node) : Array.isArray(node) ? node.map(text).join('') : node ? text(node.props?.children) : '';
const liveSnapshot = { session_id: 'session-ui', recording_generation: '1', finalized_through_seconds: 3540, segments: [{ sequence_id: 700, text: 'Public finalized fixture', start_seconds: 3535, end_seconds: 3540 }], transcription_incomplete: true, transcription_available: true };
const reply = request => ({ request_id: request.request_id, message_id: 'assistant-ui', content: 'The rollout remains open.', evidence: [], evidence_metadata: [], cited_tags: [], context_links: [], retrieval_mode: 'keyword', provider: 'custom-openai', model: 'actual-model', live_context: { session_id: 'session-ui', finalized_through_seconds: 3500, transcription_incomplete: true } });

async function panel({ sharing = true, available = true, provider = 'custom-openai', ask } = {}) {
  const requests = []; const cancelled = [];
  const { createLiveAssistanceStore } = loadTsModule('src/lib/live-assistance-state.ts');
  const owner = createLiveAssistanceStore({ service: {
    liveSnapshot: async () => ({ ...liveSnapshot, transcription_available: available }), liveSharing: async () => sharing,
    documentSharing: async () => false, setLiveSharing: async () => {}, setDocumentSharing: async () => {}, scopeDocuments: async () => [],
    ask: async request => { requests.push(request); return ask ? ask(request) : reply(request); }, cancel: async id => { cancelled.push(id); },
  }, listen: async () => () => {}, interval: () => () => {}, uuid: () => `ui-request-${requests.length + 1}` });
  owner.configureProvider(provider, 'selected-model'); const disconnect = owner.connect(); await flush(); owner.setRecording('recording'); await flush();
  const hooks = createHookHarness(); const { LiveAssistancePanel } = loadTsModule('src/components/LiveAssistancePanel.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/hooks/useLiveAssistance': { useLiveAssistance: () => ({ state: owner.getSnapshot(), actions: owner }) },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ isRecording: true, sessionMode: available ? 'live' : 'audio_only' }) },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ meetings: [{ id: 'saved-ui', title: 'Public meeting' }], currentMeeting: null }) },
    '@/components/ui/button': { Button: 'button' }, '@/components/ui/textarea': { Textarea: 'textarea' },
    '@/components/ui/switch': { Switch: 'switch' }, '@/components/ui/checkbox': { Checkbox: 'checkbox' },
    '@/components/ui/page-section': { PageSection: 'section' }, '@/components/ui/multi-select': { MultiSelect: 'multi-select' },
    '@/components/ui/tooltip': { Tooltip: 'tooltip', TooltipTrigger: 'trigger', TooltipContent: 'content' },
    '@/components/ui/dialog': { Dialog: 'dialog', DialogContent: 'content', DialogHeader: 'header', DialogTitle: 'title', DialogDescription: 'description' },
    '@/components/Knowledge/KnowledgeChat': { CitedAnswer: 'cited-answer' },
    '@/components/Knowledge/EvidencePreview': { EvidencePreview: 'evidence-preview' },
    '@/services/knowledgeService': { knowledgeService: { resolve: async () => ({ status: 'current' }) } },
    'next/navigation': { useRouter: () => ({ push: () => {} }) },
  });
  const render = () => hooks.render(() => LiveAssistancePanel());
  const button = label => nodes(render()).find(node => node.type === 'button' && text(node) === label);
  const expand = () => { const toggle = nodes(render()).find(node => node.type === 'button' && node.props['aria-controls'] === 'live-assistance-body'); assert.ok(toggle, 'a keyboard-accessible collapse toggle exists'); toggle.props.onClick(); };
  return { owner, requests, cancelled, render, button, expand, dispose() { hooks.unmount(); disconnect(); } };
}

test('manual panel stays quiet, reports finalized lag while collapsed, and Enter submits once', async () => {
  const pending = deferred(); const app = await panel({ ask: () => pending.promise });
  try {
    assert.match(text(app.render()), /59:00/); assert.match(text(app.render()), /incomplete|behind/i); assert.equal(app.requests.length, 0);
    app.expand(); const input = nodes(app.render()).find(node => node.type === 'textarea'); assert.ok(input);
    input.props.onChange({ target: { value: 'What remains open?' } });
    let prevented = 0; const enter = { key: 'Enter', shiftKey: false, nativeEvent: { isComposing: false }, preventDefault() { prevented++; } };
    nodes(app.render()).find(node => node.type === 'textarea').props.onKeyDown({ ...enter, shiftKey: true }); assert.equal(app.requests.length, 0);
    nodes(app.render()).find(node => node.type === 'textarea').props.onKeyDown(enter);
    nodes(app.render()).find(node => node.type === 'textarea').props.onKeyDown(enter);
    assert.equal(prevented, 2); assert.equal(app.requests.length, 1); assert.equal(app.requests[0].search.query, 'What remains open?');
    assert.equal(app.button('Ask').props.disabled, true); assert.ok(app.button('Cancel'));
    app.button('Cancel').props.onClick(); assert.equal(app.cancelled.length, 1);
    pending.resolve(reply(app.requests[0])); await flush(); assert.equal(app.owner.getSnapshot().messages.length, 0);
  } finally { app.dispose(); }
});

test('each manual action displays the response finalized time and actual provider, even after newer status', async () => {
  const app = await panel();
  try {
    app.expand(); assert.ok(app.button('Summarize so far')); assert.ok(app.button('List open questions'));
    app.button('Summarize so far').props.onClick(); await flush();
    assert.equal(app.requests.length, 1); assert.match(app.requests[0].search.query, /summar/i);
    assert.match(text(app.render()), /58:20/); assert.match(text(app.render()), /59:00/);
    app.button('List open questions').props.onClick(); await flush(); assert.equal(app.requests.length, 2);
    assert.match(app.requests[1].search.query, /open questions/i); assert.match(text(app.render()), /provider|OpenAI/i);
  } finally { app.dispose(); }
});

test('sharing remains an explicit separate control and unavailable modes offer saved-meeting guidance', async () => {
  const app = await panel({ sharing: false });
  try {
    app.expand(); const controls = nodes(app.render()).filter(node => node.type === 'switch');
    assert.equal(controls.length, 2); assert.equal(controls[0].props.checked, false); assert.equal(controls[1].props.checked, false);
    assert.equal(app.button('Summarize so far').props.disabled, true); assert.equal(app.requests.length, 0);
  } finally { app.dispose(); }
  for (const configuration of [{ available: false }, { provider: 'builtin-ai' }]) {
    const unavailable = await panel(configuration);
    try { unavailable.expand(); assert.match(text(unavailable.render()), /saved meeting|after recording|transcribe/i); assert.equal(unavailable.requests.length, 0); }
    finally { unavailable.dispose(); }
  }
});

test('global Live provider connects once across subscribers and receives synchronous Stop invalidation', async () => {
  const hooks = createHookHarness(); let lifecycle; let subscriptions = 0; let released = 0;
  const subscribeLifecycle = callback => { lifecycle = callback; return () => { lifecycle = null; }; };
  const events = new Map(); const pending = deferred(); const requests = [];
  const { LiveAssistanceProvider } = loadTsModule('src/contexts/LiveAssistanceContext.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ status: 'recording', subscribeLifecycle }) },
    '@/contexts/ConfigContext': { useConfig: () => ({ modelConfig: { provider: 'custom-openai', model: 'selected-model' } }) },
    '@tauri-apps/api/event': { listen: async (event, callback) => { subscriptions++; events.set(event, callback); return () => { released++; events.delete(event); }; } },
    '@/services/knowledgeService': { knowledgeService: {
      liveSnapshot: async () => liveSnapshot, liveSharing: async () => true, documentSharing: async () => false,
      ask: request => { requests.push(request); return pending.promise; }, cancel: async () => {},
    } },
  });
  const render = () => hooks.render(() => LiveAssistanceProvider({ children: 'route' }));
  try {
    let tree = render(); await flush(); tree = render(); const owner = tree?.props?.value;
    assert.ok(owner?.getSnapshot, 'global provider exposes the production state owner');
    const route = owner.subscribe(() => {}); route(); owner.subscribe(() => {})(); render(); await flush();
    assert.equal(subscriptions, 2); const ask = owner.ask('Question?'); assert.equal(requests.length, 1);
    lifecycle('stopping'); assert.equal(owner.getSnapshot().sessionId, null); assert.equal(owner.getSnapshot().pending, false);
    pending.resolve(reply(requests[0])); await ask; assert.equal(owner.getSnapshot().messages.length, 0);
  } finally { hooks.unmount(); }
  await flush(); assert.equal(released, 2); assert.equal(lifecycle, null);
});
