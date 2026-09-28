import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

function loadWithWindow(file, window, mocks = {}) {
  const previous = globalThis.window;
  try {
    globalThis.window = window;
    return loadTsModule(file, mocks);
  } finally { globalThis.window = previous; }
}

const widths = loadTsModule('src/lib/sidebarWidth.ts');

test('sidebar widths respect bounds and the viewport cap', () => {
  assert.equal(widths.DEFAULT_SIDEBAR_WIDTH, 280);
  assert.equal(widths.clampSidebarWidth(100, 1600), 224);
  assert.equal(widths.clampSidebarWidth(600, 1600), 448);
  assert.equal(widths.clampSidebarWidth(350, 1600), 350);
  assert.equal(widths.clampSidebarWidth(448, 1000), 400);
  assert.equal(widths.clampSidebarWidth(280, 500), 200);
  assert.equal(widths.clampSidebarWidth(NaN, 1600), 280);
});

test('sidebar widths round-trip through storage and reject missing or invalid values', () => {
  const values = new Map();
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
  const { readStoredSidebarWidth, storeSidebarWidth } = loadWithWindow('src/lib/sidebarWidth.ts', { localStorage: storage });
  assert.equal(readStoredSidebarWidth(), 280);
  assert.equal(storeSidebarWidth(336), 336);
  assert.equal(values.get('clawscribe.sidebarWidth'), '336');
  assert.equal(readStoredSidebarWidth(), 336);
  for (const value of ['', ' ', 'invalid', 'NaN', 'Infinity', '0', '-1', '999']) {
    values.set('clawscribe.sidebarWidth', value);
    assert.equal(readStoredSidebarWidth(), 280, value);
  }
});

test('unavailable sidebar storage falls back without crashing', () => {
  for (const window of [undefined, { get localStorage() { throw new Error('Blocked'); } }, {
    localStorage: { getItem() { throw new Error('Blocked read'); }, setItem() { throw new Error('Blocked write'); } },
  }]) {
    const { readStoredSidebarWidth, storeSidebarWidth } = loadWithWindow('src/lib/sidebarWidth.ts', window);
    assert.equal(readStoredSidebarWidth(), 280);
    assert.equal(storeSidebarWidth(336), 280);
  }
});

test('sidebar provider restores width, shares offsets, reclamps on resize and cleans up', async () => {
  const hooks = createHookHarness();
  const listeners = new Map();
  const media = { matches: false, addEventListener: (name, fn) => listeners.set(`media:${name}`, fn), removeEventListener: name => listeners.delete(`media:${name}`) };
  const window = {
    innerWidth: 1400,
    localStorage: { getItem: () => '350' },
    matchMedia: () => media,
    addEventListener: (name, fn) => listeners.set(name, fn),
    removeEventListener: name => listeners.delete(name),
  };
  let pathname = '/meetings';
  const jsx = (type, props) => ({ type, props });
  const { SidebarProvider } = loadWithWindow('src/components/Sidebar/SidebarProvider.tsx', window, {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'next/navigation': { usePathname: () => pathname, useRouter: () => ({}) },
    '@/lib/analytics': { trackBackendConnection() {} },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ isRecording: false }) },
    '@/hooks/useSummaryPolling': { useSummaryPolling: () => ({ activeSummaryPolls: new Map(), startSummaryPolling() {}, stopSummaryPolling() {} }) },
    '@tauri-apps/api/core': { invoke: async () => [] },
    '@tauri-apps/api/event': { listen: async () => () => {} },
  });
  const render = () => hooks.render(() => SidebarProvider({ children: null })).props.value;
  render(); await flush();
  assert.equal(render().sidebarWidth, 350);
  assert.equal(render().sidebarOffset, '350px');
  render().toggleCollapse();
  assert.equal(render().sidebarOffset, '4rem');
  render().toggleCollapse();
  pathname = '/settings';
  assert.equal(render().sidebarOffset, '4rem');
  pathname = '/meetings';
  render().setSidebarWidth(448);
  window.innerWidth = 1000;
  listeners.get('resize')();
  assert.equal(render().sidebarWidth, 400);
  assert.equal(render().sidebarOffset, '400px');
  assert.equal(render().sidebarMaxWidth, 400);
  render().setSidebarWidth(100);
  assert.equal(render().sidebarWidth, 224);
  render().setIsSidebarResizing(true);
  assert.equal(render().isSidebarResizing, true);
  media.matches = true;
  listeners.get('media:change')();
  assert.equal(render().sidebarOffset, '4rem');
  hooks.unmount();
  assert.equal(listeners.size, 0);
});

test('main content and status overlays use the shared offset and stop transitions during dragging', () => {
  const jsx = (type, props) => ({ type, props });
  let sidebar = { sidebarOffset: '352px', isSidebarResizing: false };
  const mocks = {
    'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => sidebar },
    '@/components/RecordingHealthBanner': { RecordingHealthBanner: 'RecordingHealthBanner' },
  };
  const MainContent = loadTsModule('src/components/MainContent/index.tsx', mocks).default;
  const { StatusOverlays } = loadTsModule('src/app/_components/StatusOverlays.tsx', mocks);
  const status = () => {
    const node = StatusOverlays({ isProcessing: true, isSaving: false }).props.children[0];
    return node.type(node.props).props.children;
  };
  assert.equal(MainContent({}).props.style.marginLeft, '352px');
  assert.equal(status().props.style.marginLeft, '352px');
  assert.match(MainContent({}).props.className, /transition-all/);
  assert.match(status().props.className, /transition-\[margin\]/);
  sidebar = { sidebarOffset: '400px', isSidebarResizing: true };
  assert.equal(MainContent({}).props.style.marginLeft, '400px');
  assert.equal(status().props.style.marginLeft, '400px');
  assert.doesNotMatch(MainContent({}).props.className, /transition/);
  assert.doesNotMatch(status().props.className, /transition/);
});
