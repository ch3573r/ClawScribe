import assert from "node:assert/strict";
import test from "node:test";
import { loadTsModule } from "./load-ts-module.mjs";
import { createHookHarness, deferred, flush } from "./hook-harness.mjs";

const jsx = (type, props) => ({ type, props });
const nodes = (node) =>
  !node || typeof node !== "object"
    ? []
    : Array.isArray(node)
      ? node.flatMap(nodes)
      : [node, ...nodes(node.props?.children), ...nodes(node.props?.actions)];
const text = (node) =>
  typeof node === "string" || typeof node === "number"
    ? String(node)
    : Array.isArray(node)
      ? node.map(text).join("")
      : node
        ? text(node.props?.children)
        : "";
const project = { tags: ["Launch"], mode: "all", untagged: false };
const meetings = [
  { id: "a", title: "Meeting A" },
  { id: "b", title: "Meeting B" },
];
function component(file, name, props, invoke) {
  const hooks = createHookHarness();
  const exports = loadTsModule(file, {
    react: hooks.react,
    "react/jsx-runtime": { jsx, jsxs: jsx },
    "@tauri-apps/api/core": { invoke },
    "@tauri-apps/api/event": { listen: async () => () => {} },
    "@/components/Sidebar/SidebarProvider": {
      useSidebar: () => ({
        meetings,
        projectTags: [],
        projectTagsError: null,
        projectTagsLoading: false,
      }),
    },
    "@/contexts/ConfigContext": {
      useConfig: () => ({
        modelConfig: { provider: "fixture", model: "fixture" },
      }),
    },
    "@/components/ui/button": { Button: "button" },
    "@/components/ui/textarea": { Textarea: "textarea" },
    "@/components/ui/scroll-area": { ScrollArea: "scroll-area" },
    "@/components/ui/tooltip": {},
    "@/components/ui/input": { Input: "input" },
    "@/components/ui/checkbox": { Checkbox: "checkbox" },
    "@/components/ui/switch": { Switch: "switch" },
    "@/components/ui/progress": { Progress: "progress" },
    "@/components/ui/select": { Select: "select", SelectContent: "select-content", SelectItem: "select-item", SelectTrigger: "select-trigger", SelectValue: "select-value" },
    "@/components/ui/multi-select": { MultiSelect: "select" },
    "@/components/ui/popover": { Popover: "popover", PopoverTrigger: "trigger", PopoverContent: "content" },
    "@/components/ui/dialog": {
      Dialog: "dialog",
      DialogContent: "content",
      DialogDescription: "description",
      DialogHeader: "header",
      DialogTitle: "title",
      DialogTrigger: "trigger",
    },
    "@/components/ui/dropdown-menu": {},
    "@/components/Knowledge/EvidencePreview": { EvidencePreview: "preview" },
    "./EvidencePreview": { EvidencePreview: "preview" },
  });
  const render = () => hooks.render(() => exports[name](props));
  return {
    render,
    unmount: hooks.unmount,
    state: () => nodes(render()).find((node) => node.props?.state)?.props.state,
    check(label, checked) {
      const row = nodes(render()).find(
        (node) => node.type === "label" && text(node) === label,
      );
      const control=nodes(row).find(node=>["input","checkbox","switch"].includes(node.type));
      if(control.props.onCheckedChange) control.props.onCheckedChange(checked);
      else control.props.onChange({target:{checked}});
    },
    button(label) {
      return nodes(render()).find(
        (node) => node.type === "button" && text(node) === label,
      );
    },
  };
}

