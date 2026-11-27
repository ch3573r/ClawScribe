import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, flush } from "./hook-harness.mjs";
import { loadTsModule } from "./load-ts-module.mjs";

const jsx = (type, props) => ({ type, props });
const jsxRuntime = { jsx, jsxs: jsx, Fragment: "Fragment" };
const components = names => Object.fromEntries(names.split(" ").map(name => [name, name]));
const dialog = components("Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle");

function find(tree, predicate) {
  if (Array.isArray(tree)) return tree.map(child => find(child, predicate)).find(Boolean);
  if (!tree || typeof tree !== "object") return undefined;
  return predicate(tree) ? tree : find(tree.props?.children, predicate);
}

test("moving meetings to Trash saves pending editor drafts and resets the active meeting", async t => {
  const hooks = createHookHarness();
  t.after(hooks.unmount);
  const calls = [];
  const invoke = async (command, args) => {
    calls.push([command, args]);
    return {};
  };
  const drafts = loadTsModule("src/lib/meetingDrafts.ts", { "@tauri-apps/api/core": { invoke }, sonner: {} });
  const meeting = { id: "deleted", title: "Meeting" };
  const draft = drafts.meetingDraft(meeting.id);
  t.after(() => drafts.discardMeetingDraft(meeting.id));
  draft.edit({ title: "Pending title" });
  await flush();
  assert.equal(calls.at(-1)[1].pending, true);
  let current = meeting;
  const { ArchiveBulkActions } = loadTsModule("src/components/Archive/ArchiveBulkActions.tsx", {
    react: hooks.react, "react/jsx-runtime": jsxRuntime, "@tauri-apps/api/core": { invoke },
    "@/components/ui/button": components("Button"), "@/components/ui/checkbox": components("Checkbox"),
    "@/components/ui/dialog": dialog, "@/components/ui/input": components("Input"),
    "@/components/ui/select": components("Select SelectContent SelectItem SelectTrigger SelectValue"),
    "@/components/Sidebar/SidebarProvider": { useSidebar: () => ({
      setCurrentMeeting: update => { current = update(current); }, setMeetings() {},
    }) },
    "@/contexts/RecordingStateContext": { useRecordingState: () => ({}) },
    "@/lib/meetingDrafts": drafts, "@/lib/library": {},
    "./archive-bulk-export": {},
  });
  const tree = hooks.render(() => ArchiveBulkActions({
    action: "delete", meetings: [meeting], onDeleted() {}, onClose() {}, onReturnFocus() {},
  }));
  find(tree, node => node.type === "form").props.onSubmit({ preventDefault() {} });
  await flush();
  assert.equal(draft.dirty(), false);
  const save = calls.findIndex(([command]) => command === "api_save_meeting_title");
  const move = calls.findIndex(([command]) => command === "api_delete_meeting");
  assert.ok(save >= 0 && move > save);
  assert.deepEqual({ ...calls[save][1] }, { meetingId: meeting.id, title: "Pending title" });
  assert.equal(calls.findLast(([command]) => command === "api_set_summary_edits_pending")[1].pending, false);
  assert.equal(current, null);
});

test("bulk vocabulary errors identify the invalid term within the original line", () => {
  const { mergeVocabulary } = loadTsModule("src/lib/vocabulary-list.ts");
  assert.throws(() => mergeVocabulary([], "Alpha\r\n\r\nBeta, +++", true), /Line 3, term 2.*at least one letter or number/);
});

test("an open alert dialog blocks the command palette shortcut", t => {
  const hooks = createHookHarness();
  const priorWindow = globalThis.window;
  const priorDocument = globalThis.document;
  const priorEvent = globalThis.CustomEvent;
  let onKey, opened = 0;
  globalThis.window = {
    addEventListener(_name, callback) { onKey = callback; }, removeEventListener() {},
    dispatchEvent() { opened++; },
  };
  globalThis.document = { querySelector: selector => selector.includes('[role="alertdialog"][data-state="open"]') };
  globalThis.CustomEvent = class {};
  t.after(() => {
    hooks.unmount();
    Object.assign(globalThis, { window: priorWindow, document: priorDocument, CustomEvent: priorEvent });
  });
  const { AppShortcuts } = loadTsModule("src/components/AppShortcuts.tsx", {
    react: hooks.react, "next/navigation": { useRouter: () => ({}) },
  });
  hooks.render(AppShortcuts);
  onKey({ key: "k", ctrlKey: true, preventDefault() {} });
  assert.equal(opened, 0);
});
