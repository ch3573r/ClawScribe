import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { loadTsModule } from "./load-ts-module.mjs";

const modulePath = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "src",
  "lib",
  "cloudTranscription.ts",
);

function createWindow() {
  const listeners = new Map();
  const storage = new Map();

  return {
    localStorage: {
      getItem(key) {
        return storage.has(key) ? storage.get(key) : null;
      },
      setItem(key, value) {
        storage.set(key, String(value));
      },
    },
    addEventListener(type, listener) {
      const list = listeners.get(type) ?? [];
      list.push(listener);
      listeners.set(type, list);
    },
    removeEventListener(type, listener) {
      listeners.set(
        type,
        (listeners.get(type) ?? []).filter((item) => item !== listener),
      );
    },
    dispatchEvent(event) {
      for (const listener of listeners.get(event.type) ?? []) {
        listener(event);
      }
      return true;
    },
  };
}

test("cloud transcription preference is default-off and syncs to backend", async () => {
  const previousWindow = globalThis.window;
  const calls = [];
  globalThis.window = createWindow();
  globalThis.CustomEvent = class CustomEvent extends Event {
    constructor(type, options = {}) {
      super(type);
      this.detail = options.detail;
    }
  };

  try {
    const cloud = loadTsModule(modulePath, {
      "@tauri-apps/api/core": {
        invoke: async (command, args) => {
          calls.push({ command, args });
        },
      },
    });

    const seen = [];
    const unsubscribe = cloud.subscribeCloudTranscription((enabled) => {
      seen.push(enabled);
    });

    assert.equal(cloud.getCloudTranscription(), false);

    await cloud.setCloudTranscription(true);

    assert.equal(cloud.getCloudTranscription(), true);
    assert.deepEqual(seen, [true]);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].command, "set_cloud_transcription_enabled");
    assert.equal(calls[0].args.enabled, true);

    unsubscribe();
  } finally {
    globalThis.window = previousWindow;
  }
});
