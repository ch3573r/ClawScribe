let _opener: (() => void) | null = null;

export function registerStorageDialogOpener(fn: () => void): () => void {
  _opener = fn;
  return () => { if (_opener === fn) _opener = null; };
}

export function openStorageDialog(): void {
  _opener?.();
}
