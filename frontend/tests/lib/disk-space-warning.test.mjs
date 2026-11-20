import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { formatFreeSpace, diskSpaceLowText } = loadTsModule('src/lib/disk-space.ts');
const { describeStartError, isDiskSpaceRefusal, startErrorText } = loadTsModule('src/lib/start-error.ts');
const plain = value => JSON.parse(JSON.stringify(value));

// The refusal the core builds (disk_space.rs start_refusal), with a folder whose
// name contains words the old guesses looked for.
const REFUSAL = 'Not enough free disk space to record. 0.5 GB is free where recordings are saved (D:\\Microphone Models\\Output); at least 1.6 GB is needed. Free up space or choose another recordings folder in Settings.';

const MICROPHONE = {
  title: 'Microphone not available',
  message: 'Unable to access your microphone. Please check that:\n• Your microphone is connected\n• The app has microphone permissions\n• No other app is using the microphone',
};
const SYSTEM_AUDIO = {
  title: 'System audio not available',
  message: 'Unable to capture system audio. Please check that:\n• An audio output device is connected and enabled\n• Another app is not using it exclusively\n• Your sound settings are correct',
};
const PERMISSION = {
  title: 'Permission required',
  message: 'Recording permissions are required. Please:\n• Allow microphone access for ClawScribe in your system privacy settings\n• Restart the app after changing permissions',
};
const GENERIC = {
  title: 'Recording failed',
  message: 'Unable to start recording. Please check your audio device settings and try again.',
};

test('free space is shown in a readable unit', () => {
  assert.equal(formatFreeSpace(777_599_999), '778 MB');
  assert.equal(formatFreeSpace(612_345_678), '612 MB');
  assert.equal(formatFreeSpace(999_999_999), '1000 MB');
  assert.equal(formatFreeSpace(1_000_000_000), '1.0 GB');
  assert.equal(formatFreeSpace(1_234_567_890), '1.2 GB');
  assert.equal(formatFreeSpace(42_000_000_000), '42.0 GB');
  assert.equal(formatFreeSpace(0), '0 MB');
  assert.equal(formatFreeSpace(400_000), '0 MB');
});

test('an unusable number is not formatted', () => {
  for (const bad of [-1, NaN, Infinity, '12', null, undefined]) assert.equal(formatFreeSpace(bad), null, String(bad));
});

test('the warning says how much is free, that recording continues and that it may be cut short', () => {
  const text = diskSpaceLowText({ freeBytes: 612_345_678, folder: 'D:\\Secret\\Recordings' });
  assert.equal(text, '612 MB is free where recordings are saved. The recording continues, but it may be cut short if the disk fills up.');
  assert.doesNotMatch(text, /Secret|Recordings folder|D:/);
});

test('the warning still makes sense without a usable number', () => {
  assert.match(diskSpaceLowText({}), /^Free space is low where recordings are saved\. The recording continues/);
  assert.match(diskSpaceLowText(null), /cut short/);
});

test('the refused start is recognised and its whole message is kept', () => {
  assert.equal(isDiskSpaceRefusal(REFUSAL), true);
  assert.equal(isDiskSpaceRefusal(new Error(REFUSAL)), true);
  assert.equal(isDiskSpaceRefusal('The microphone is busy'), false);
  assert.equal(startErrorText(REFUSAL), REFUSAL);
  const shown = describeStartError(REFUSAL);
  assert.equal(shown.title, 'Not enough disk space');
  assert.equal(shown.message, REFUSAL, 'both sentences and the folder, unchanged');
});

test('words in the folder path do not turn the refusal into a device message', () => {
  assert.notEqual(describeStartError(REFUSAL).title, 'Microphone not available');
  assert.notEqual(describeStartError(REFUSAL).title, 'System audio not available');
});

test('other start errors map to their guidance, in the Windows wording', () => {
  assert.deepEqual(plain(describeStartError('Failed to open the microphone')), MICROPHONE);
  assert.deepEqual(plain(describeStartError('no output device')), SYSTEM_AUDIO);
  assert.deepEqual(plain(describeStartError('Unable to open the speaker')), SYSTEM_AUDIO);
  assert.deepEqual(plain(describeStartError('system audio capture failed')), SYSTEM_AUDIO);
  assert.deepEqual(plain(describeStartError('permission denied')), PERMISSION);
  assert.deepEqual(plain(describeStartError('boom')), GENERIC);
});

test('no start message carries macOS-only wording', () => {
  for (const error of ['microphone', 'speaker', 'permission', 'boom']) {
    const { message } = describeStartError(error);
    assert.doesNotMatch(message, /BlackHole|macOS|screen recording|System Settings/i, error);
  }
});
