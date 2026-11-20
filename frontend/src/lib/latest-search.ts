/**
 * A search box that waits for a pause in typing and only ever shows the answer
 * to the latest query. Older requests may still finish, in any order; their
 * answers, errors and "searching" state are ignored.
 */
export interface LatestSearchOptions<T> {
  delayMs?: number;
  run: (query: string) => Promise<T>;
  empty: T;
  onResults: (results: T) => void;
  onSearching: (searching: boolean) => void;
  onError?: (error: unknown) => void;
}

export interface LatestSearch {
  /** Resolves when this query has been answered, or replaced by a newer one. */
  search: (query: string) => Promise<void>;
  /** Drops any pending or running search without touching the results. */
  cancel: () => void;
}

export function createLatestSearch<T>({ delayMs = 200, run, empty, onResults, onSearching, onError }: LatestSearchOptions<T>): LatestSearch {
  let latest = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let settlePending: (() => void) | undefined;

  const supersede = () => {
    latest++;
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
    settlePending?.();
    settlePending = undefined;
  };

  return {
    cancel: supersede,
    search(query: string) {
      supersede();
      const mine = latest;
      if (!query.trim()) {
        onSearching(false);
        onResults(empty);
        return Promise.resolve();
      }
      onSearching(true);
      return new Promise<void>(resolve => {
        settlePending = resolve;
        timer = setTimeout(async () => {
          timer = undefined;
          try {
            const results = await run(query);
            if (mine === latest) onResults(results);
          } catch (error) {
            if (mine === latest) {
              onError?.(error);
              onResults(empty);
            }
          } finally {
            if (mine === latest) {
              onSearching(false);
              settlePending = undefined;
            }
            resolve();
          }
        }, delayMs);
      });
    },
  };
}
