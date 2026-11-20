import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { createHookHarness, flush } from "./hook-harness.mjs";
import { loadTsModule } from "./load-ts-module.mjs";

// The core owns the behaviour settings. These tests use a fake core behind
// `invoke`, with the same rules as the real one: values live in the core, the
// old webview values are handed over once and only where the core has none.

function fakeCore(initial = {}) {
  const core = {
    values: { parakeetDirectml: null, parakeetWorker: null, cloudTranscription: null, sourceAttribution: null, language: null, ...initial },
    migrated: initial.legacyMigrated === true,
    calls: [],
    failSave: false,
    failRead: false,
  };
  const view = () => ({
    parakeetDirectml: core.values.parakeetDirectml === true,
    parakeetWorker: core.values.parakeetWorker === true,
    cloudTranscription: core.values.cloudTranscription === true,
    sourceAttribution: core.values.sourceAttribution === true,
    language: core.values.language ?? "auto",
    legacyMigrated: core.migrated,
  });
  core.invoke = async (command, args) => {
    core.calls.push({ command, args });
    if (command === "get_behavior_settings") {
      if (core.failRead) throw new Error("unreadable");
      return view();
    }
    if (command === "migrate_legacy_behavior_settings") {
      if (!core.migrated) {
        for (const key of Object.keys(args.legacy)) {
          if (core.values[key] === null && args.legacy[key] !== null) core.values[key] = args.legacy[key];
        }
        core.migrated = true;
      }
      return view();
    }
    const setters = {
      set_cloud_transcription_enabled: "cloudTranscription",
      set_source_attribution_enabled: "sourceAttribution",
      set_parakeet_use_directml: "parakeetDirectml",
      set_parakeet_use_worker: "parakeetWorker",
    };
    if (setters[command]) {
      if (core.failSave) throw new Error("not saved");
      core.values[setters[command]] = args.enabled;
      return undefined;
    }
    if (command === "get_recording_preferences") return { auto_save: true };
    throw new Error(`unexpected ${command}`);
  };
  return core;
}

function load(core, storage = new Map()) {
  const window = {
    localStorage: {
      getItem: key => (storage.has(key) ? storage.get(key) : null),
      setItem: (key, value) => { storage.set(key, String(value)); },
    },
  };
  const previous = globalThis.window;
  globalThis.window = window;
  const mocks = { "@tauri-apps/api/core": { invoke: core.invoke } };
  const behavior = loadTsModule("src/lib/behaviorSettings.ts", mocks);
  const shared = { ...mocks, "@/lib/behaviorSettings": behavior };
  return {
    behavior,
    storage,
    cloud: loadTsModule("src/lib/cloudTranscription.ts", shared),
    attribution: loadTsModule("src/lib/sourceAttribution.ts", shared),
    parakeet: loadTsModule("src/lib/parakeetAccel.ts", shared),
    restore: () => { globalThis.window = previous; },
  };
}

test("every setting reads as off before the core has answered and cloud transcription stays off", async (t) => {
  const core = fakeCore({ cloudTranscription: true, sourceAttribution: true });
  const m = load(core);
  t.after(m.restore);
  assert.equal(m.cloud.getCloudTranscription(), false);
  assert.equal(m.attribution.getSourceAttribution(), false);
  assert.equal(m.parakeet.getParakeetDirectml(), false);
  assert.equal(m.behavior.behaviorSettingsLoaded(), false);
  await m.behavior.loadBehaviorSettings();
  assert.equal(m.cloud.getCloudTranscription(), true, "now it is the core's value");
  assert.equal(m.attribution.getSourceAttribution(), true);
});

test("a core that cannot be read leaves the defaults and the opt-in off, and a later read can succeed", async (t) => {
  const core = fakeCore({ cloudTranscription: true });
  core.failRead = true;
  const m = load(core);
  t.after(m.restore);
  await assert.rejects(m.behavior.loadBehaviorSettings());
  assert.equal(m.cloud.getCloudTranscription(), false);
  assert.equal(m.behavior.behaviorSettingsLoaded(), false);
  core.failRead = false;
  await m.behavior.loadBehaviorSettings();
  assert.equal(m.cloud.getCloudTranscription(), true);
});

