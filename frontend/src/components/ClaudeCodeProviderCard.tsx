import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { platform } from '@tauri-apps/plugin-os';
import { openExternal } from '@/lib/openExternal';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion';

export const CLAUDE_CODE_MODELS = ['sonnet', 'opus', 'haiku'];
export type ClaudeCodeStatus =
  | { state: 'not_installed' }
  | { state: 'not_signed_in' }
  | { state: 'ready'; version: string; plan: string | null }
  | { state: 'error'; message: string };
type ClaudeCodeConfig = { programPath: string | null };

export function useClaudeCodeSettings(active: boolean) {
  const [status, setStatus] = useState<ClaudeCodeStatus | null>(null);
  const [programPath, setProgramPath] = useState('');
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  const inFlight = useRef(false);

  useEffect(() => {
    const current = ++epoch.current;
    if (!active) { setBusy(false); inFlight.current = false; return; }
    inFlight.current = true;
    setBusy(true);
    setStatus(null);
    void (async () => {
      try {
        const config = await invoke<ClaudeCodeConfig>('claude_code_get_config');
        if (current !== epoch.current) return;
        setProgramPath(config.programPath || '');
        const next = await invoke<ClaudeCodeStatus>('claude_code_check_status');
        if (current === epoch.current) setStatus(next);
      } catch {
        if (current === epoch.current) setStatus({ state: 'error', message: 'Could not check Claude Code. Check Program path and try again' });
      } finally {
        if (current === epoch.current) { setBusy(false); inFlight.current = false; }
      }
    })();
    return () => { epoch.current += 1; };
  }, [active]);

  const check = async () => {
    if (!active || inFlight.current) return;
    const current = epoch.current;
    inFlight.current = true;
    setBusy(true);
    try {
      // The core runs only a saved path that passed its checks, so the path is
      // saved first (and refused with a message if it is not acceptable).
      try {
        await invoke('claude_code_save_config', { config: { programPath: programPath.trim() || null } });
      } catch (failure) {
        if (current === epoch.current) setStatus({ state: 'error', message: typeof failure === 'string' && failure.length < 200 ? failure : 'Choose the Claude Code program: a local file named claude, claude.exe, claude.cmd or claude.bat' });
        return;
      }
      const next = await invoke<ClaudeCodeStatus>('claude_code_check_status');
      if (current === epoch.current) setStatus(next);
    } catch {
      if (current === epoch.current) setStatus({ state: 'error', message: 'Could not check Claude Code. Check Program path and try again' });
    } finally {
      if (current === epoch.current) { setBusy(false); inFlight.current = false; }
    }
  };
  const save = async () => {
    await invoke('claude_code_save_config', { config: { programPath: programPath.trim() || null } });
  };
  return { status, programPath, setProgramPath, busy, check, save };
}

interface Props {
  status: ClaudeCodeStatus | null;
  busy: boolean;
  programPath: string;
  model: string;
  onProgramPathChange: (path: string) => void;
  onModelChange: (model: string) => void;
  onCheck: () => void;
}

function CopyCommand({ command, label }: { command: string; label: string }) {
  const [feedback, setFeedback] = useState('');
  const copy = async () => {
    try { await globalThis.navigator.clipboard.writeText(command); setFeedback('Copied'); }
    catch { setFeedback('Select the command and copy it manually'); }
  };
  return (
    <div className="space-y-2">
      <Label htmlFor="claude-code-command">{label}</Label>
      <div className="flex gap-2">
        <Input id="claude-code-command" value={command} readOnly className="font-mono text-sm" />
        <Button type="button" variant="outline" onClick={copy} aria-label={`Copy ${label.toLowerCase()}`}>Copy</Button>
      </div>
      {feedback && <p role="status" className="text-xs text-muted-foreground">{feedback}</p>}
    </div>
  );
}

export function ClaudeCodeProviderCard({ status, busy, programPath, model, onProgramPathChange, onModelChange, onCheck }: Props) {
  const [guideError, setGuideError] = useState('');
  let isWindows = false;
  try { isWindows = platform() === 'windows'; } catch { /* The guide also works when the platform is unknown. */ }
  const installGuide = async () => {
    try { await openExternal('https://code.claude.com/docs/en/setup'); }
    catch { setGuideError('Could not open the install guide. Try again'); }
  };
  return (
    <section aria-label="Claude Code" aria-busy={busy} className="rounded-lg border border-border bg-card p-4 space-y-4">
      <div className="space-y-1">
        <h3 className="text-sm font-medium">Claude Code</h3>
        <p className="text-sm text-muted-foreground">Uses the Claude Code you installed and signed in to. Your Claude plan&apos;s limits apply.</p>
      </div>
      {busy && <p role="status" className="text-sm text-muted-foreground">Checking Claude Code…</p>}
      {!busy && status?.state === 'not_installed' && <>
        <p className="text-sm text-muted-foreground">Install Claude Code yourself, then check again.</p>
        {isWindows && <CopyCommand command="winget install Anthropic.ClaudeCode" label="Install command" />}
        <Button type="button" variant="outline" onClick={installGuide}>Open install guide</Button>
      </>}
      {!busy && status?.state === 'not_signed_in' && <>
        <p className="text-sm text-muted-foreground">Open a terminal and run this command to sign in.</p>
        <CopyCommand command="claude auth login" label="Sign-in command" />
      </>}
      {!busy && status?.state === 'ready' && <>
        <p role="status" className="text-sm text-success">Ready · {status.version}{status.plan ? ` · ${status.plan}` : ''}</p>
        <div className="space-y-2">
          <Label htmlFor="claude-code-model">Model</Label>
          <Select value={model} onValueChange={onModelChange} disabled={busy}>
            <SelectTrigger id="claude-code-model"><SelectValue placeholder="Select model" /></SelectTrigger>
            <SelectContent>{CLAUDE_CODE_MODELS.map(alias => <SelectItem key={alias} value={alias}>{alias.charAt(0).toUpperCase() + alias.slice(1)}</SelectItem>)}</SelectContent>
          </Select>
        </div>
      </>}
      {!busy && status?.state === 'error' && <p role="alert" className="text-sm text-destructive">{status.message}</p>}
      {guideError && <p role="alert" className="text-sm text-destructive">{guideError}</p>}
      <Accordion type="single" collapsible>
        <AccordionItem value="advanced">
          <AccordionTrigger>Advanced</AccordionTrigger>
          <AccordionContent>
            <Label htmlFor="claude-code-program">Program path</Label>
            <Input id="claude-code-program" className="mt-2" value={programPath} placeholder="Path to claude.exe (optional)" disabled={busy} onChange={event => onProgramPathChange(event.target.value)} />
            <p className="mt-2 text-xs text-muted-foreground">Leave blank to find Claude Code automatically. Otherwise, use the full path to claude.exe (or the claude.cmd command shim) on a local drive. Checking saves the path.</p>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
      <Button type="button" variant="outline" disabled={busy} onClick={onCheck}>Check again</Button>
    </section>
  );
}
