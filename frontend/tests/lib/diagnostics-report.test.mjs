import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, deferred, flush } from "./hook-harness.mjs";
import { loadTsModule } from "./load-ts-module.mjs";

const jsx = (type, props) => ({ type, props });
const snapshot = {
  app: { version: "1.0.0", os: "windows", arch: "x86_64", buildProfile: "release", logsDir: "private report path" },
  transcription: { selectedProvider: "parakeet", selectedModel: "test model", engines: [] },
  audio: { inputDevices: 1, outputDevices: 1, totalDevices: 2, deviceNames: ["private device name"] },
  recording: { session_id: "private meeting ID" },
  diarization: { preferredProvider: "cpu", runtimeDlls: [] },
  integrations: {},
  errors: [],
};

function findButton(node, label) {
  if (Array.isArray(node)) {
    return node.map((child) => findButton(child, label)).find(Boolean);
  }
  if (!node || typeof node !== "object") return undefined;
  if (node.type === "button" && node.props.children.includes(label)) return node;
  return findButton(node.props.actions, label) ?? findButton(node.props.children, label);
}

function setup() {
  const hooks = createHookHarness();
  const collection = deferred();
  const calls = [];
  const feedback = [];
  let save = deferred();
  const { DiagnosticsSettings, sanitizeDiagnosticsForCopy } = loadTsModule("src/components/settings/DiagnosticsSettings.tsx", {
    "react": { ...hooks.react, useSyncExternalStore: () => ({ sources: [], issues: {} }) },
    "react/jsx-runtime": { jsx, jsxs: jsx },
    "@tauri-apps/api/core": {
      invoke: (...args) => {
        calls.push(args);
        if (args[0] === "get_diagnostics_snapshot") return collection.promise;
        if (args[0] === "save_diagnostics_report") return save.promise;
        return Promise.resolve(null);
      },
    },
    "sonner": {
      toast: {
        success: (...args) => feedback.push(["success", ...args]),
        error: (...args) => feedback.push(["error", ...args]),
      },
    },
    "lucide-react": {},
    "@/components/ui/button": { Button: "button" },
    "@/components/ui/accordion": {},
    "@/components/ui/settings-layout": { SettingsCard: "card", SettingsColumns: "columns", SettingsFooter: "footer" },
    "@/components/settings/settings-parts": {},
    "@/components/settings/call-detection-check": {},
    "@/lib/utils": {},
    "@/lib/parakeet": { getModelDisplayName: (name) => name },
    "@/lib/autoRecord": {},
    "@/services/icalService": {},
  });
  return {
    calls,
    feedback,
    collect: async () => { collection.resolve(snapshot); await flush(); },
    render: () => hooks.render(DiagnosticsSettings),
    save: () => save,
    resetSave: () => { save = deferred(); },
    sanitizeDiagnosticsForCopy,
  };
}

test("diagnostics report only saves on request and sends neither snapshot data nor destination to the core", async () => {
  const harness = setup();
  assert.equal(findButton(harness.render(), "Save report").props.disabled, true);
  await harness.collect();
  const tree = harness.render();
  assert.equal(harness.calls.some(([command]) => command === "save_diagnostics_report"), false);

  const copy = findButton(tree, "Copy diagnostics");
  assert.equal(copy.props.disabled, false);
  assert.equal(harness.sanitizeDiagnosticsForCopy(snapshot).audio.deviceNames[0], "private device name");

  const button = findButton(tree, "Save report");
  const pending = button.props.onClick();
  await button.props.onClick();
  assert.equal(findButton(harness.render(), "Saving…").props.disabled, true);
  assert.deepEqual(harness.calls.filter(([command]) => command === "save_diagnostics_report"), [["save_diagnostics_report"]]);

  harness.save().resolve(true);
  await pending;
  assert.deepEqual(harness.feedback, [["success", "Diagnostics report saved"]]);
  assert.equal(findButton(harness.render(), "Save report").props.disabled, false);
});

test("cancel is quiet and save failure gives a generic retry message", async () => {
  const harness = setup();
  harness.render();
  await harness.collect();

  const pending = findButton(harness.render(), "Save report").props.onClick();
  harness.save().resolve(false);
  await pending;
  assert.deepEqual(harness.feedback, []);

  harness.resetSave();
  const retry = findButton(harness.render(), "Save report").props.onClick();
  harness.save().reject(new Error("private destination and provider response"));
  await retry;
  assert.equal(harness.feedback.length, 1);
  assert.equal(harness.feedback[0][0], "error");
  assert.match(JSON.stringify(harness.feedback[0]), /choose.*folder|try again/i);
  assert.doesNotMatch(JSON.stringify(harness.feedback[0]), /private/);
  assert.equal(findButton(harness.render(), "Save report").props.disabled, false);
});