test("an unexpected value from the core never switches a setting on", () => {
  const behavior = loadTsModule("src/lib/behaviorSettings.ts", { "@tauri-apps/api/core": { invoke: async () => null } });
  for (const odd of ["true", 1, "yes", null, undefined, {}, [true]]) {
    assert.equal(behavior.readBehaviorSettings({ cloudTranscription: odd, parakeetWorker: odd }).cloudTranscription, false, String(odd));
  }
  assert.equal(behavior.readBehaviorSettings(null).language, "auto");
  assert.equal(behavior.readBehaviorSettings({ language: "de" }).language, "de");
});

test("the first run hands the old webview values to the core, which keeps its own where it has one", async (t) => {
  const core = fakeCore({ parakeetWorker: false, language: "de" });
  const storage = new Map([
    ["clawscribe.parakeetDirectml", "true"],
    ["clawscribe.parakeetWorker", "true"],
    ["clawscribe.cloudTranscription", "true"],
    ["primaryLanguage", "fr"],
    ["clawscribe.sourceAttribution", "not a boolean"],
  ]);
  const m = load(core, storage);
  t.after(m.restore);
  const settings = await m.behavior.loadBehaviorSettings();
  const migrate = core.calls.find(call => call.command === "migrate_legacy_behavior_settings");
  assert.deepEqual(JSON.parse(JSON.stringify(migrate.args.legacy)), {
    parakeetDirectml: true, parakeetWorker: true, cloudTranscription: true, sourceAttribution: null, language: "fr",
  });
  assert.equal(settings.parakeetDirectml, true, "the core had none: the webview value is taken");
  assert.equal(settings.parakeetWorker, false, "the core had a value: it keeps its own");
  assert.equal(settings.cloudTranscription, true);
  assert.equal(settings.language, "de");
  assert.equal(settings.sourceAttribution, false, "an unusable old value is ignored");
  assert.equal(storage.get("clawscribe.parakeetDirectml"), "true", "the old keys are not deleted");
});

test("after the hand-over the webview values are not sent again and are no longer read", async (t) => {
  const core = fakeCore({ legacyMigrated: true, cloudTranscription: false });
  const m = load(core, new Map([["clawscribe.cloudTranscription", "true"]]));
  t.after(m.restore);
  const settings = await m.behavior.loadBehaviorSettings();
  assert.equal(core.calls.some(call => call.command === "migrate_legacy_behavior_settings"), false);
  assert.equal(settings.cloudTranscription, false, "a stale webview value cannot switch it on");
});

test("a change goes to the core and is read back from it; the old key mirrors it for a downgrade", async (t) => {
  const core = fakeCore({ legacyMigrated: true });
  const m = load(core);
  t.after(m.restore);
  await m.behavior.loadBehaviorSettings();
  const seen = [];
  m.cloud.subscribeCloudTranscription(enabled => seen.push(enabled));
  assert.equal(await m.cloud.setCloudTranscription(true), true);
  assert.equal(core.values.cloudTranscription, true);
  assert.equal(m.cloud.getCloudTranscription(), true);
  assert.deepEqual(seen, [true]);
  assert.equal(m.storage.get("clawscribe.cloudTranscription"), "true");
  await m.parakeet.setParakeetWorker(true);
  assert.equal(m.parakeet.getParakeetWorker(), true);
  assert.equal(await m.attribution.setSourceAttribution(true), true);
  assert.equal(m.attribution.getSourceAttribution(), true);
});

test("a failed save throws, leaves the stored value and does not touch the old key", async (t) => {
  const core = fakeCore({ legacyMigrated: true, cloudTranscription: false });
  const m = load(core);
  t.after(m.restore);
  await m.behavior.loadBehaviorSettings();
  core.failSave = true;
  await assert.rejects(m.cloud.setCloudTranscription(true));
  await assert.rejects(m.parakeet.setParakeetDirectml(true));
  await assert.rejects(m.attribution.setSourceAttribution(true));
  assert.equal(m.cloud.getCloudTranscription(), false);
  assert.equal(m.storage.has("clawscribe.cloudTranscription"), false);
  assert.equal(m.parakeet.getParakeetDirectml(), false);
});

