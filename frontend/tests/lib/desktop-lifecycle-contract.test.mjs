import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

// Source-level checks for the tray, call monitor and power code. The Rust unit
// tests for these modules run on the designated build machine; these guard the
// contract between the window and the core wherever only Node is available.

const read = relative => fs.readFileSync(fileURLToPath(new URL('../../' + relative, import.meta.url)), 'utf8').replace(/\r\n/g, '\n');
const rust = name => read('src-tauri/src/' + name);
const service = read('src/services/callDetectionService.ts');
const lib = rust('lib.rs');
const snake = word => word.replace(/[A-Z]/g, letter => '_' + letter.toLowerCase());

function functionBody(source, signature) {
  const start = source.indexOf(signature);
  assert.notEqual(start, -1, `${signature} exists`);
  const open = source.indexOf('{', source.indexOf(')', start));
  let depth = 0;
  for (let index = open; index < source.length; index++) {
    if (source[index] === '{') depth++;
    if (source[index] === '}' && --depth === 0) return source.slice(open, index + 1);
  }
  throw new Error(`${signature} is not closed`);
}

test('every command the window calls for call detection and requests is registered in the core', () => {
  const commands = [...service.matchAll(/invoke(?:<[^>]*>)?\('([a-z_]+)'/g)].map(match => match[1]);
  assert.ok(commands.length >= 6);
  for (const command of commands) {
    assert.match(lib, new RegExp(`::${command},`), `${command} is registered in the invoke handler`);
  }
});

test('the arguments the window sends match the parameters of the core commands', () => {
  const signatures = {
    finish_recording_start: rust('start_request.rs'),
    set_call_detection_settings: rust('teams_detection/monitor.rs'),
    confirm_call_detection_prompt: rust('teams_detection/monitor.rs'),
  };
  for (const [command, source] of Object.entries(signatures)) {
    const call = service.match(new RegExp(`invoke\\('${command}', \\{ ([^}]*) \\}`));
    assert.ok(call, `${command} is called with arguments`);
    const start = source.indexOf(`fn ${command}`);
    const parameters = source.slice(start, source.indexOf('{', source.indexOf(')', start)));
    for (const argument of call[1].split(',').map(part => part.trim().split(':')[0])) {
      assert.match(parameters, new RegExp(`\\b${snake(argument)}\\b`), `${command} takes ${snake(argument)}`);
    }
  }
});

test('a start from the tray, shortcut or command line neither navigates nor takes the focus', () => {
  const tray = rust('tray.rs');
  assert.doesNotMatch(tray, /autoStartRecording/);
  assert.doesNotMatch(tray, /location\.assign\('\/'\)/);
  const intent = functionBody(tray, 'pub(crate) fn recording_intent_handler');
  assert.doesNotMatch(intent, /focus_main_window/);
  const request = functionBody(tray, 'async fn request_start');
  // Only a start that failed brings the window forward, so the reason is seen.
  const failure = request.slice(request.indexOf('Failed'));
  assert.match(failure, /focus_main_window/);
  assert.doesNotMatch(request.slice(0, request.indexOf('Failed')), /focus_main_window/);
  assert.doesNotMatch(functionBody(tray, 'fn stop_recording_handler'), /focus_main_window/);
  // The dead window-side handler that forwarded a toggle to the Home page is gone.
  assert.doesNotMatch(read('src/app/layout.tsx'), /request-recording-toggle/);
});

test('shortcuts and the command line go through the same gated start path as the tray', () => {
  assert.match(rust('shortcuts.rs'), /StartSource::Shortcut/);
  assert.match(rust('cli_control.rs'), /StartSource::Cli/);
  const gate = rust('start_request.rs');
  assert.match(gate, /capture_active\(\)/, 'no request while capture runs or stops');
  assert.match(gate, /if pending\.is_some\(\)/, 'one open request at a time');
  assert.match(gate, /START_TIMEOUT/, 'an unanswered request ends');
});

test('a registered hotkey checks the foreground HWND before deciding whether settings or recording receives it', () => {
  const shortcuts = rust('shortcuts.rs');
  const route = functionBody(shortcuts, 'pub fn on_shortcut');
  assert.ok(route.indexOf('capture_channel') >= 0 && route.indexOf('capture_channel') < route.indexOf('match action'));
  const windows = functionBody(shortcuts, 'fn main_is_foreground');
  assert.match(windows, /GetForegroundWindow\(\)/);
  assert.match(windows, /GetAncestor\(foreground, GA_ROOT\)/);
  assert.match(windows, /\.hwnd\(\)/);
  assert.doesNotMatch(windows, /set_focus|SetForegroundWindow/);
});

test('hidden capture alerts use generic desktop messages and the existing notification preferences', () => {
  assert.match(rust('tray.rs'), /listen_for_background_alerts\(app\)/);
  const alerts = functionBody(rust('notifications/commands.rs'), 'pub(crate) fn listen_for_background_alerts');
  for (const name of ['call-detection-prompt', 'recording-warning', 'recording-error', 'transcription-warning', 'recording-disk-space-low']) {
    assert.ok(alerts.includes('"' + name + '"'), name);
  }
  assert.match(alerts, /window\.is_visible\(\)[\s\S]*window\.is_minimized\(\)/);
  assert.match(alerts, /manager\s*\.show_notification/);
  assert.doesNotMatch(alerts, /\.payload\(|focus_main_window|request_permission/);
});

test('the power request is narrow, bounded and released on every way out', () => {
  const power = rust('power.rs');
  assert.match(power, /ES_SYSTEM_REQUIRED/);
  assert.doesNotMatch(power, /ES_DISPLAY_REQUIRED|ES_AWAYMODE_REQUIRED/, 'only the system, never the display');
  assert.match(power, /MAX_LEASE: Duration = Duration::from_secs\(12 \* 60 \* 60\)/);
  assert.match(power, /stopping \|\| \(recording && !paused\)/, 'held while capturing or finishing, not while paused');
  // The only place that talks to Windows power management.
  for (const file of ['tray.rs', 'lib.rs', 'exit_guard.rs', 'start_request.rs']) {
    assert.doesNotMatch(rust(file), /SetThreadExecutionState/, `${file} leaves power requests to power.rs`);
  }
  const exit = lib.split('tauri::RunEvent::Exit => {')[1];
  assert.match(exit, /if windows_lifecycle::session_ending\(\) \{[^}]*return;[^}]*\}/,
    'Windows session end can return immediately and release the request with the process');
  assert.match(exit, /log::info!\("Application exiting, cleaning up resources\.\.\."\);\s*power::release\(\);\s*tauri::async_runtime::block_on/,
    'normal exits explicitly release the request before database cleanup');
  assert.match(rust('tray.rs'), /crate::power::reconcile\(/);
  assert.match(read('src-tauri/Cargo.toml'), /"Win32_System_Power"/);
});

test('the monitor starts with the app, after the database, and the old window poller is gone', () => {
  assert.match(lib, /teams_detection::monitor::spawn\(_app\.handle\(\)\.clone\(\)\)/);
  assert.ok(lib.indexOf('report_startup_failure') < lib.indexOf('teams_detection::monitor::spawn'));
  assert.ok(!fs.existsSync(fileURLToPath(new URL('../../src/components/TeamsAutoRecord.tsx', import.meta.url))));
  assert.doesNotMatch(read('src/lib/autoRecord.ts'), /ObservedCalls|endedOnScan/);
});
