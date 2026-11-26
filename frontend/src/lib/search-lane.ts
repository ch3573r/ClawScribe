/** Hold one IPC call and at most one waiting query. Tauri invoke has no AbortSignal. */
export function createSearchLane<T>() {
  type Task = { run: () => Promise<T>; resolve: (value: T | null) => void; reject: (error: unknown) => void; signal: AbortSignal };
  let running = false;
  let waiting: Task | undefined;

  async function drain() {
    if (running || !waiting) return;
    const task = waiting;
    waiting = undefined;
    if (task.signal.aborted) { task.resolve(null); return; }
    running = true;
    try {
      const result = await task.run();
      task.resolve(task.signal.aborted ? null : result);
    } catch (error) {
      if (task.signal.aborted) task.resolve(null);
      else task.reject(error);
    } finally {
      running = false;
      void drain();
    }
  }

  return (signal: AbortSignal, run: () => Promise<T>) => new Promise<T | null>((resolve, reject) => {
    if (signal.aborted) { resolve(null); return; }
    waiting?.resolve(null);
    waiting = { signal, run, resolve, reject };
    void drain();
  });
}
