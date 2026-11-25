import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ExternalLink, Wrench } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Meta, SettingsRow } from '@/components/ui/settings-layout';
import { SaveError, safeMessage, type FeedbackState } from '@/components/settings/commit-field';
import { openExternal } from '@/lib/openExternal';
import { numberProblem, ProviderTextField, SimpleModelSelect, type ProviderStatus, type UpdateConfig } from './shared';

interface CodexConfig {
  codexHomeMode: 'clawscribe-isolated' | 'existing-user-codex-session';
  codexHomePath?: string | null;
  useExistingUserCodexSession: boolean;
  codexBinaryPath?: string | null;
  model: string;
  timeoutSeconds: number;
}

interface RuntimeStatus {
  found: boolean;
  version?: string | null;
  path?: string | null;
  runtimeSha256?: string | null;
  runtimeKind: string;
  codexHome: string;
  authStatus?: string | null;
  accountEmail?: string | null;
  planType?: string | null;
  rateLimitState?: string | null;
  message: string;
}

interface CommandStatus { success: boolean; stdout: string; stderr: string; message: string }
interface CodexModel { id: string; displayName: string; isDefault: boolean }
interface RepairCommand { label: string; shell: string; command: string }
interface RepairPlan { docsUrl: string; message: string; recommended: RepairCommand; alternatives: RepairCommand[] }

const DEFAULT_MODEL = 'gpt-5.6-sol';
const DEFAULT_HOME = '%APPDATA%\\ClawScribe\\codex';
const DEFAULT_CONFIG: CodexConfig = {
  codexHomeMode: 'clawscribe-isolated', useExistingUserCodexSession: false, codexHomePath: null, codexBinaryPath: null,
  model: DEFAULT_MODEL, timeoutSeconds: 600,
};
const timeoutProblem = numberProblem(30, 86400, true, true);

type CodexAction = 'codex_login_browser' | 'codex_login_device' | 'codex_logout' | 'codex_test_app_server' | 'codex_test_processing';

