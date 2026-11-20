import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

// Tauri 2 reads command arguments by camelCase name. A snake_case key at the
// top level of an invoke() call is ignored: an Option argument silently
// becomes None and a required one fails. Fields nested inside a struct
// argument follow that struct's serde names and are not checked here.
const source = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', 'src');
const BACKSLASH = String.fromCharCode(92);

function sourceFiles(directory, found = []) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) sourceFiles(full, found);
    else if (/[.]tsx?$/.test(entry.name)) found.push(full);
  }
  return found;
}

/** Splits at commas that are not inside brackets or strings. */
function topLevel(text) {
  const parts = [];
  let depth = 0, current = '', quote = null;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (quote) {
      current += char;
      if (char === quote && text[i - 1] !== BACKSLASH) quote = null;
      continue;
    }
    if (char === "'" || char === '"' || char === '`') { quote = char; current += char; continue; }
    if ('{[('.includes(char)) depth++;
    else if ('}])'.includes(char)) depth--;
    if (depth === 0 && char === ',') { parts.push(current); current = ''; } else current += char;
  }
  if (current.trim()) parts.push(current);
  return parts;
}

function invokeCalls(text) {
  const calls = [];
  const pattern = /[\s.(]invoke\w*\s*(<)?/g;
  let match;
  while ((match = pattern.exec(text))) {
    let i = match.index + match[0].length;
    if (match[1]) {
      let depth = 1;
      while (i < text.length && depth > 0) {
        if (text[i] === '<') depth++;
        else if (text[i] === '>' && text[i - 1] !== '=') depth--;
        i++;
      }
      while (/\s/.test(text[i])) i++;
    }
    if (text[i] !== '(') continue;
    let depth = 1, end = i + 1, quote = null;
    while (end < text.length && depth > 0) {
      const char = text[end];
      if (quote) { if (char === quote && text[end - 1] !== BACKSLASH) quote = null; }
      else if (char === "'" || char === '"' || char === '`') quote = char;
      else if ('([{'.includes(char)) depth++;
      else if (')]}'.includes(char)) depth--;
      end++;
    }
    const [command, args] = topLevel(text.slice(i + 1, end - 1)).map(part => part.trim());
    if (!command || !/^['"`]\w+['"`]$/.test(command)) continue;
    const keys = args && args.startsWith('{')
      ? topLevel(args.slice(1, -1)).map(part => part.trim().match(/^['"]?(\w+)['"]?\s*[:,}]?/)?.[1]).filter(Boolean)
      : [];
    calls.push({ command: command.slice(1, -1), keys, line: text.slice(0, match.index).split('\n').length });
  }
  return calls;
}

test('invoke calls send camelCase argument names', () => {
  let checked = 0;
  const problems = [];
  for (const file of sourceFiles(source)) {
    for (const call of invokeCalls(fs.readFileSync(file, 'utf8'))) {
      checked++;
      for (const key of call.keys) {
        if (key.includes('_')) problems.push(`${path.relative(source, file)}:${call.line} ${call.command} sends "${key}"`);
      }
    }
  }
  assert.ok(checked > 100, `expected to find the app's invoke calls, found ${checked}`);
  assert.deepEqual(problems, []);
});

test('the manual start sends the title and both device choices by camelCase name', async () => {
  const sent = [];
  const { loadTsModule } = await import('./load-ts-module.mjs');
  const { recordingService } = loadTsModule('src/services/recordingService.ts', {
    '@tauri-apps/api/core': { invoke: async (command, args) => { sent.push([command, args]); } },
    '@tauri-apps/api/event': { listen: async () => () => {} },
  });
  await recordingService.startRecordingWithDevices('Synthetic mic', 'Synthetic speakers', 'Synthetic meeting', null, undefined, 'de');
  await recordingService.attemptDeviceReconnect('Synthetic mic', 'Microphone');
  assert.deepEqual(JSON.parse(JSON.stringify(sent)), [
    ['start_recording_with_devices_and_meeting', { micDeviceName: 'Synthetic mic', systemDeviceName: 'Synthetic speakers', meetingName: 'Synthetic meeting', useDefaultMicrophone: true, calendarLink: null, summaryLanguage: 'de' }],
    ['attempt_device_reconnect', { deviceName: 'Synthetic mic', deviceType: 'Microphone' }],
  ]);
});
