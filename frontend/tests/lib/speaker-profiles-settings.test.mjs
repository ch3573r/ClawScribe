import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, flush } from "./hook-harness.mjs";
import { loadTsModule } from "./load-ts-module.mjs";

test("turning off profiles confirms the number of voices before deleting them", async () => {
  const hooks = createHookHarness();
  const changes = [];
  const jsx = (type, props) => ({ type, props });
  const { SpeakerProfilesSettings } = loadTsModule("src/components/settings/SpeakerProfilesSettings.tsx", {
    react: { ...hooks.react, useId: () => "profiles" },
    "react/jsx-runtime": { jsx, jsxs: jsx, Fragment: "Fragment" },
    "@/components/ui/button": { Button: "Button" },
    "@/components/ui/dialog": Object.fromEntries(
      ["Dialog", "DialogContent", "DialogDescription", "DialogFooter", "DialogHeader", "DialogTitle"].map(name => [name, name]),
    ),
    "@/components/ui/settings-layout": { SettingsCard: "SettingsCard", SettingsRow: "SettingsRow" },
    "@/components/ui/switch": { Switch: "Switch" },
    "@tauri-apps/api/core": { invoke: async (command, args) => {
      if (command === "get_speaker_profile_settings") return {
        preferences: { enabled: true, autoApply: true },
        profiles: [{ key: "ada", name: "Ada" }, { key: "grace", name: "Grace" }],
      };
      assert.equal(command, "set_speaker_profile_preferences");
      changes.push({ ...args });
      return { preferences: { enabled: false, autoApply: false }, cleanupPending: false };
    } },
  });

  function nodes(value) {
    if (Array.isArray(value)) return value.flatMap(nodes);
    return value?.props ? [value, ...nodes(value.props.children)] : [];
  }
  const render = () => nodes(hooks.render(SpeakerProfilesSettings));
  const toggle = () => render().find(node => node.type === "Switch" && node.props.id === "profiles");
  try {
    render();
    await flush();
    toggle().props.onCheckedChange(false);
    assert.equal(render().find(node => node.type === "DialogDescription").props.children, "Turning this off deletes 2 saved voices.");
    assert.equal(changes.length, 0);

    render().find(node => node.type === "Button" && node.props.children === "Cancel").props.onClick();
    assert.equal(toggle().props.checked, true);
    toggle().props.onCheckedChange(false);
    render().find(node => node.type === "Button" && node.props.children === "Turn off and delete").props.onClick();
    await flush();
    assert.deepEqual(changes, [{ enabled: false, autoApply: false }]);
    assert.equal(toggle().props.checked, false);
    assert.ok(!render().some(node => node.props["aria-label"] === "Delete the profile for Ada"));
  } finally {
    hooks.unmount();
  }
});
