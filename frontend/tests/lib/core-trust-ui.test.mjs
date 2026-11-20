import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { loadTsModule } from "./load-ts-module.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const read = relative => fs.readFileSync(path.join(root, relative), "utf8");

test("a core message that is a plain string is shown, not Unknown error", () => {
  const { errorText } = loadTsModule("src/lib/errorText.ts");
  assert.equal(errorText("This folder is not one of your recordings."), "This folder is not one of your recordings.");
  assert.equal(errorText(new Error("Disk full")), "Disk full");
  assert.equal(errorText({ message: "From another context" }), "From another context");
  assert.equal(errorText("   "), "Something went wrong. Try again.");
  assert.equal(errorText(undefined, "Custom fallback"), "Custom fallback");
  assert.equal(errorText(42), "Something went wrong. Try again.");
  assert.equal(errorText("x".repeat(500)), "Something went wrong. Try again.", "an essay is not a message");
});

test("the stop and save flow shows what the core said instead of Unknown error", () => {
  for (const file of ["src/hooks/useRecordingStop.ts", "src/hooks/useTranscriptRecovery.ts", "src/app/page.tsx"]) {
    const source = read(file);
    assert.doesNotMatch(source, /'Unknown error/, file);
    assert.match(source, /errorText\(/, file);
  }
});

test("the window may not use the store plugin or the open and save dialogs", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  const permissions = config.app.security.capabilities.flatMap(capability => capability.permissions);
  for (const forbidden of ["store:default", "dialog:allow-open", "dialog:allow-save", "dialog:default", "fs:default"]) {
    assert.equal(permissions.includes(forbidden), false, forbidden);
  }
  assert.equal(permissions.some(permission => permission.startsWith("store:")), false);
  // Boxes that only show a message stay.
  assert.ok(permissions.includes("dialog:allow-message"));
});

test("nothing in the interface imports the store plugin or the dialog plugin's open and save", () => {
  const walk = directory => fs.readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const full = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(full) : /\.(ts|tsx)$/.test(entry.name) ? [full] : [];
  });
  for (const file of walk(path.join(root, "src"))) {
    const source = fs.readFileSync(file, "utf8");
    assert.doesNotMatch(source, /@tauri-apps\/plugin-store/, path.relative(root, file));
    const dialog = source.match(/import\s*\{([^}]*)\}\s*from\s*['"]@tauri-apps\/plugin-dialog['"]/);
    if (dialog) {
      assert.doesNotMatch(dialog[1], /\b(open|save)\b/, `${path.relative(root, file)} imports ${dialog[1]}`);
    }
  }
});

test("the interface never passes a file path it made up to the commands that read files", () => {
  // Each of these accepts only a path the core's own dialog (or the window drop it saw) granted.
  const attach = read("src/components/MeetingDetails/AttachReferenceButton.tsx");
  assert.match(attach, /pick_reference_document/);
  const settings = read("src/components/TranscriptSettings.tsx");
  assert.match(settings, /pick_transcription_test_audio/);
  const backup = read("src/components/LibraryBackup.tsx");
  assert.match(backup, /pick_backup_save_path/);
  assert.match(backup, /pick_backup_restore_path/);
  const layout = read("src/app/layout.tsx");
  assert.match(layout, /files-dropped/);
  assert.doesNotMatch(layout, /tauri:\/\/drag-drop/, "the paths come from the core's event, not the webview's drop event");
  const onboarding = read("src/contexts/OnboardingContext.tsx");
  assert.match(onboarding, /import_legacy_database_if_present/);
  assert.doesNotMatch(onboarding, /legacyDbPath|check_homebrew_database/);
});

test("the recordings folder is chosen with the core's picker and never typed", () => {
  const settings = read("src/components/RecordingSettings.tsx");
  assert.match(settings, /select_recording_folder/);
  assert.doesNotMatch(settings, /<Input[^>]*save_folder/);
});