export function useCodexSettings(active: boolean, preview: boolean, selected: string, updateConfig: UpdateConfig) {
  const [config, setConfig] = useState(DEFAULT_CONFIG);
  const [timeoutDraft, setTimeoutDraft] = useState<string | null>(null);
  const timeout = timeoutDraft ?? String(config.timeoutSeconds);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [models, setModels] = useState<CodexModel[]>([]);
  const [modelError, setModelError] = useState(false);
  const [result, setResult] = useState('');
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(false);
  const canStart = useRef(active && !preview);
  canStart.current = active && !preview;

  const loadModels = useCallback(async () => {
    setLoading(true);
    setModelError(false);
    try {
      const rows = await invoke<CodexModel[]>('codex_list_models');
      setModels(rows);
      setModelError(rows.length === 0);
    } catch {
      setModels([]);
      setModelError(true);
    } finally { setLoading(false); }
  }, []);

  const persistConfig = useCallback(async (next: CodexConfig) => {
    const problem = timeoutProblem(String(next.timeoutSeconds));
    if (problem) throw new SaveError(problem);
    const normalized: CodexConfig = {
      ...next, codexHomeMode: 'clawscribe-isolated', useExistingUserCodexSession: false, codexBinaryPath: null,
      model: next.model.trim() || DEFAULT_MODEL,
    };
    try {
      const saved = await invoke<{ processing?: { codex?: CodexConfig } }>('codex_save_config', { config: normalized });
      setConfig(saved?.processing?.codex ?? normalized);
      return normalized;
    } catch { throw new SaveError('Could not save the Codex settings'); }
  }, []);

  const save = useCallback(async (next: CodexConfig) => {
    if (!canStart.current) throw new SaveError('Select Use Codex before saving Codex settings');
    return persistConfig(next);
  }, [persistConfig]);

  const check = useCallback(async (next?: CodexConfig) => {
    if (!canStart.current) return;
    setBusy(true);
    try {
      if (next) await save(next);
      const status = await invoke<RuntimeStatus>('codex_check_installation');
      setRuntime(status);
      setResult(status.message);
      if (status.found) {
        await loadModels();
        toast.success('Bundled Codex runtime found');
      } else toast.error(status.message || 'Bundled Codex runtime is missing or damaged');
    } catch (error) {
      const message = safeMessage(error, 'Could not check the bundled Codex runtime');
      setResult(message);
      toast.error(message);
    } finally { setBusy(false); }
  }, [loadModels, save]);

  useEffect(() => {
    setTimeoutDraft(null);
    if (!active) return;
    let current = true;
    void (async () => {
      try {
        const saved = await invoke<{ processing?: { codex?: CodexConfig } }>('codex_get_config');
        if (!current) return;
        const codex = saved.processing?.codex;
        if (codex) {
          setConfig(codex);
          if (codex.model) updateConfig(prev => prev.provider === 'codex' && prev.model !== codex.model ? { ...prev, model: codex.model } : prev);
        }
      } catch {
        if (!current) return;
        setResult('Could not load the Codex settings');
      }
      if (!preview) await check();
    })();
    return () => { current = false; };
  }, [active, preview, updateConfig, check]);

  const findRuntime = async () => {
    if (!canStart.current) return;
    setBusy(true);
    try {
      await save(config);
      const status = await invoke<RuntimeStatus>('codex_find_automatically');
      setRuntime(status);
      setResult(status.message);
      if (status.found && status.path) {
        await loadModels();
        toast.success('Bundled Codex runtime found');
      } else toast.error(status.message || 'Bundled Codex runtime is missing or damaged');
    } catch { toast.error('Could not find the bundled Codex runtime'); }
    finally { setBusy(false); }
  };

  const repair = async () => {
    setBusy(true);
    try {
      const plan = await invoke<RepairPlan>('codex_prepare_install_command');
      const format = (item: RepairCommand) => `${item.label} (${item.shell}):\n${item.command}`;
      setResult([plan.message, format(plan.recommended), ...plan.alternatives.map(format), `Docs: ${plan.docsUrl}`].join('\n\n'));
      toast.info('Codex app-server repair information prepared');
    } catch { toast.error('Could not prepare Codex repair information'); }
    finally { setBusy(false); }
  };

  const run = async (command: CodexAction) => {
    if (!canStart.current) return;
    setBusy(true);
    try {
      await save(config);
      const response = await invoke<CommandStatus>(command);
      const message = response.message || (response.success ? 'Codex command completed' : 'Codex command failed');
      if (response.success) toast.success(message);
      else toast.error(message);
      await check();
      setResult([message, response.stdout, response.stderr].filter(Boolean).join('\n').trim());
    } catch { toast.error('Could not run the Codex command'); }
    finally { setBusy(false); }
  };

  const chooseHome = async () => {
    if (!canStart.current) return;
    try {
      const folder = await invoke<string | null>('pick_codex_home_folder');
      if (folder) await save({ ...config, codexHomePath: folder });
    } catch (error) { toast.error(safeMessage(error, 'Could not use that folder')); }
  };

  const configured = config.model || selected || DEFAULT_MODEL;
  const options = models.some(model => model.id === configured) ? models
    : [{ id: configured, displayName: configured, isDefault: false }, ...models];
  const signedIn = !['', 'unauthenticated', 'not-required'].includes(runtime?.authStatus?.trim().toLowerCase() ?? '');
  const status: ProviderStatus = busy ? { tone: 'neutral', text: 'Checking…' }
    : !runtime ? { tone: 'neutral', text: 'Not checked yet' }
    : runtime.found ? { tone: 'ok', text: 'Runtime found', meta: [runtime.version, runtime.planType].filter(Boolean).join(' · ') || undefined }
    : { tone: 'warning', text: 'Runtime missing or damaged' };

  return {
    config, timeout, setTimeoutDraft, timeoutError: timeoutProblem(timeout),
    runtime, busy, loading, modelError, options, result, signedIn, status, save, run, check: () => check(config),
    saveModelAfterSwitch: (model: string) => persistConfig({ ...config, model }),
    findRuntime, repair, chooseHome,
  };
}

type CodexState = ReturnType<typeof useCodexSettings>;

