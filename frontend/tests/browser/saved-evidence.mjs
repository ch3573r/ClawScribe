import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';

// From frontend/, run pnpm run build, then serve out/ on loopback port 3167 with clean URLs
// (/meetings and /meeting-details must resolve to their exported .html files).
// Playwright is an optional local tool. Set PLAYWRIGHT_MODULE to its entry file if needed.
// CLAWSCRIBE_BROWSER can select an installed Chromium-based browser for the headless run.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
  ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
  : 'playwright');
const origin = process.env.CLAWSCRIBE_TEST_URL ?? 'http://127.0.0.1:3167';
const date = '2026-01-01T12:00:00Z';
const meeting = { id: 'fixture', title: 'Synthetic meeting', created_at: date, updated_at: date, folder_path: 'fixture' };
const owner = { kind: 'library', id: 'fixture-conversation', first_question: 'What did we decide?', created_at: date };
const targetId = 'row-105';
const rows = Array.from({ length: 105 }, (_, index) => ({
  id: `row-${index + 1}`, text: index === 104 ? 'Verified late passage.' : `Earlier passage ${index + 1}.`,
  timestamp: date, audio_start_time: index, audio_end_time: index + 1,
}));
const reference = { source_id: 'fixture', source_revision: 1, chunk_id: 'fixture-passage', fingerprint: 'fixture-fingerprint',
  locator: { kind: 'transcript', meeting_id: meeting.id, transcript_ids: [targetId], spans: [], start_seconds: 104 } };
const metadata = { title: meeting.title, date, speaker: null, metadata_truncated: false };
const evidence = { status: 'current', passage: { ...metadata, evidence: reference, meeting_id: meeting.id, text: rows.at(-1).text, rank: 1 },
  navigation: { meeting_id: meeting.id, transcript_id: targetId, transcript_index: 104, start_seconds: 104 } };
const answer = `A verified answer [K1].\n\n${'syntheticlabel'.repeat(28)}`;
const message = { id: 'answer', role: 'assistant', created_at: date, status: 'completed', legacy: false, content: answer,
  reply: { request_id: 'fixture-request', message_id: 'answer', content: answer, evidence: [reference], evidence_metadata: [metadata],
    cited_tags: [1], retrieval_mode: 'keyword', model: 'fixture-model', provider: 'ollama' } };

