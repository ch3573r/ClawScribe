import assert from "node:assert/strict";
import path from "node:path";
import { beforeEach, describe, mock, test } from "node:test";
import { fileURLToPath } from "node:url";
import { loadTsModule } from "./load-ts-module.mjs";

const modulePath = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "src",
  "lib",
  "summary-language-preferences.ts",
);

let invokeMock;

function loadPreferencesModule() {
  return loadTsModule(modulePath, {
    "@tauri-apps/api/core": {
      invoke: invokeMock,
    },
  });
}

function installLocalStorage() {
  const values = new Map();

  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      localStorage: {
        getItem: (key) => values.get(key) ?? null,
        setItem: (key, value) => {
          values.set(key, value);
        },
        removeItem: (key) => {
          values.delete(key);
        },
        clear: () => {
          values.clear();
        },
      },
    },
  });

  return values;
}

function installFailingLocalStorage() {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      localStorage: {
        getItem: () => null,
        setItem: () => {
          throw new Error("quota exceeded");
        },
        removeItem: () => {},
        clear: () => {},
      },
    },
  });
}

describe("summary language local fallback", () => {
  let storageValues;

  beforeEach(() => {
    invokeMock = mock.fn(async () => null);
    storageValues = installLocalStorage();
  });

  test("reads summary language from local fallback when meeting has no folder", async () => {
    const prefs = loadPreferencesModule();
    storageValues.set("summaryLanguageFallback:meeting-1", "fr");
    invokeMock.mock.mockImplementationOnce(async () => ({
      language: null,
      storage: "local_fallback",
    }));

    const result = await prefs.readMeetingSummaryLanguage("meeting-1");
    assert.equal(result.language, "fr");
    assert.equal(result.storage, "local_fallback");
  });

  test("saves summary language locally when command reports no folder", async () => {
    const prefs = loadPreferencesModule();
    invokeMock.mock.mockImplementationOnce(async () => ({
      language: null,
      storage: "local_fallback",
    }));

    const result = await prefs.saveMeetingSummaryLanguage("meeting-1", "es");
    assert.equal(result.language, "es");
    assert.equal(result.storage, "local_fallback");
    assert.equal(storageValues.get("summaryLanguageFallback:meeting-1"), "es");
  });

  test("clears local fallback when Auto is saved for a folderless meeting", async () => {
    const prefs = loadPreferencesModule();
    storageValues.set("summaryLanguageFallback:meeting-1", "de");
    invokeMock.mock.mockImplementationOnce(async () => ({
      language: null,
      storage: "local_fallback",
    }));

    const result = await prefs.saveMeetingSummaryLanguage("meeting-1", null);
    assert.equal(result.language, null);
    assert.equal(result.storage, "local_fallback");
    assert.equal(storageValues.has("summaryLanguageFallback:meeting-1"), false);
  });

  test("caches detected language locally when meeting has no folder", async () => {
    const prefs = loadPreferencesModule();
    invokeMock.mock.mockImplementationOnce(async () => ({
      language: null,
      storage: "local_fallback",
    }));

    await prefs.saveCachedDetectedSummaryLanguage("meeting-1", "pt");

    assert.equal(
      storageValues.get("detectedSummaryLanguageFallback:meeting-1"),
      "pt",
    );
  });

  test("rejects when folderless summary language cannot be persisted locally", async () => {
    installFailingLocalStorage();
    const prefs = loadPreferencesModule();
    invokeMock.mock.mockImplementationOnce(async () => ({
      language: null,
      storage: "local_fallback",
    }));

    await assert.rejects(
      () => prefs.saveMeetingSummaryLanguage("meeting-1", "it"),
      /Failed to save summary language on this device/,
    );
  });
});
