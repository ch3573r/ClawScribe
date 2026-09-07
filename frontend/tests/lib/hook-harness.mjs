// Deterministic hooks for exercising async state transitions without a WebView.
export function createHookHarness() {
  const slots = [];
  let cursor = 0;
  let effects = [];
  const changed = (previous, next) => !previous || !next || previous.length !== next.length || next.some((value, index) => !Object.is(value, previous[index]));
  const react = {
    useState(initial) {
      const slot = slots[cursor++] ??= { value: typeof initial === 'function' ? initial() : initial };
      return [slot.value, next => { slot.value = typeof next === 'function' ? next(slot.value) : next; }];
    },
    useRef(initial) { return slots[cursor++] ??= { current: initial }; },
    useMemo(compute, dependencies) {
      const slot = slots[cursor++] ??= {};
      if (changed(slot.dependencies, dependencies)) {
        slot.value = compute();
        slot.dependencies = dependencies;
      }
      return slot.value;
    },
    useCallback(callback, dependencies) { return react.useMemo(() => callback, dependencies); },
    useEffect(callback, dependencies) {
      const slot = slots[cursor++] ??= {};
      if (changed(slot.dependencies, dependencies)) {
        effects.push(() => { slot.cleanup?.(); slot.cleanup = callback(); });
        slot.dependencies = dependencies;
      }
    },
    createContext: () => ({ Provider: 'Provider' }),
  };
  return {
    react,
    render(callback) {
      cursor = 0;
      const value = callback();
      const pending = effects;
      effects = [];
      pending.forEach(effect => effect());
      return value;
    },
    unmount() { slots.forEach(slot => slot.cleanup?.()); },
  };
}

export function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

export const flush = () => new Promise(setImmediate);
