import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, deferred, flush } from "./hook-harness.mjs";
import { loadTsModule } from "./load-ts-module.mjs";

const jsx = (type, props) => ({ type, props });
const nodes = (node) => Array.isArray(node) ? node.flatMap(nodes)
  : node?.props ? [node, ...nodes(node.props.children), ...nodes(node.props.actions)] : [];
const text = (node) => Array.isArray(node) ? node.map(text).join("")
  : node?.props ? text(node.props.children) : typeof node === "string" ? node : "";
const button = (tree, label) => nodes(tree).find(node => node.type === "Button" && text(node) === label);
const input = (tree, id) => nodes(tree).find(node => node.type === "Input" && node.props.id === id);

const defaults = {
  revision: null, configured: false, destination: null, enabled: false,
  onSummary: true, onRecording: false, includeSummary: true, includeTranscript: false,
  allowUnencrypted: false, signingSecret: { stored: false, hint: null },
  configurationError: null, lastDelivery: null,
};

function harness(state, invoke = async () => undefined) {
  const hooks = createHookHarness();
  const { WebhookSettings } = loadTsModule("src/components/settings/WebhookSettings.tsx", {
    react: hooks.react,
    "react/jsx-runtime": { jsx, jsxs: jsx, Fragment: "fragment" },
    "@tauri-apps/api/core": { invoke },
    sonner: { toast: { success() {} } },
    "@/hooks/useWebhookSettings": { useWebhookSettings: () => state },
    "@/components/UnencryptedHttpOptIn": { UnencryptedHttpOptIn: "HttpOptIn" },
    "@/components/ui/button": { Button: "Button" },
    "@/components/ui/input": { Input: "Input" },
    "@/components/ui/switch": { Switch: "Switch" },
    "@/components/ui/checkbox": { Checkbox: "Checkbox" },
    "@/components/ui/settings-layout": {
      SettingsStack: "Stack", SettingsCard: "Card", SettingsRow: "Row", SettingsFooter: "Footer",
    },
  });
  const panel = () => hooks.render(WebhookSettings);
  const child = panel().props.children;
  return { ...hooks, panel, form: () => hooks.render(() => child.type(child.props)) };
}

test("webhook settings show loading and read errors, then require a URL before enabling", () => {
  assert.match(text(harness({ status: "loading" }).panel()), /Loading webhook settings/);
  assert.match(text(harness({ status: "error" }).panel()), /Could not load webhook settings/);
  const app = harness({ status: "ready", value: defaults });
  let tree = app.form();
  assert.match(text(tree), /No deliveries yet/);
  assert.equal(button(tree, "Send test"), undefined);
  assert.equal(button(tree, "Remove"), undefined);
  nodes(tree).find(node => node.props.id === "webhook-enabled").props.onCheckedChange(true);
  tree = app.form();
  assert.equal(button(tree, "Save").props.disabled, true);
  input(tree, "webhook-url").props.onChange({ target: { value: "http://hooks.local/example" } });
  tree = app.form();
  assert.equal(button(tree, "Save").props.disabled, true);
  nodes(tree).find(node => node.type === "HttpOptIn").props.onChange(true);
  assert.equal(button(app.form(), "Save").props.disabled, false);
  app.unmount();
});

test("configured webhook shows only the origin and keeps delivery controls disabled while the test runs", async () => {
  const response = deferred();
  const calls = [];
  const app = harness({ status: "ready", value: {
    ...defaults, revision: "example-revision", destination: "https://hooks.example.com",
    configured: true, enabled: true, signingSecret: { stored: true, hint: "abcd" },
  } }, (command, args) => {
    calls.push([command, args.revision]);
    return response.promise;
  });
  let tree = app.form();
  assert.match(text(tree), /https:\/\/hooks\.example\.com\/••••/);
  assert.equal(input(tree, "webhook-url").props.value, "");
  assert.equal(input(tree, "webhook-secret").props.value, "");
  button(tree, "Send test").props.onClick();
  tree = app.form();
  assert.equal(button(tree, "Sending test…").props.disabled, true);
  assert.equal(button(tree, "Remove").props.disabled, true);
  response.resolve({ outcome: "failed", httpStatus: 307, errorClass: "redirect" });
  await flush();
  assert.match(text(app.form()), /Delivery failed \(HTTP 307\).*redirected/);
  assert.deepEqual(calls, [["send_webhook_test", "example-revision"]]);
  app.unmount();
});
