import assert from "node:assert/strict";
import path from "node:path";
import { describe, test } from "node:test";
import { fileURLToPath } from "node:url";
import { loadTsModule } from "./load-ts-module.mjs";

const modulePath = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "src",
  "lib",
  "audioDevicePreferences.ts",
);

const {
  getAudioDeviceDisplayName,
  isExplicitAudioDevice,
  selectedDevicesFromPreferences,
} = loadTsModule(modulePath);

describe("audio device preferences", () => {
  test("maps persisted preferences to the shared Home device shape", () => {
    assert.equal(
      JSON.stringify(
        selectedDevicesFromPreferences({
          preferred_mic_device: "USB Microphone (input)",
          preferred_system_device: "Laptop Speakers (output)",
        }),
      ),
      JSON.stringify({
        micDevice: "USB Microphone (input)",
        systemDevice: "Laptop Speakers (output)",
      }),
    );
  });

  test("uses readable labels without changing the persisted device value", () => {
    assert.equal(
      getAudioDeviceDisplayName("USB Microphone (input)", "Default microphone"),
      "USB Microphone",
    );
    assert.equal(
      getAudioDeviceDisplayName("Desk Speakers (OUTPUT)", "Default output"),
      "Desk Speakers",
    );
  });

  test("identifies defaults and explicit device choices", () => {
    assert.equal(isExplicitAudioDevice(null), false);
    assert.equal(isExplicitAudioDevice("  "), false);
    assert.equal(isExplicitAudioDevice("USB Microphone (input)"), true);
    assert.equal(getAudioDeviceDisplayName(null, "System default"), "System default");
  });
});
