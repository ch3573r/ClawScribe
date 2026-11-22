import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness } from "./hook-harness.mjs";
import { loadTsModule } from "./load-ts-module.mjs";

function mounted(t, initial = {}) {
  const hooks = createHookHarness();
  const content = {};
  const resizeListeners = new Set();
  const viewport = new EventTarget();
  let scrollTop = 0;
  Object.defineProperty(viewport, "scrollTop", {
    get: () => scrollTop,
    set: value => { scrollTop = Math.min(Math.max(0, value), viewport.scrollHeight - viewport.clientHeight); },
  });
  Object.assign(viewport, {
    clientHeight: 410, scrollHeight: 1200, focus() {},
  });

  const previousWindow = globalThis.window;
  globalThis.window = {
    ResizeObserver: class {
      constructor(callback) { this.callback = callback; }
      observe() { resizeListeners.add(this.callback); }
      disconnect() { resizeListeners.delete(this.callback); }
    },
  };
  let useAutoScroll;
  try {
    ({ useAutoScroll } = loadTsModule("src/hooks/useAutoScroll.ts", {
      react: { ...hooks.react, useLayoutEffect: hooks.react.useEffect },
    }));
  } finally {
    globalThis.window = previousWindow;
  }

  let props = {
    scrollRef: { current: viewport }, contentRef: { current: content },
    segmentCount: 10, enabled: true, isPaused: false, ...initial,
  };
  const harness = {
    viewport, resizeListeners,
    render(next) {
      props = { ...props, ...next };
      return hooks.render(() => useAutoScroll(props));
    },
    scroll(top) {
      viewport.scrollTop = top;
      viewport.dispatchEvent(new Event("scroll"));
    },
    wheel(deltaY) {
      viewport.dispatchEvent(Object.assign(new Event("wheel"), { deltaY }));
    },
    grow(height) {
      viewport.scrollHeight += height;
      for (const callback of resizeListeners) callback();
    },
    unmount: hooks.unmount,
  };
  harness.render();
  t.after(harness.unmount);
  return harness;
}

test("restored rows start at the tail and wrapped live rows keep it in view as they grow", t => {
  const h = mounted(t);
  assert.equal(h.viewport.scrollTop, 790);
  h.viewport.scrollHeight += 120;
  h.render({ segmentCount: 11 });
  assert.equal(h.viewport.scrollTop, 910);
  h.grow(75);
  assert.equal(h.viewport.scrollTop, 985);
});

test("a short upward scroll detaches even when it is within one wrapped row of the tail", t => {
  const h = mounted(t);
  h.scroll(742);
  assert.equal(h.render().autoScroll, false);
  h.viewport.scrollHeight += 125;
  h.render({ segmentCount: 11 });
  h.grow(65);
  assert.equal(h.viewport.scrollTop, 742);
});

test("an upward wheel gesture detaches before a concurrent transcript commit", t => {
  const h = mounted(t);
  h.wheel(-48);
  h.viewport.scrollHeight += 125;
  h.render({ segmentCount: 11 });
  assert.equal(h.viewport.scrollTop, 790);
  assert.equal(h.render().autoScroll, false);
});

test("jump restores the live tail after scroll-back, including a second scroll-back", t => {
  const h = mounted(t);
  for (const segmentCount of [11, 12]) {
    h.scroll(120);
    h.viewport.scrollHeight += 125;
    h.render({ segmentCount });
    assert.equal(h.viewport.scrollTop, 120);
    const control = h.render();
    assert.equal(control.autoScroll, false);

    // Arriving at the bottom through unrelated layout changes cannot resume following.
    h.scroll(h.viewport.scrollHeight);
    assert.equal(h.render().autoScroll, false);
    control.scrollToBottom();
    assert.equal(h.render().autoScroll, true);
    h.grow(75);
    assert.equal(h.viewport.scrollTop, h.viewport.scrollHeight - 410);
  }
});

test("pause preserves detachment and Stop leaves the position alone", t => {
  const h = mounted(t);
  h.scroll(120);
  h.render({ isPaused: true });
  h.grow(120);
  h.render({ isPaused: false, segmentCount: 11 });
  assert.equal(h.viewport.scrollTop, 120);
  assert.equal(h.render().autoScroll, false);

  h.render({ enabled: false, segmentCount: 12 });
  h.grow(125);
  assert.equal(h.viewport.scrollTop, 120);
  assert.equal(h.resizeListeners.size, 0);
  h.render({ enabled: true, segmentCount: 1 });
  assert.equal(h.viewport.scrollTop, 1035);
  assert.equal(h.render().autoScroll, true);
});

test("a paused or saved transcript does not jump as its content changes", t => {
  for (const props of [{ isPaused: true }, { enabled: false }]) {
    const h = mounted(t, props);
    assert.equal(h.viewport.scrollTop, 0);
    h.grow(120);
    h.render({ segmentCount: 11 });
    assert.equal(h.viewport.scrollTop, 0);
    assert.equal(h.resizeListeners.size, 0);
  }
});

test("unmount disconnects the resize observer and removes scroll handling", t => {
  const h = mounted(t);
  h.unmount();
  assert.equal(h.resizeListeners.size, 0);
  h.scroll(120);
  assert.equal(h.render().autoScroll, true);
});
