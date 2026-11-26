import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Check, Download, ExternalLink, RefreshCw } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Progress } from '@/components/ui/progress';
import { Meta, SettingsRow } from '@/components/ui/settings-layout';
import { useOllamaDownload } from '@/contexts/OllamaDownloadContext';
import { openExternal } from '@/lib/openExternal';
import { cn, isOllamaNotInstalledError } from '@/lib/utils';
import { ModelSaveFeedback, ProviderTextField, urlProblem, type ProviderStatus } from './shared';
import type { CommitResult, FeedbackState } from '@/components/settings/commit-field';

interface OllamaModel {
  id: string;
  name: string;
  size: string;
}

const RECOMMENDED_MODEL = 'gemma3:1b';

function validModelName(value: string): boolean {
  // Ollama names can include a registry, namespace and tag.
  const part = '[a-z0-9][a-z0-9._-]*';
  return new RegExp(`^(?:${part}(?::[0-9]+)?/)?(?:${part}/)?${part}(?::${part})?$`, 'i').test(value);
}

export function useOllamaSettings(active: boolean, savedEndpoint: string) {
  const downloads = useOllamaDownload();
  const [endpoint, setEndpoint] = useState(savedEndpoint);
  const [result, setResult] = useState<{ endpoint: string; models: OllamaModel[] } | null>(null);
  const [loading, setLoading] = useState(false);
  const [problem, setProblem] = useState<'missing' | 'unreachable' | 'invalid' | null>(null);
  const [search, setSearch] = useState('');
  const [pullName, setPullName] = useState('');
  const [pullError, setPullError] = useState('');
  const [pendingPull, setPendingPull] = useState('');
  const localPull = useRef('');
  const request = useRef(0);
  const cache = useRef(new Map<string, OllamaModel[]>());
  const current = useRef({ active, endpoint });
  current.current = { active, endpoint };

  const fetchModels = useCallback(async (rawEndpoint: string) => {
    const revision = ++request.current;
    const url = rawEndpoint.trim();
    if (url && urlProblem(url)) {
      setProblem('invalid');
      setLoading(false);
      return;
    }
    setLoading(true);
    setProblem(null);
    try {
      const models = await invoke<OllamaModel[]>('get_ollama_models', { endpoint: url || null });
      if (request.current !== revision) return;
      cache.current.set(url, models);
      setResult({ endpoint: url, models });
    } catch (error) {
      if (request.current !== revision) return;
      const message = error instanceof Error ? error.message : typeof error === 'string' ? error : '';
      setProblem(isOllamaNotInstalledError(message) ? 'missing' : 'unreachable');
    } finally {
      if (request.current === revision) setLoading(false);
    }
  }, []);

  useEffect(() => {
    setEndpoint(savedEndpoint);
    if (active) void fetchModels(savedEndpoint);
    return () => { request.current += 1; };
  }, [active, savedEndpoint, fetchModels]);

  const checked = result?.endpoint === endpoint.trim();
  const models = checked ? result.models : cache.current.get(endpoint.trim()) ?? [];
  const needsCheck = !checked && !cache.current.has(endpoint.trim());
  const check = () => fetchModels(endpoint);

  const previousDownloads = useRef(new Set<string>());
  useEffect(() => {
    const finished = [...previousDownloads.current].some(name => !downloads.downloadingModels.has(name) && name !== localPull.current);
    previousDownloads.current = downloads.downloadingModels;
    if (active && finished && !needsCheck) void fetchModels(endpoint);
  }, [active, downloads.downloadingModels, endpoint, fetchModels, needsCheck]);

  const pull = async (rawName: string) => {
    const name = rawName.trim();
    if (!validModelName(name)) {
      setPullError('Enter a valid Ollama model name, for example qwen3:1b');
      return;
    }
    if (needsCheck || localPull.current || downloads.isDownloading(name)) return;
    localPull.current = name;
    setPendingPull(name);
    setPullError('');
    const url = endpoint.trim();
    try {
      await invoke('pull_ollama_model', { modelName: name, endpoint: url || null });
      if (current.current.active && current.current.endpoint.trim() === url) await fetchModels(url);
    } catch (error) {
      const message = error instanceof Error ? error.message : typeof error === 'string' ? error : '';
      if (current.current.active && current.current.endpoint.trim() === url) {
        if (isOllamaNotInstalledError(message)) setProblem('missing');
        setPullError('Could not pull the model. Check the model name and Ollama connection, then try again');
      }
    } finally {
      localPull.current = '';
      setPendingPull('');
    }
  };

  const status: ProviderStatus = loading ? { tone: 'neutral', text: 'Checking Ollama…' }
    : problem === 'missing' ? { tone: 'error', text: 'Ollama is not installed or not running' }
    : problem === 'invalid' ? { tone: 'error', text: 'Enter an Ollama URL that starts with http:// or https://' }
    : problem === 'unreachable' ? { tone: 'error', text: 'Could not reach Ollama' }
    : needsCheck ? { tone: 'neutral', text: 'Check again to load models from this endpoint' }
    : models.length ? { tone: 'ok', text: 'Connected', meta: `${models.length} ${models.length === 1 ? 'model' : 'models'}` }
    : { tone: 'warning', text: 'No models found' };

  return {
    endpoint, setEndpoint, check, loading, problem, status, models, needsCheck, search, setSearch,
    pullName, setPullName: (name: string) => { setPullName(name); setPullError(''); },
    pullError, pull, pendingPull, downloads,
  };
}

