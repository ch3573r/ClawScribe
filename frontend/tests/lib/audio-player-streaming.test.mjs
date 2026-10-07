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
    first.currentTime = 120; first.emit('timeupdate');
    assert.equal(render().currentTime, 120);
    render().pause(); assert.equal(render().isPlaying, false);
    await render().seek(9999); assert.equal(first.currentTime, 3600);
    first.emit('ended'); assert.equal(render().currentTime, 0);
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