test("the settings screen hook reads the core's value and fails closed", async (t) => {
  const core = fakeCore({ legacyMigrated: true, cloudTranscription: true });
  const m = load(core);
  t.after(m.restore);
  const hooks = createHookHarness();
  const { useCloudTranscription } = loadTsModule("src/hooks/useCloudTranscription.ts", {
    react: hooks.react,
    "@/lib/behaviorSettings": m.behavior,
    "@/lib/cloudTranscription": m.cloud,
  });
  const read = () => hooks.render(() => useCloudTranscription());
  assert.deepEqual(JSON.parse(JSON.stringify(read())), { enabled: false, loaded: false, failed: false });
  await flush();
  assert.deepEqual(JSON.parse(JSON.stringify(read())), { enabled: true, loaded: true, failed: false });
  hooks.unmount();

  const broken = fakeCore({ cloudTranscription: true });
  broken.failRead = true;
  const b = load(broken);
  const hooks2 = createHookHarness();
  const mod = loadTsModule("src/hooks/useCloudTranscription.ts", {
    react: hooks2.react,
    "@/lib/behaviorSettings": b.behavior,
    "@/lib/cloudTranscription": b.cloud,
  });
  hooks2.render(() => mod.useCloudTranscription());
  await flush();
  assert.deepEqual(JSON.parse(JSON.stringify(hooks2.render(() => mod.useCloudTranscription()))), { enabled: false, loaded: false, failed: true });
  hooks2.unmount();
});

test("the recording notice fails towards showing: a failed or slow read shows it, an explicit no does not", async (t) => {
  const calls = [];
  const behaviorWith = invoke => loadTsModule("src/lib/behaviorSettings.ts", { "@tauri-apps/api/core": { invoke } });
  const previous = globalThis.window;
  globalThis.window = { localStorage: { getItem: () => null, setItem() {} } };
  t.after(() => { globalThis.window = previous; });

  const failing = behaviorWith(async () => { throw new Error("core unavailable"); });
  assert.equal(await failing.readNoticePreference(), true, "a failed read shows the notice");

  const slow = behaviorWith(() => new Promise(() => {}));
  assert.equal(await slow.readNoticePreference(20), true, "a read that does not answer in time shows the notice");

  const off = behaviorWith(async command => { calls.push(command); return { showRecordingNotification: false }; });
  assert.equal(await off.readNoticePreference(), false, "the core's explicit no is respected");
  const on = behaviorWith(async () => ({ showRecordingNotification: true }));
  assert.equal(await on.readNoticePreference(), true);
  const odd = behaviorWith(async () => ({ showRecordingNotification: "no" }));
  assert.equal(await odd.readNoticePreference(), true, "an unreadable value shows it");
  assert.deepEqual(calls, ["get_behavior_settings"], "it reads the settings only; it never waits for the engine-settings hand-over");
});

test("the notice does not depend on the engine-settings hand-over, which can be held up by a busy job", async (t) => {
  const calls = [];
  const previous = globalThis.window;
  globalThis.window = { localStorage: { getItem: () => null, setItem() {} } };
  t.after(() => { globalThis.window = previous; });
  const behavior = loadTsModule("src/lib/behaviorSettings.ts", { "@tauri-apps/api/core": { invoke: async command => {
    calls.push(command);
    if (command === "get_behavior_settings") return { legacyMigrated: false, showRecordingNotification: true };
    throw "PARAKEET_WORKER_BUSY: a job is active";
  } } });
  assert.equal(await behavior.readNoticePreference(), true);
  assert.equal(calls.includes("migrate_legacy_behavior_settings"), false);
  const source = fs.readFileSync(new URL("../../src/lib/recordingNotification.tsx", import.meta.url), "utf8");
  assert.match(source, /readNoticePreference\(\)/);
  assert.doesNotMatch(source, /loadBehaviorSettings/);
});
