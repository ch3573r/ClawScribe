import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred } from './hook-harness.mjs';

test('playback streams the resolved file, seeks and releases old media on navigation', async () => {
  const previousWindow = globalThis.window;
  const elements = [];
  class Audio {
    constructor() { this.handlers = new Map(); this.duration = NaN; this.currentTime = 0; elements.push(this); }
    addEventListener(name, handler) { this.handlers.set(name, handler); }
    removeEventListener(name) { this.handlers.delete(name); }
    emit(name) { this.handlers.get(name)?.(); }
    load() {}
    play() { this.emit('playing'); return this.playResult ?? Promise.resolve(); }
    pause() { this.paused = true; this.emit('pause'); }
    removeAttribute(name) { delete this[name]; }
  }
  globalThis.window = { Audio };
  const hooks = createHookHarness();
  try {
    const { useAudioPlayer } = loadTsModule('src/hooks/useAudioPlayer.ts', {
      react: hooks.react,
      '@tauri-apps/api/core': { convertFileSrc: path => `asset:${path}` },
    });
    let path = 'meeting/audio.mp4';
    const render = () => hooks.render(() => useAudioPlayer(path));
    render();
    const first = elements[0];
    assert.equal(first.src, 'asset:meeting/audio.mp4');
    assert.equal(first.preload, 'metadata');
    await render().seek(12);
    first.duration = 3600; first.emit('loadedmetadata');
    assert.equal(first.currentTime, 12);
    assert.equal(render().duration, 3600);
    await render().play(); assert.equal(render().isPlaying, true);
    const player = render();
    const observed = [];
    const unsubscribe = player.clock.subscribe(() => observed.push(player.clock.getSnapshot()));
    first.currentTime = 120; first.emit('timeupdate');
    first.emit('timeupdate');
    assert.equal(render(), player, 'clock ticks leave the page-level player value unchanged');
    assert.deepEqual(observed, [120]);
    render().pause(); assert.equal(render().isPlaying, false);
    await render().seek(9999); assert.equal(first.currentTime, 3600);
    first.emit('ended'); assert.equal(player.clock.getSnapshot(), 0);
    assert.deepEqual(observed, [120, 3600, 0]);
    unsubscribe();
    const pending = deferred(); first.playResult = pending.promise;
    const oldPlay = render().play();
    path = 'another/audio.wav'; render();
    pending.reject(new Error('Old media unloaded')); await oldPlay;
    assert.equal(render().error, null);
    assert.equal(first.src, undefined);
    assert.equal(first.handlers.size, 0);
    hooks.unmount();
    assert.equal(elements[1].src, undefined);
  } finally { globalThis.window = previousWindow; }
});

test('citation playback can distinguish native media rejection from successful playback', async () => {
  const previousWindow = globalThis.window;
  class Audio {
    addEventListener() {} removeEventListener() {} load() {} pause() {} removeAttribute() {}
    play() { return Promise.reject(new Error('Synthetic playback failure')); }
  }
  globalThis.window = { Audio };
  const hooks = createHookHarness();
  try {
    const { useAudioPlayer } = loadTsModule('src/hooks/useAudioPlayer.ts', { react: hooks.react, '@tauri-apps/api/core': { convertFileSrc: path => `asset:${path}` } });
    const render = () => hooks.render(() => useAudioPlayer('public-fixture/audio.wav'));
    render();
    assert.equal(await render().play(), false, 'failed playback must not complete a citation navigation as successful');
    assert.equal(render().error, 'Failed to play audio');
    hooks.unmount();
  } finally { globalThis.window = previousWindow; }
});

test('player resumes each meeting once metadata is ready, remembers seeks, and keeps mute independent of volume', async () => {
  const previousWindow = globalThis.window;
  const elements = [];
  const positions = new Map([['clawscribe.playbackPosition.first', '40'], ['clawscribe.playbackPosition.second', '99']]);
  const mapKey = 'clawscribe.playbackPositions';
  positions.set(mapKey, JSON.stringify(Array.from({ length: 200 }, (_, i) => [`old-${i}`, 10])));
  const windowEvents = new Map();
  class Audio {
    duration = NaN;
    currentTime = 0;
    handlers = new Map();
    constructor() { elements.push(this); }
    load() {} pause() {} removeAttribute() {}
    addEventListener(name, handler) { this.handlers.set(name, handler); }
    removeEventListener(name) { this.handlers.delete(name); }
    emit(name) { this.handlers.get(name)?.(); }
  }
  globalThis.window = { Audio, localStorage: {
    getItem: key => positions.get(key) ?? null, setItem: (key, value) => positions.set(key, value),
    removeItem: key => positions.delete(key),
  }, addEventListener: (event, handler) => windowEvents.set(event, handler), removeEventListener: event => windowEvents.delete(event) };
  const hooks = createHookHarness();
  try {
    const { useAudioPlayer } = loadTsModule('src/hooks/useAudioPlayer.ts', { react: hooks.react, '@tauri-apps/api/core': { convertFileSrc: path => path } });
    let id = 'first';
    const render = () => hooks.render(() => useAudioPlayer(`${id}.wav`, id));
    render();
    elements[0].duration = 100;
    elements[0].emit('loadedmetadata');
    assert.equal(elements[0].currentTime, 40);
    await render().seekBy(10);
    elements[0].emit('durationchange');
    assert.equal(render().clock.getSnapshot(), 50, 'metadata refresh does not reset playback');
    render().setVolume(0.4);
    render().toggleMute();
    assert.equal(elements[0].volume, 0.4);
    assert.equal(elements[0].muted, true);
    assert.equal(render().muted, true);
    id = 'second'; render();
    const saved = JSON.parse(positions.get(mapKey));
    assert.equal(saved.length, 200);
    assert.deepEqual(saved[0], ['first', 50]);
    assert.equal(saved.some(([key]) => key === 'old-199'), false);
    assert.equal(positions.has('clawscribe.playbackPosition.first'), false);
    assert.equal(elements[1].volume, 0.4);
    assert.equal(elements[1].muted, true);
    render().toggleMute();
    elements[1].duration = 100;
    elements[1].emit('loadedmetadata');
    assert.equal(elements[1].currentTime, 0, 'starts at the beginning near the end');
    assert.equal(elements[1].muted, false);
    await render().seek(15);
    windowEvents.get('pagehide')?.();
    assert.deepEqual(JSON.parse(positions.get(mapKey))[0], ['second', 15]);
    elements[1].emit('ended');
    assert.deepEqual(JSON.parse(positions.get(mapKey))[0], ['second', 0]);
    id = 'first'; render();
    await render().seek(4);
    elements[2].duration = 100;
    elements[2].emit('loadedmetadata');
    assert.equal(elements[2].currentTime, 4, 'an explicit early seek wins over the remembered position');
  } finally { hooks.unmount(); globalThis.window = previousWindow; }
});
