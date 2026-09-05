import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { loadTsModule } from "./load-ts-module.mjs";

const modulePath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../src/hooks/useAutoScroll.ts");

// Deterministic hook/timer harness, not a substitute for browser integration tests.
function createHarness(options = {}) {
  let cursor = 0;
  let effects = [];
  const slots = [];
  const timers = new Map();
  const listeners = new Map();
  const navigation = [];
  let timerId = 0;
  const windowMock = {
    setTimeout(callback) { timers.set(++timerId, callback); return timerId; },
    clearTimeout(id) { timers.delete(id); },
    matchMedia() { return { matches: Boolean(options.reducedMotion) }; },
  };
  const sameDeps = (a, b) => a && b && a.length === b.length && a.every((value, index) => Object.is(value, b[index]));
  const react = {
    useRef(value) {
      const index = cursor++;
      return slots[index] ??= { current: value };
    },
    useState(value) {
      const index = cursor++;
      const slot = slots[index] ??= { value, set: next => { slot.value = next; } };
      return [slot.value, slot.set];
    },
    useCallback(callback, deps) {
      const index = cursor++;
      if (!sameDeps(slots[index]?.deps, deps)) slots[index] = { callback, deps };
      return slots[index].callback;
    },
    useEffect(callback, deps) {
      const index = cursor++;
      if (!sameDeps(slots[index]?.deps, deps)) {
        const previous = slots[index];
        slots[index] = { deps, cleanup: undefined };
        effects.push(() => {
          previous?.cleanup?.();
          slots[index].cleanup = callback();
        });
      }
    },
  };
  const previousWindow = globalThis.window;
  globalThis.window = windowMock;
  let useAutoScroll;
  try {
    ({ useAutoScroll } = loadTsModule(modulePath, { react }));
  } finally {
    globalThis.window = previousWindow;
  }
  const container = {
    scrollHeight: 1000, clientHeight: 400, scrollTop: 600,
    ownerDocument: {
      getElementById(id) { return { scrollIntoView(config) { navigation.push({ id, ...config }); } }; },
    },
    addEventListener(name, callback) {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(callback);
    },
    removeEventListener(name, callback) { listeners.get(name)?.delete(callback); },
  };
  let props = {
    scrollRef: { current: container }, segments: [{ id: "one" }],
    isRecording: true, isPaused: false, ...options.props,
  };
  const harness = {
    container, timers, navigation,
    render(next = {}) {
      props = { ...props, ...next };
      cursor = 0;
      effects = [];
      const result = useAutoScroll(props);
      for (const run of effects) run();
      return result;
    },
    emit(name, event = {}) { for (const callback of listeners.get(name) ?? []) callback(event); },
    flushTimers() {
      for (const [id, callback] of [...timers]) {
        if (timers.delete(id)) callback();
      }
    },
    unmount() { for (const slot of slots) slot?.cleanup?.(); },
    listenerCount() { return [...listeners.values()].reduce((count, set) => count + set.size, 0); },
  };
  return harness;
}

function mounted(t, options) {
  const harness = createHarness(options);
  t.after(() => harness.unmount());
  harness.render();
  return harness;
}

test("follows a tall appended segment using the previous bottom position", t => {
  const h = mounted(t);
  h.container.scrollHeight = 1600;
  h.render({ segments: [{ id: "one" }, { id: "two" }] });
  assert.equal(h.container.scrollTop, 1600);
});

test("manual scrolling stops following; returning to the bottom resumes it", t => {
  const h = mounted(t);
  h.container.scrollTop = 100;
  h.emit("scroll");
  assert.equal(h.render().autoScroll, false);
  h.container.scrollHeight = 1600;
  h.render({ segments: [{ id: "one" }, { id: "two" }] });
  assert.equal(h.container.scrollTop, 100);
  h.container.scrollTop = 1200;
  h.emit("scroll");
  assert.equal(h.render().autoScroll, true);
  h.container.scrollHeight = 2000;
  h.render({ segments: [{ id: "one" }, { id: "two" }, { id: "three" }] });
  assert.equal(h.container.scrollTop, 2000);
});

for (const mode of [{ isPaused: true }, { disableAutoScroll: true }, { isRecording: false }]) {
  test(`does not follow when ${Object.keys(mode)[0]}=${Object.values(mode)[0]}`, t => {
    const h = mounted(t, { props: mode });
    h.container.scrollHeight = 1600;
    h.render({ segments: [{ id: "one" }, { id: "two" }] });
    assert.equal(h.container.scrollTop, 600);
    assert.equal(h.timers.size, 0);
  });
}

test("a user gesture cancels the delayed virtualized follow-up", t => {
  const virtualizer = { getTotalSize: () => 1600, scrollToOffset() {}, scrollToIndex() {} };
  const h = mounted(t, { props: { virtualizer, virtualizationThreshold: 1 } });
  h.container.scrollHeight = 1600;
  h.render({ segments: [{ id: "one" }, { id: "two" }] });
  assert.ok(h.timers.size > 0);
  h.emit("wheel", { deltaY: -200 });
  h.container.scrollTop = 100;
  h.emit("scroll");
  h.flushTimers();
  assert.equal(h.container.scrollTop, 100);
  assert.equal(h.render().autoScroll, false);
});

test("unmount removes event listeners and pending scroll timers", t => {
  const h = mounted(t);
  h.render({ segments: [{ id: "one" }, { id: "two" }] });
  assert.ok(h.listenerCount() > 0);
  assert.ok(h.timers.size > 0);
  h.unmount();
  assert.equal(h.listenerCount(), 0);
  assert.equal(h.timers.size, 0);
});

test("appending segments does not repeatedly navigate to the same search result", t => {
  const h = mounted(t, { props: { activeSegmentId: "one", disableAutoScroll: true } });
  assert.equal(h.navigation.length, 1);
  h.render({ segments: [{ id: "one" }, { id: "two" }] });
  assert.equal(h.navigation.length, 1);
});

test("explicit navigation respects reduced motion", t => {
  const h = mounted(t, { reducedMotion: true, props: { activeSegmentId: "one" } });
  assert.equal(h.navigation[0].behavior, "auto");
});

test("disabling follow updates the ref before the next render", t => {
  const h = mounted(t);
  h.render().setAutoScroll(false);
  h.container.scrollHeight = 1600;
  h.render({ segments: [{ id: "one" }, { id: "two" }] });
  assert.equal(h.container.scrollTop, 600);
});

test("jumping to the latest segment re-enables following", t => {
  const h = mounted(t);
  h.container.scrollTop = 100;
  h.emit("scroll");
  h.render().scrollToBottom();
  assert.equal(h.container.scrollTop, h.container.scrollHeight);
  assert.equal(h.render().autoScroll, true);
});
