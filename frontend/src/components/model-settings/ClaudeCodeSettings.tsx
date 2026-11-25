import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { platform } from '@tauri-apps/plugin-os';
import { Button } from '@/components/ui/button';
import { SettingsRow } from '@/components/ui/settings-layout';
import { CopyCommandRow, CommitInputRow, withError } from '@/components/settings/summary-provider-parts';
import { NoteRow } from '@/components/settings/settings-parts';
import { SaveError, type FeedbackState } from '@/components/settings/commit-field';
import { ClaudeCodeProviderCard, CLAUDE_CODE_MODELS, useClaudeCodeSettings } from '@/components/ClaudeCodeProviderCard';
import { openExternal } from '@/lib/openExternal';
import { SimpleModelSelect, type ProviderStatus } from './shared';

export { CLAUDE_CODE_MODELS };

export function useClaudeCodeProvider(active: boolean) {
  const claude = useClaudeCodeSettings(active);
  const [recheck, setRecheck] = useState(0);
  const [guideError, setGuideError] = useState('');
  const lastCheck = useRef(0);
  const { busy, status, check, setProgramPath } = claude;
  useEffect(() => {
    if (lastCheck.current === recheck) return;
    lastCheck.current = recheck;
    void check();
  }, [check, recheck]);
  useEffect(() => { if (!active) setGuideError(''); }, [active]);

  const savePath = async (value: string) => {
    try { await invoke('claude_code_save_config', { config: { programPath: value.trim() || null } }); }
    catch { throw new SaveError('Could not save the program path'); }
    setProgramPath(value);
    setRecheck(count => count + 1);
  };
  const openGuide = async () => {
    try { setGuideError(''); await openExternal('https://code.claude.com/docs/en/setup'); }
    catch { setGuideError('Could not open the install guide. Try again'); }
  };

  const version = status?.state === 'ready'
    ? (status.version.includes('Claude Code') ? status.version : `Claude Code ${status.version}`) : null;
  const health: ProviderStatus = busy ? { tone: 'neutral', text: 'Checking Claude Code…' }
    : !status ? { tone: 'neutral', text: 'Not checked yet' }
    : status.state === 'ready' ? { tone: 'ok', text: 'Ready', meta: [version, status.plan ? `${status.plan} plan` : null].filter(Boolean).join(' · ') }
    : status.state === 'not_installed' ? { tone: 'warning', text: 'Claude Code is not installed' }
    : status.state === 'not_signed_in' ? { tone: 'warning', text: 'Not signed in to Claude Code' }
    : { tone: 'error', text: 'Could not check Claude Code' };

  return { ...claude, health, guideError, openGuide, savePath };
}

type ClaudeState = ReturnType<typeof useClaudeCodeProvider>;

export function ClaudeCodeSettings({ state, autosave, model, chooseModel, disabled, feedback }: {
  state: ClaudeState; autosave: boolean; model: string; chooseModel: (model: string) => void; disabled: boolean; feedback?: FeedbackState;
}) {
  const { status, busy, programPath, setProgramPath, check, guideError, openGuide } = state;
  if (!autosave) return <ClaudeCodeProviderCard status={status} busy={busy} programPath={programPath} model={model}
    onProgramPathChange={setProgramPath} onModelChange={chooseModel} onCheck={check} />;

  let windows = false;
  try { windows = platform() === 'windows'; } catch { /* The guide also works when the platform is unknown. */ }
  if (busy) return <NoteRow role="status">Checking Claude Code…</NoteRow>;
  switch (status?.state) {
    case 'not_installed':
      return <>
        <SettingsRow title="Install Claude Code" description={withError('Install Claude Code, then choose Check again', guideError)}>
          <Button type="button" variant="outline" onClick={() => void openGuide()}>Open install guide</Button>
        </SettingsRow>
        {windows && <CopyCommandRow id="claude-code-command" title="Install command" command="winget install Anthropic.ClaudeCode" />}
      </>;
    case 'not_signed_in':
      return <CopyCommandRow id="claude-code-command" title="Sign-in command" description="Run this in a terminal to sign in" command="claude auth login" />;
    case 'ready':
      return <SimpleModelSelect value={model} disabled={disabled} feedback={feedback} onChoose={chooseModel}
        options={CLAUDE_CODE_MODELS.map(alias => ({ value: alias, label: alias.charAt(0).toUpperCase() + alias.slice(1) }))} />;
    case 'error': return <NoteRow tone="error">{status.message}</NoteRow>;
    default: return null;
  }
}

export function ClaudeCodeAdvanced({ state }: { state: ClaudeState }) {
  return <CommitInputRow id="claude-code-program" title="Program path" placeholder="Full path (optional)"
    description="Leave empty to find Claude Code automatically, or use the full path to the program or Windows command shim"
    value={state.programPath} disabled={state.busy} onCommit={state.savePath} />;
}