test("MeetingChat expands into a durable library owner and restores each history without late old-owner work", async () => {
  const requests = [],
    cancellations = [],
    historyOwners = [];
  const pending = deferred();
  let deferAnswer = false;
  const library = { kind: "library", id: "saved-library" };
  const app = component(
    "src/components/MeetingDetails/MeetingChat.tsx",
    "MeetingChat",
    { meetingId: "a", provider: "fixture", model: "fixture" },
    async (command, args) => {
      if (command === "knowledge_list_library_conversations") return [library];
      if (command === "knowledge_create_library_conversation") return library;
      if (command === "knowledge_cancel_request") {
        cancellations.push(args.requestId);
        return;
      }
      if (command === "knowledge_history") {
        historyOwners.push(args.owner.id);
        return [
          {
            id: args.owner.id,
            role: "assistant",
            content: args.owner.id,
            status: "completed",
            legacy: args.owner.kind === "meeting",
            reply: null,
          },
        ];
      }
      if (command === "knowledge_ask") {
        requests.push(JSON.parse(JSON.stringify(args.request)));
        return deferAnswer ? pending.promise : {};
      }
      throw new Error(`Unexpected command ${command}`);
    },
  );
  try {
    app.render();
    await flush();
    await app.state().ask("Original meeting question", "keyword");
    assert.deepEqual(requests[0].owner, { kind: "meeting", id: "a" });
    app.check("Include other saved meetings", true);
    app.render();
    await flush();
    await app.state().newConversation();
    app.render();
    await flush();
    app.check("Search all saved meetings", true);
    app.render();
    await flush();
    await app.state().ask("Cross meeting question", "keyword");
    assert.deepEqual(
      requests[1].owner,
      library,
      "expanded native request must use the durable library owner",
    );
    assert.equal(requests[1].search.scope.kind, "library");
    assert.deepEqual(requests[1].search.scope.filter.meeting_ids, []);
    assert.equal(app.state().messages[0].id, "saved-library");
    deferAnswer = true;
    const late = app.state().ask("Pending library question", "keyword");
    app.check("Include other saved meetings", false);
    assert.equal(
      app.state().messages.length,
      0,
      "old history hides synchronously",
    );
    await flush();
    const readCount = historyOwners.length;
    pending.resolve({});
    await late;
    assert.equal(
      historyOwners.length,
      readCount,
      "late library result cannot restore old-owner history",
    );
    assert.ok(cancellations.includes(requests[2].request_id));
    assert.equal(app.state().messages[0].id, "a");
    deferAnswer = false;
    await app.state().ask("Meeting again", "keyword");
    assert.deepEqual(requests[3].owner, { kind: "meeting", id: "a" });
    assert.deepEqual(requests[3].search.scope, {
      kind: "meeting",
      meeting_id: "a",
    });
    app.check("Include other saved meetings", true);
    app.render();
    await flush();
    assert.equal(app.state().owner.id, "saved-library");
    assert.equal(app.state().messages[0].id, "saved-library");
  } finally {
    pending.resolve({});
    app.unmount();
  }
});

test("archive selected-to-all transition removes ID restriction for search and answers and restores selection", async () => {
  const requests = [];
  const library = { kind: "library", id: "library" };
  const app = component(
    "src/components/Knowledge/KnowledgeArchive.tsx",
    "KnowledgeArchive",
    { meetings, projectFilter: project },
    async (command, args) => {
      if (command === "knowledge_list_library_conversations") return [library];
      if (command === "knowledge_create_library_conversation") return library;
      if (command === "knowledge_history") return [];
      if (command === "knowledge_search" || command === "knowledge_ask") {
        requests.push(
          JSON.parse(
            JSON.stringify(
              command === "knowledge_ask" ? args.request.search : args.request,
            ),
          ),
        );
        return { passages: [], mode: "keyword", index_status: {} };
      }
      if (command === "knowledge_cancel_request") return;
      throw new Error(`Unexpected command ${command}`);
    },
  );
  try {
    app.render();
    await flush();
    app.check("Meeting A", true);
    app.render();
    await flush();
    await app.state().search("decision", "keyword");
    assert.deepEqual(requests[0].scope.filter.meeting_ids, ["a"]);
    app.check("Search all saved meetings", true);
    app.render();
    await flush();
    await app.state().newConversation();
    app.render();
    await flush();
    await app.state().search("decision", "keyword");
    await app.state().ask("decision", "keyword");
    for (const request of requests.slice(1)) {
      assert.deepEqual(request.scope.filter, {
        all_meetings: true,
        meeting_ids: [],
        tags: ["Launch"],
        tag_mode: "all",
        untagged: false,
        from: null,
        to: null,
      });
    }
    app.check("Search all saved meetings", false);
    app.render();
    await flush();
    await app.state().search("decision", "keyword");
    assert.deepEqual(requests[3].scope.filter.meeting_ids, ["a"]);
  } finally {
    app.unmount();
  }
});

test("status polling clears a recovered read error without clearing a failed action", async () => {
  let failRead = true;
  const app = component(
    "src/components/KnowledgeSettings.tsx",
    "KnowledgeSettings",
    {},
    async (command) => {
      if (command === "knowledge_index_status") {
        if (failRead) throw new Error("transient read");
        return {
          keyword_ready: true,
          semantic_ready: 0,
          pending: 0,
          failed: 0,
          semantic_enabled: false,
          reason: null,
        };
      }
      if (command === "knowledge_model_status")
        return { model: "fixture", enabled: false, ready: false, installed: false,
          download: {stage: "idle", downloaded_bytes: 0, total_bytes: 487351240, current_file: null, error: null} };
      if (command === "knowledge_model_download") throw "Download failed";
      throw new Error(`Unexpected command ${command}`);
    },
  );
  try {
    app.render();
    await flush();
    assert.match(text(app.render()), /Could not read meeting memory status/);
    // Invoke the scheduled poll itself; manual Refresh previously hid the bug.
    // The same refresh closure is captured by the component's interval.
    failRead = false;
    await new Promise((resolve) => setTimeout(resolve, 2600));
    assert.doesNotMatch(text(app.render()), /Could not read meeting memory status/);
    app.button("Download search model").props.onClick();
    await flush();
    assert.match(text(app.render()), /Download failed/);
    await new Promise((resolve) => setTimeout(resolve, 2600));
    assert.match(text(app.render()), /Download failed/);
  } finally {
    app.unmount();
  }
});
