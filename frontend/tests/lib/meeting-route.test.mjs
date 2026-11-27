import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function createView({ fromRecording = false, outcome = null } = {}) {
  const harness = createHookHarness();
  let meetingId = 'first';
  let metadata = null;
  const calls = [];
  const summaries = new Map();
  const listeners = new Map();
  const jsx = (type, props) => ({ type, props });
  const sidebar = { setCurrentMeeting() {}, refetchMeetings() {} };
  const router = { push() {} };
  const transcripts = [{ id: 'segment', text: 'A synthetic test meeting.' }];
  const { default: MeetingDetails } = loadTsModule(fileURLToPath(new URL('../../src/app/meeting-details/page.tsx', import.meta.url)), {
    react: { ...harness.react, Suspense: 'Suspense' },
    'react/jsx-runtime': { jsx, jsxs: jsx },
    './page-content': { default: 'PageContent', __esModule: true },
    '@/components/ui/button': { Button: 'button' },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => sidebar },
    'next/navigation': { useRouter: () => router, useSearchParams: () => ({ get: key => key === 'id' ? meetingId : fromRecording ? 'recording' : null }) },
    '@/contexts/ConfigContext': { useConfig: () => ({ isAutoSummary: true }) },
    '@/hooks/usePaginatedTranscripts': { usePaginatedTranscripts: () => ({ metadata, transcripts }) },
    '@tauri-apps/api/event': { listen: async (event, callback) => {
      if (!listeners.has(event)) listeners.set(event, new Set());
      const callbacks = listeners.get(event);
      callbacks.add(callback);
      return () => callbacks.delete(callback);
    } },
    '@tauri-apps/api/core': { invoke(command, args) {
      calls.push({ command, args });
      if (command === 'get_recording_outcome') return Promise.resolve(outcome);
      if (command === 'api_get_summary') {
        const request = deferred();
        summaries.set(args.meetingId, request);
        return request.promise;
      }
      return Promise.resolve(command === 'api_get_model_config' ? {} : null);
    } },
  });
  const findContent = node => {
    if (!node || typeof node !== 'object') return undefined;
    if (Array.isArray(node)) return node.map(findContent).find(Boolean);
    if (node.type === 'PageContent') return node.props;
    return findContent(node.props?.children);
  };
  const render = () => harness.render(() => MeetingDetails().props.children.type());
  return {
    ...harness, calls, summaries, render,
    emit(event, payload) { listeners.get(event)?.forEach(callback => callback({ payload })); },
    content() { return findContent(render()); },
    navigate(id) { meetingId = id; metadata = null; render(); },
    loadMetadata() { metadata = { id: meetingId, title: meetingId }; render(); },
  };
}

for (const incomplete of [false, true]) {
  test(`a capture gap stays visible with automatic notes ${incomplete ? 'paused for incomplete recognition' : 'available'}`, async () => {
    const view = createView({ outcome: { capture_incomplete: true, transcription_incomplete: incomplete } });
    view.render();
    view.loadMetadata();
    await flush();
    const alert = view.render().props.children.find(child => child?.props?.role === 'alert');
    const message = alert.props.children.filter(child => typeof child === 'string').join(' ');
    assert.match(message, /lost during capture/);
    assert.equal(/notes are paused/i.test(message), incomplete);
    assert.ok(view.content().meeting.transcripts.length);
    view.unmount();
  });
}

test('a restored meeting with a successful recording outcome and no folder has no recovery warning', async t => {
  const view = createView({ outcome: { audio_save_failed: false, transcription_incomplete: false } });
  t.after(view.unmount);
  view.render();
  view.loadMetadata();
  await flush();
  assert.equal(view.content().meeting.folder_path, undefined);
  assert.equal(view.render().props.children.some(child => child?.props?.role === 'alert'), false);
});

test('a summary that arrives after navigation cannot appear in or overwrite the next meeting', async () => {
  const view = createView();
  view.render();
  await flush();
  view.navigate('second');
  view.loadMetadata();
  await flush();
  view.summaries.get('second').resolve({ status: 'completed', data: { markdown: 'Second notes' } });
  await flush();
  view.summaries.get('first').resolve({ status: 'completed', data: { markdown: 'First notes' } });
  await flush();
  const content = view.content();
  assert.equal(content.meeting.id, 'second');
  assert.equal(content.summaryData.markdown, 'Second notes');
  view.unmount();
});

test('opening a recorded meeting displays an existing native summary', async () => {
  const view = createView({ fromRecording: true });
  view.render();
  view.loadMetadata();
  view.render();
  await flush();
  assert.equal(view.calls.some(call => call.command === 'api_get_model_config'), false);
  view.summaries.get('first').resolve({ status: 'completed', data: { markdown: 'Already saved' } });
  await flush();
  assert.equal(view.content().summaryData.markdown, 'Already saved');
  await flush();
  assert.equal(view.calls.some(call => call.command === 'api_get_model_config'), false);
  view.unmount();
});

test('transcript and progress updates keep the same editor data until the saved content changes', async t => {
  const view = createView();
  t.after(view.unmount);
  const snapshot = (notes, changed) => ({
    status: changed ? 'processing' : 'completed',
    transcript_changed: changed,
    data: {
      generation: { model: changed ? 'new-metadata' : 'old-metadata' },
      summary_json: [{
        id: 'note', type: 'paragraph', props: {}, children: [],
        content: [{ type: 'text', text: notes, styles: {} }],
      }],
    },
  });
  view.render();
  view.loadMetadata();
  await flush();
  view.summaries.get('first').resolve(snapshot('Saved notes', false));
  await flush();
  const original = view.content().summaryData;
  assert.equal(original.summary_json[0].content[0].text, 'Saved notes');

  view.emit('transcript-text-edited', { meeting_id: 'first' });
  view.summaries.get('first').resolve(snapshot('Saved notes', true));
  await flush();
  assert.equal(view.content().summaryJob.transcript_changed, true);
  assert.equal(view.content().summaryData, original);

  view.emit('summary-job-progress', 'first');
  view.summaries.get('first').resolve({
    ...snapshot('Saved notes', true),
    progress: { completed: 1, total: 3, level: 1 },
  });
  await flush();
  assert.equal(view.content().summaryJob.progress.completed, 1);
  assert.equal(view.content().summaryData, original);

  view.emit('summary-job-changed', 'first');
  view.summaries.get('first').resolve(snapshot('New notes', false));
  await flush();
  assert.notEqual(view.content().summaryData, original);
  assert.equal(view.content().summaryData.summary_json[0].content[0].text, 'New notes');
});

test('reopening a skipped recording displays its reason without starting a second job', async () => {
  const view = createView({ fromRecording: true });
  view.render();
  view.loadMetadata();
  await flush();
  view.summaries.get('first').resolve({ status: 'skipped', error: 'Automatic summaries are off.' });
  await flush();
  assert.equal(view.content().summaryJob.error, 'Automatic summaries are off.');
  assert.equal(view.calls.some(call => /generate|start_recording_summary|get_model_config/.test(call.command)), false);
  view.unmount();
});