export function CodexSettings({ state, model, chooseModel, preview, disabled, feedback }: {
  state: CodexState; model: string; chooseModel: (model: string) => void; preview: boolean; disabled: boolean; feedback?: FeedbackState;
}) {
  return <>
    <SimpleModelSelect value={model} options={state.options.map(option => ({
      value: option.id, label: `${option.displayName}${option.isDefault ? ' · recommended' : ''}`,
    }))} onChoose={chooseModel} loading={state.loading} disabled={disabled} feedback={feedback} />
    <SettingsRow title="Account"
      description={preview ? 'Select Use Codex to check your account and runtime' : state.modelError
        ? 'The model list is unavailable, so only the configured model is shown' : (state.runtime?.accountEmail || state.runtime?.authStatus || 'Not signed in')}>
      <Button type="button" variant="outline" onClick={() => void state.run('codex_login_browser')} disabled={state.busy || preview}>
        <ExternalLink aria-hidden="true" />Sign in with ChatGPT
      </Button>
      <Button type="button" variant="outline" onClick={() => void state.run('codex_logout')} disabled={state.busy || preview || !state.signedIn}>Sign out</Button>
    </SettingsRow>
  </>;
}

export function CodexAdvanced({ state, autosave, preview }: { state: CodexState; autosave: boolean; preview: boolean }) {
  const { config, timeout, setTimeoutDraft, runtime, busy, result, run, save } = state;
  return <>
    <SettingsRow title="Isolated Codex profile" description={<>
      A new or empty folder on a local drive for ClawScribe’s Codex profile
      <Meta className="mt-1 block break-all">{config.codexHomePath || DEFAULT_HOME}</Meta>
    </>}>
      <Button type="button" variant="outline" onClick={() => void state.chooseHome()} disabled={busy || preview}>Choose folder</Button>
    </SettingsRow>
    <ProviderTextField id="codex-timeout" title="Timeout" description="Seconds, 30 or more" inputMode="numeric"
      value={timeout} disabled={preview || busy} validate={timeoutProblem}
      onCommit={async value => { await save({ ...config, timeoutSeconds: Number(value) }); }}
      onDraft={autosave ? undefined : setTimeoutDraft} />
    <SettingsRow title="Runtime" description={runtime?.found ? runtime.version || 'Bundled Codex app-server found' : (runtime?.message || 'Bundled Codex runtime has not been checked')}>
      <Button type="button" variant="outline" onClick={() => void state.findRuntime()} disabled={busy || preview}>Check bundled runtime</Button>
      <Button type="button" variant="outline" onClick={() => void state.repair()} disabled={busy}><Wrench aria-hidden="true" />Install or repair</Button>
    </SettingsRow>
    <SettingsRow title="Tests" description="Check the app-server or run a test meeting through it">
      <Button type="button" variant="outline" onClick={() => void run('codex_test_app_server')} disabled={busy || preview}>Test app-server</Button>
      <Button type="button" variant="outline" onClick={() => void run('codex_test_processing')} disabled={busy || preview}>Test meeting processing</Button>
    </SettingsRow>
    <SettingsRow title="Other sign-in" description="Use a device code when browser sign-in is unavailable">
      <Button type="button" variant="outline" onClick={() => void run('codex_login_device')} disabled={busy || preview}>Sign in with device code</Button>
      <Button type="button" variant="ghost" onClick={() => void openExternal('https://github.com/ch3573r/ClawScribe/blob/main/docs/auth/codex-auth.md')}>
        <ExternalLink aria-hidden="true" />Auth docs
      </Button>
    </SettingsRow>
    <SettingsRow title="Details" description={<>
      <span className="block break-all">CODEX_HOME: {runtime?.codexHome || config.codexHomePath || DEFAULT_HOME}</span>
      <span className="block">Runtime: {runtime?.runtimeKind || 'codex-app-server'} · Account: {runtime?.accountEmail || runtime?.authStatus || 'not signed in'}{runtime?.planType ? ` · Plan: ${runtime.planType}` : ''}{runtime?.rateLimitState ? ` · Rate limit: ${runtime.rateLimitState}` : ''}</span>
      {runtime?.path && <span className="block break-all">Path: {runtime.path}</span>}
      {runtime?.runtimeSha256 && <span className="block break-all">SHA256: {runtime.runtimeSha256}</span>}
    </>} />
    {result && <div className="px-5 py-3"><pre className="whitespace-pre-wrap break-all text-xs text-muted-foreground">{result}</pre></div>}
  </>;
}
