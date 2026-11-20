import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

test('Planner and To Do send the original preview identities with a filtered, edited selection', async () => {
  const calls = [];
  const { microsoftExportService, taskPreviewCandidates } = loadTsModule(
    'src/services/microsoftExportService.ts',
    { '@tauri-apps/api/core': { invoke: async (command, payload) => {
      calls.push([command, JSON.parse(JSON.stringify(payload))]);
    } } },
  );
  const originalTitleHash = '0123456789abcdef';
  const rows = ['action-1', 'action-2', 'action-3'].map((localId, index) => ({
    localId,
    originalTitleHash,
    title: 'Same task',
    bucketId: index === 2 ? 'other-bucket' : 'bucket',
    owner: null,
    dueDate: null,
  }));
  const candidates = taskPreviewCandidates(rows);
  rows[0].title = 'Renamed and deselected';
  rows[1].title = 'Renamed and selected';
  rows.shift();
  const plannerTasks = rows;
  const todoTasks = plannerTasks.slice(0, 1).map(({ bucketId, ...task }) => task);
  const previewCandidates = ['action-1', 'action-2', 'action-3'].map((localId) => ({
    localId, originalTitleHash,
  }));

  await microsoftExportService.exportSelectedPlannerTasks('meeting', 'Test meeting', 'plan', plannerTasks, candidates);
  await microsoftExportService.exportSelectedToDoTasks('meeting', 'Test meeting', 'list', todoTasks, candidates);
  assert.deepEqual(calls, [
    ['export_selected_planner_tasks', { meetingId: 'meeting', meetingTitle: 'Test meeting', planId: 'plan', tasks: plannerTasks, previewCandidates }],
    ['export_selected_todo_tasks', { meetingId: 'meeting', meetingTitle: 'Test meeting', listId: 'list', tasks: todoTasks, previewCandidates }],
  ]);
});

// ── taskIdentityReviewIds ────────────────────────────────────────────────────

test('taskIdentityReviewIds collects only held-row local IDs', () => {
  const { taskIdentityReviewIds, TASK_IDENTITY_REVIEW_CODE } = loadTsModule(
    'src/services/microsoftExportService.ts',
    { '@tauri-apps/api/core': { invoke: async () => {} } },
  );
  assert.equal(TASK_IDENTITY_REVIEW_CODE, 'task_identity_review_required');
  const report = {
    overall: 'partial',
    connectionState: null,
    items: [
      { dedupeKey: 'k1', localId: 'uuid-succeeded', status: 'succeeded', resourceId: 'task-1', webUrl: null, code: null, graphCalled: true },
      { dedupeKey: 'k2', localId: 'uuid-held', status: 'failed', resourceId: null, webUrl: null, code: 'task_identity_review_required', graphCalled: false },
      { dedupeKey: 'k3', localId: 'uuid-also-failed', status: 'failed', resourceId: null, webUrl: null, code: 'some_other_code', graphCalled: true },
    ],
  };
  const ids = taskIdentityReviewIds(report);
  assert.ok(ids.has('uuid-held'), 'must include held action');
  assert.ok(!ids.has('uuid-succeeded'), 'succeeded must not be included');
  assert.ok(!ids.has('uuid-also-failed'), 'other failure code must not be included');
  assert.equal(ids.size, 1);
});

test('taskIdentityReviewIds returns empty set when no held rows', () => {
  const { taskIdentityReviewIds } = loadTsModule(
    'src/services/microsoftExportService.ts',
    { '@tauri-apps/api/core': { invoke: async () => {} } },
  );
  const report = {
    overall: 'succeeded',
    connectionState: null,
    items: [
      { dedupeKey: 'k1', localId: 'uuid-a', status: 'succeeded', resourceId: 'task-1', webUrl: null, code: null, graphCalled: true },
    ],
  };
  const ids = taskIdentityReviewIds(report);
  assert.equal(ids.size, 0);
});

test('TASK_IDENTITY_REVIEW_MESSAGE guides user to check destination before creating', () => {
  const { TASK_IDENTITY_REVIEW_MESSAGE } = loadTsModule(
    'src/services/microsoftExportService.ts',
    { '@tauri-apps/api/core': { invoke: async () => {} } },
  );
  assert.ok(
    TASK_IDENTITY_REVIEW_MESSAGE.toLowerCase().includes('destination'),
    'message must mention the destination',
  );
  assert.ok(
    TASK_IDENTITY_REVIEW_MESSAGE.toLowerCase().includes('deselect') ||
    TASK_IDENTITY_REVIEW_MESSAGE.toLowerCase().includes('skip'),
    'message must guide to deselect/skip already-created tasks',
  );
  assert.ok(
    TASK_IDENTITY_REVIEW_MESSAGE.toLowerCase().includes('meeting actions') ||
    TASK_IDENTITY_REVIEW_MESSAGE.toLowerCase().includes('add action'),
    'message must mention adding a new action for genuinely new tasks',
  );
});