type OllamaState = ReturnType<typeof useOllamaSettings>;

export function OllamaSettings({ ollama, model, chooseModel, feedback, disabled }: {
  ollama: OllamaState; model: string; chooseModel: (model: string) => void; feedback?: FeedbackState; disabled: boolean;
}) {
  const { models, search, problem, loading, needsCheck, pendingPull, downloads, pull, pullName, pullError } = ollama;
  const query = search.trim().toLowerCase();
  const filtered = models.filter(item => item.name.toLowerCase().includes(query) || item.size.toLowerCase().includes(query)
    || (item.name === model && 'loaded'.includes(query)));
  const recommended = models.length === 0;
  const name = recommended ? RECOMMENDED_MODEL : pullName.trim();
  const pulling = (name !== '' && downloads.isDownloading(name)) || pendingPull !== '';
  const pullDisabled = disabled || needsCheck || loading || pulling;
  const submit = () => { if (!pullDisabled) void pull(name); };
  const tracked = [...new Set([...downloads.downloadingModels, ...(pendingPull ? [pendingPull] : [])])];

  return <>
    {!!models.length && <div className="space-y-3 px-5 py-3">
      <SettingsRow title="Installed models" htmlFor="summary-ollama-search" className="min-h-0 px-0 py-0">
        <Input id="summary-ollama-search" aria-label="Search installed models" placeholder="Search models"
          value={search} onChange={event => ollama.setSearch(event.target.value)} />
      </SettingsRow>
      <ModelSaveFeedback feedback={feedback} />
      {!filtered.length && <p className="text-sm text-muted-foreground">No models match your search</p>}
      <div className="grid gap-2 sm:grid-cols-2">
        {filtered.map(item => <Button key={item.id} type="button" variant="outline"
          aria-pressed={model === item.name} disabled={disabled} onClick={() => chooseModel(item.name)}
          className={cn('h-auto min-h-11 min-w-0 justify-start whitespace-normal break-all text-left',
            model === item.name && 'border-primary bg-accent')}>
          {model === item.name && <Check aria-hidden="true" className="shrink-0 text-primary" />}
          <span className="min-w-0 flex-1">{item.name}</span>
          <Meta className="shrink-0 whitespace-nowrap">{item.size}</Meta>
        </Button>)}
      </div>
    </div>}
    {problem === 'missing' ? <SettingsRow title="Ollama" description="Install and start Ollama, then choose Check again">
      <Button type="button" variant="outline" onClick={() => void openExternal('https://ollama.com/download')}>
        <ExternalLink aria-hidden="true" />Download Ollama
      </Button>
    </SettingsRow> : (!recommended || (!loading && !needsCheck)) && (
      <SettingsRow title={recommended ? 'Recommended model' : 'Model to pull'} htmlFor={recommended ? undefined : 'summary-ollama-pull'}
        description={recommended ? `${RECOMMENDED_MODEL}, about 800 MB` : 'Use a model name with an optional tag'}>
        {!recommended && <Input id="summary-ollama-pull" placeholder="qwen3:1b" value={pullName}
          aria-invalid={pullError && !pulling ? true : undefined}
          aria-describedby={pullError ? 'summary-ollama-pull-error' : undefined}
          onChange={event => ollama.setPullName(event.target.value)}
          onKeyDown={event => { if (event.key === 'Enter' && !event.nativeEvent?.isComposing) { event.preventDefault(); submit(); } }} />}
        <Button type="button" variant="outline" onClick={submit} disabled={pullDisabled}>
          {pulling ? <RefreshCw aria-hidden="true" className="animate-spin motion-reduce:animate-none" /> : <Download aria-hidden="true" />}
          {pulling ? 'Downloading…' : recommended ? 'Download' : 'Pull model'}
        </Button>
      </SettingsRow>
    )}
    {pullError && <p id="summary-ollama-pull-error" role="alert" className="px-5 pb-3 text-sm text-error-foreground">{pullError}</p>}
    {tracked.map(name => {
      const progress = downloads.getProgress(name);
      return <div key={name} className="space-y-1 px-5 py-2">
        <p className="text-sm text-muted-foreground">{name} {progress === undefined ? ' · Starting download…' : ` · ${Math.round(progress)}%`}</p>
        {progress !== undefined && <Progress aria-label={`Download progress for ${name}`} value={progress} />}
      </div>;
    })}
  </>;
}

export function OllamaEndpoint({ ollama, autosave, commit }: {
  ollama: OllamaState; autosave: boolean; commit: (value: string) => Promise<CommitResult>;
}) {
  return <ProviderTextField id="summary-ollama-endpoint" title="Endpoint" type="url" placeholder="http://localhost:11434"
    description="Leave empty to use the local Ollama server" value={ollama.endpoint}
    validate={value => value ? urlProblem(value) : null} onCommit={commit}
    onDraft={autosave ? undefined : ollama.setEndpoint} />;
}
