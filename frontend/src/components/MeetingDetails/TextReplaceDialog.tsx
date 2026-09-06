'use client';
import { useEffect, useRef, useState } from 'react';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

export interface ReplaceOptions { query: string; replacement: string; matchCase: boolean }
export interface ReplacePreview { token: string; matches: number; segments: number; examples: {before: string; after: string}[] }

export function TextReplaceDialog({ open, onOpenChange, title, preview, apply }: {
  open: boolean; onOpenChange: (open: boolean) => void; title: string;
  preview: (options: ReplaceOptions) => Promise<ReplacePreview>;
  apply: (options: ReplaceOptions, token: string) => Promise<void>;
}) {
  const [options, setOptions] = useState<ReplaceOptions>({ query: '', replacement: '', matchCase: false });
  const [result, setResult] = useState<ReplacePreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const version = useRef(0);
  const running = useRef(false);
  useEffect(() => { version.current++; setResult(null); setError(''); setBusy(false); }, [open]);
  const update = (value: Partial<ReplaceOptions>) => { version.current++; setOptions(old => ({ ...old, ...value })); setResult(null); setError(''); };
  const run = async (save: boolean) => {
    if (running.current) return;
    running.current = true;
    const request = ++version.current;
    setBusy(true); setError('');
    try {
      if (save && result) { await apply(options, result.token); if (request === version.current) onOpenChange(false); }
      else { const value = await preview(options); if (request === version.current) setResult(value); }
    } catch (e) { if (request === version.current) { setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Could not replace text. Please retry.'); setResult(null); } }
    finally { running.current = false; if (request === version.current) setBusy(false); }
  };
  return <Dialog open={open} onOpenChange={value => { if (!busy) onOpenChange(value); }}>
    <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
      <DialogHeader><DialogTitle>Find and replace in {title}</DialogTitle><DialogDescription>Preview literal text replacements before applying them. This checks the entire {title}.</DialogDescription></DialogHeader>
      <fieldset disabled={busy} className="space-y-3">
        <label className="block space-y-1 text-sm">Find<Input autoFocus maxLength={1000} value={options.query} onChange={e => update({ query: e.target.value })} /></label>
        <label className="block space-y-1 text-sm">Replace with<Input maxLength={4000} value={options.replacement} onChange={e => update({ replacement: e.target.value })} /></label>
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={options.matchCase} onChange={e => update({ matchCase: e.target.checked })} />Match case</label>
      </fieldset>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {result && <div className="space-y-3"><p role="status" className="text-sm font-medium">{result.matches} replacements in {result.segments} passages</p>
        {result.examples.map((example, i) => <div key={i} className="space-y-2 rounded border border-border p-3 text-sm">
          <p className="font-medium text-muted-foreground">Before</p><p className="whitespace-pre-wrap break-words">{example.before}</p>
          <p className="font-medium text-primary">After</p><p className="whitespace-pre-wrap break-words">{example.after}</p>
        </div>)}{result.segments > result.examples.length && <p className="text-xs text-muted-foreground">Showing the first {result.examples.length} affected passages.</p>}</div>}
      <DialogFooter><Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>Cancel</Button>
        <Button variant="outline" disabled={busy || !options.query} onClick={() => void run(false)}>Preview</Button>
        <Button disabled={busy || !result?.matches} onClick={() => void run(true)}>{busy ? 'Working…' : 'Replace all'}</Button></DialogFooter>
    </DialogContent>
  </Dialog>;
}
