import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ProviderStatus } from './shared';

type Catalog = {
  models: string[];
  status: 'idle' | 'loading' | 'ready' | 'empty' | 'invalid' | 'forbidden' | 'unavailable';
};

const idle: Catalog = { models: [], status: 'idle' };
type CatalogCommand = 'get_openai_models' | 'get_anthropic_models' | 'get_groq_models';

export function useModelCatalog(command: CatalogCommand | undefined, scope: string, auto: boolean, automaticKey: string | null = null, customEndpoint?: string) {
  const [result, setResult] = useState<{ scope: string; catalog: Catalog }>({ scope: '', catalog: idle });
  const generation = useRef(0);
  const pending = useRef<string | null>(null);
  const refresh = useCallback(async (apiKey: string | null = null) => {
    if (!command || pending.current === scope) return;
    pending.current = scope;
    const request = ++generation.current;
    setResult({ scope, catalog: { models: [], status: 'loading' } });
    let catalog: Catalog;
    try {
      const rows = command === 'get_openai_models' && customEndpoint
        ? await invoke<{ id: string }[]>(command, { customEndpoint, apiKey })
        : await invoke<{ id: string }[]>(command, { apiKey });
      catalog = { models: Array.from(new Set(rows.map(model => model.id))), status: rows.length ? 'ready' : 'empty' };
    } catch (error) {
      const code = error instanceof Error ? error.message : error;
      const status = code === 'invalid_api_key' ? 'invalid' : code === 'catalog_forbidden' ? 'forbidden' : 'unavailable';
      catalog = { models: [], status };
    }
    if (generation.current === request) {
      pending.current = null;
      setResult({ scope, catalog });
    }
  }, [command, scope, customEndpoint]);

  useEffect(() => {
    setResult({ scope, catalog: idle });
    if (auto) void refresh(automaticKey);
    return () => { generation.current += 1; pending.current = null; };
  }, [auto, automaticKey, refresh, scope]);

  return { catalog: result.scope === scope ? result.catalog : idle, refresh };
}

export function catalogStatus(status: Catalog['status'], fallback = 'Showing default models.'): ProviderStatus {
  switch (status) {
    case 'loading': return { tone: 'neutral', text: 'Loading models…' };
    case 'invalid': return { tone: 'error', text: 'The provider rejected this API key. Replace it and try again.' };
    case 'forbidden': return { tone: 'warning', text: 'This key cannot list models. Check its permissions.' };
    case 'unavailable': return { tone: 'warning', text: `Could not verify this key. ${fallback}` };
    case 'empty': return { tone: 'warning', text: 'The provider returned no supported chat models.' };
    case 'ready': return { tone: 'ok', text: 'Model list verified with this key' };
    default: return { tone: 'neutral', text: 'Key not checked yet' };
  }
}