const browser = await chromium.launch({ executablePath: process.env.CLAWSCRIBE_BROWSER });
const failures = [];
const viewports = [
  { width: 768, height: 380 },
  { width: 960, height: 490 },
  { width: 800, height: 620 },
  { width: 1440, height: 860 },
];
try {
  for (const viewport of viewports) {
    const page = await browser.newPage({ viewport, deviceScaleFactor: 2.5, colorScheme: viewport.width <= 800 ? 'dark' : 'light' });
    if (process.env.CLAWSCRIBE_TEST_DEBUG) {
      page.on('console', message => { if (message.type() === 'error' || message.type() === 'warning') console.error(message.text()); });
      page.on('pageerror', error => console.error(error.stack?.split('\n').slice(0, 2).join('\n')));
    }
    page.setDefaultTimeout(20000);
    await page.route('**/*', route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
    await page.addInitScript(({ meeting, owner, rows, reference, evidence, message }) => {
      const log = { pageOffsets: [], seek: [], played: 0, stale: false };
      window.fixture = log;
      window.Audio = class extends EventTarget {
        duration = 112;
        time = 0;
        get currentTime() { return this.time; }
        set currentTime(value) { this.time = value; log.seek.push(value); }
        load() { setTimeout(() => this.dispatchEvent(new Event('loadedmetadata')), 75); }
        async play() { log.played++; this.dispatchEvent(new Event('playing')); }
        pause() { this.dispatchEvent(new Event('pause')); }
        removeAttribute() {}
      };
      let eventId = 0;
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { windowLabel: 'main', label: 'main' } },
        transformCallback() { return ++eventId; },
        convertFileSrc() { return '/fixture.wav'; },
        async invoke(command, args) {
          switch (command) {
            case 'startup_recovery_active': return true;
            case 'get_recording_state': return { is_recording: false, is_paused: false, is_active: false };
            case 'get_onboarding_status': return { completed: true };
            case 'api_get_meetings': return [meeting];
            case 'api_get_meeting_metadata': return meeting;
            case 'api_get_meeting_transcripts': {
              log.pageOffsets.push(args.offset);
              const transcripts = rows.slice(args.offset, args.offset + args.limit);
              return { transcripts, total_count: rows.length, has_more: args.offset + transcripts.length < rows.length };
            }
            case 'api_get_meeting_transcript_timeline': return rows;
            case 'api_get_transcript_edit_state': return { can_undo: false, can_restore_previous: false, pending_file_sync: false, has_edits: false };
            case 'resolve_meeting_audio_file': return meeting.folder_path;
            case 'knowledge_list_library_conversations': return [owner];
            case 'knowledge_library_conversation_scope': return { all_meetings: false, meeting_ids: [meeting.id], tags: [], tag_mode: 'any', untagged: false, from: null, to: null };
            case 'knowledge_history': return [message];
            case 'knowledge_search': return { passages: [evidence.passage], mode: 'keyword', index_status: { reason: null } };
            case 'knowledge_resolve_evidence': return args.reference.fingerprint === reference.fingerprint && !log.stale
              ? evidence : { status: 'stale', passage: null, navigation: null };
            case 'api_get_model_config': return { provider: 'ollama', model: 'fixture-model', whisperModel: 'base' };
            case 'api_get_summary': return { status: 'idle', data: null };
            case 'knowledge_index_status': return { keyword_ready: true, semantic_enabled: false, semantic_ready: 0 };
            case 'list_meeting_tags':
            case 'api_list_templates':
            case 'list_saved_live_answers':
            case 'get_ollama_models':
            case 'knowledge_list_scope_documents':
            case 'knowledge_list_documents':
            case 'list_meeting_bookmarks': return [];
            case 'get_openclaw_submission_status': return { enabled: false };
            case 'plugin:event|listen': return ++eventId;
            default: return null;
          }
        },
      };
    }, { meeting, owner, rows, reference, evidence, message });

    const check = async (name, verify) => {
      try { await verify(); }
      catch (error) {
        const message = `${viewport.width}×${viewport.height} ${name}: ${error.message}`;
        failures.push(message);
        console.error(message);
      }
    };
    async function openConversation() {
      await page.getByRole('tab', { name: 'Meeting memory' }).click();
      await page.getByRole('button', { name: 'Conversations', exact: true }).click();
      await page.getByRole('menuitem', { name: /What did we decide/ }).click();
      await page.getByRole('button', { name: 'Open source 1', exact: true }).waitFor();
    }
    const chat = page.getByRole('region', { name: 'Ask about these meetings' });
    const citation = page.getByRole('button', { name: 'Open source 1', exact: true });
    const dialog = page.getByRole('dialog', { name: 'Source passage' });
    async function openCitation(activate = () => citation.click()) {
      await activate();
      await dialog.getByText('Current verified source.').waitFor();
    }
    async function dismissEvidence(trigger) {
      await page.keyboard.press('Escape');
      await dialog.waitFor({ state: 'hidden' });
      await page.waitForFunction(element => document.activeElement === element, trigger, { timeout: 1500 });
    }

    await page.goto(`${origin}/meetings`);
    await openConversation();
    const wholeCardScrolls = await chat.evaluate(node => getComputedStyle(node).overflowY === 'auto');
    const scroller = wholeCardScrolls ? chat : chat.locator('[data-radix-scroll-area-viewport]');
    await check('conversation scrollport', async () => {
      const size = await scroller.evaluate(node => ({ width: node.clientWidth, height: node.clientHeight, scrollWidth: node.scrollWidth }));
      assert.ok(size.height >= 112, `conversation scrollport is only ${size.height}px tall`);
      assert.equal(size.scrollWidth, size.width, 'conversation content clips horizontally');
      console.log(`${viewport.width}×${viewport.height}: ${size.width}×${size.height} ${wholeCardScrolls ? 'card' : 'conversation'} scrollport`);
    });
    const citationTrigger = await citation.elementHandle();
    await openCitation();
    await check('Escape returns focus to the citation', () => dismissEvidence(citationTrigger));

    for (const [index, action] of ['Show in transcript', 'Play from here', 'Show in transcript'].entries()) {
      await openCitation(index === 0 ? () => page.keyboard.press('Enter') : undefined);
      await dialog.getByRole('button', { name: action }).click();
      await page.waitForURL('**/meeting-details?**');
      const target = page.locator(`#segment-${targetId}`);
      const header = page.locator('main header h1').filter({ hasText: meeting.title });
      await header.first().waitFor();
      await page.getByRole('region', { name: 'Recording playback' }).first().waitFor();
      await check(`${action} renders the verified row`, async () => {
        const { top, bottom } = await target.evaluate(node => new Promise((resolve, reject) => {
          if (!node.textContent.includes('Verified late passage.')) { reject(new Error('Verified transcript text is absent.')); return; }
          const bounds = () => {
            const { top, bottom } = node.getBoundingClientRect();
            return { top: Math.round(top), bottom: Math.round(bottom) };
          };
          const observer = new IntersectionObserver(([entry]) => {
            if (entry.intersectionRatio >= 0.9) { clearTimeout(timer); observer.disconnect(); resolve(bounds()); }
          }, { threshold: 0.9 });
          const timer = setTimeout(() => {
            observer.disconnect();
            const { top, bottom } = bounds();
            reject(new Error(`row spans ${top} to ${bottom} in a ${innerHeight}px viewport`));
          }, 1800);
          observer.observe(node);
        }), null, { timeout: 3000 });
        assert.ok(await page.locator('[data-transcript-panel]').evaluate(node => document.activeElement === node));
        if (index < 2) console.log(`${viewport.width}×${viewport.height}: ${action} row at y=${top}–${bottom}`);
      });
      await check('single rendered meeting header', async () => assert.equal(await header.count(), 1));
      if (action === 'Play from here') {
        await page.waitForFunction(() => window.fixture.played === 1);
        assert.equal(await page.evaluate(() => window.fixture.seek.at(-1)), 104);
      }
      await page.getByRole('button', { name: 'Meetings', exact: true }).click();
      await page.waitForURL('**/meetings');
      await openConversation();
    }
    assert.ok(await page.evaluate(() => window.fixture.pageOffsets.includes(100)), 'citation must load the second transcript page');
    const staleTrigger = await citation.elementHandle();
    await openCitation();
    await page.evaluate(() => { window.fixture.stale = true; });
    await dialog.getByRole('button', { name: 'Show in transcript' }).press('Enter');
    await dialog.getByText(/reference is historical or the source changed/).waitFor();
    assert.ok(await dialog.getByRole('button', { name: 'Show in transcript' }).isDisabled());
    assert.ok(await dialog.getByRole('button', { name: 'Play from here' }).isDisabled());
    assert.equal(new URL(page.url()).pathname, '/meetings');
    await check('Escape returns focus after stale-source revalidation', () => dismissEvidence(staleTrigger));

    await check('pointer and keyboard can reach the composer', async () => {
      const question = chat.getByRole('textbox', { name: 'Question' });
      const ask = chat.getByRole('button', { name: 'Ask', exact: true });
      await question.click();
      await page.keyboard.type('Follow-up question');
      await page.keyboard.press('Shift+Enter');
      await page.keyboard.type('Second line');
      assert.equal(await question.inputValue(), 'Follow-up question\nSecond line');
      assert.ok(await ask.isEnabled());
      await page.keyboard.press('Tab');
      await page.waitForFunction(element => {
        const { x, y, width, height } = element.getBoundingClientRect();
        return document.activeElement === element && element.contains(document.elementFromPoint(x + width / 2, y + height / 2));
      }, await ask.elementHandle(), { timeout: 1500 });
    });

    await page.getByRole('button', { name: 'Date range', exact: true }).click();
    await page.getByLabel('From', { exact: true }).waitFor();
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Reference documents and source search' }).click();
    await page.getByRole('button', { name: 'Source search', exact: true }).click();
    const searchInput = page.getByRole('textbox', { name: 'Search sources' });
    await searchInput.click();
    await page.keyboard.type('late passage');
    await page.keyboard.press('Enter');
    const inspect = page.getByRole('button', { name: 'Inspect passage' });
    const searchTrigger = await inspect.elementHandle();
    await inspect.click();
    await dialog.getByText(/reference is historical or the source changed/).waitFor();
    await check('Escape returns focus to the search result', () => dismissEvidence(searchTrigger));
    await check('mouse wheel scrolls the active scrollport', async () => {
      await scroller.hover();
      const { scrollTop, scrollHeight, clientHeight } = await scroller.evaluate(node => ({ scrollTop: node.scrollTop, scrollHeight: node.scrollHeight, clientHeight: node.clientHeight }));
      assert.ok(scrollHeight > clientHeight, 'fixture must overflow the scrollport');
      await page.mouse.wheel(0, scrollTop > 0 ? -100 : 100);
      await page.waitForFunction(({ element, before }) => element.scrollTop !== before, { element: await scroller.elementHandle(), before: scrollTop }, { timeout: 1500 });
    });
    await page.close();
  }
} finally {
  await browser.close();
}
assert.deepEqual(failures, []);
console.log('Saved evidence passed at all four viewports.');
