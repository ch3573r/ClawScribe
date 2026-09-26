import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

function titlebar() {
  const calls = [];
  const jsx = (type, props) => ({ type, props });
  const appWindow = Object.fromEntries(
    ['startDragging', 'toggleMaximize', 'minimize', 'close'].map(name =>
      [name, async () => { calls.push(name); }]),
  );
  const previousWindow = globalThis.window;
  let root;
  try {
    globalThis.window = {};
    const { AppTitlebar } = loadTsModule('src/components/AppTitlebar.tsx', {
      'react/jsx-runtime': { jsx, jsxs: jsx },
      '@tauri-apps/api/window': { getCurrentWindow: () => appWindow },
    });
    root = AppTitlebar();
  } finally {
    globalThis.window = previousWindow;
  }

  // Model bubbling from React to Tauri's document listener. The latter handles
  // direct mousedown on a marked region (including double-click maximization).
  async function mouseDown(path, { button = 0, detail = 1 } = {}) {
    let stopped = false;
    const event = { button, detail, stopPropagation: () => { stopped = true; } };
    for (const node of path) {
      await node.props.onMouseDown?.(event);
      if (stopped) return;
    }
    if (button === 0 && (detail === 1 || detail === 2) &&
        Object.hasOwn(path[0].props, 'data-tauri-drag-region')) {
      await appWindow[detail === 2 ? 'toggleMaximize' : 'startDragging']();
    }
  }

  const [region, controls] = root.props.children;
  return { root, region, controls, calls, mouseDown };
}

test('title-bar gestures dispatch each native action once', async () => {
  for (const detail of [1, 2]) {
    for (const target of ['root', 'region']) {
      const view = titlebar();
      const path = target === 'root' ? [view.root] : [view.region, view.root];
      await view.mouseDown(path, { detail });
      assert.deepEqual(view.calls, [detail === 2 ? 'toggleMaximize' : 'startDragging']);
    }
  }
});

test('secondary mouse buttons do not start a window gesture', async () => {
  const view = titlebar();
  for (const button of [1, 2]) {
    await view.mouseDown([view.region, view.root], { button });
  }
  assert.deepEqual(view.calls, []);
});

test('window controls invoke their action without starting a drag', async () => {
  for (const [index, expected] of ['minimize', 'toggleMaximize', 'close'].entries()) {
    const view = titlebar();
    const button = view.controls.props.children[index];
    await view.mouseDown([button, view.controls, view.root]);
    await button.props.onClick();
    assert.deepEqual(view.calls, [expected]);
  }
});
