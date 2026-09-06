import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';

export interface TemplateInfo { id: string; name: string; description: string }

export function useTemplates() {
  const [availableTemplates, setAvailableTemplates] = useState<TemplateInfo[]>([]);
  const [selectedTemplate, setSelectedTemplate] = useState('');
  const [defaultTemplate, setDefaultTemplate] = useState('');
  const [isLoading, setIsLoading] = useState(true);
  const chosen = useRef<string | null>(null);

  useEffect(() => {
    let active = true;
    let version = 0;
    const refresh = async () => {
      const request = ++version;
      try {
        const [templates, defaultId] = await Promise.all([
          invoke<TemplateInfo[]>('api_list_templates'),
          invoke<string>('api_get_default_template'),
        ]);
        if (!active || request !== version) return;
        setAvailableTemplates(templates);
        setDefaultTemplate(defaultId);
        if (chosen.current && !templates.some(t => t.id === chosen.current)) chosen.current = null;
        setSelectedTemplate(chosen.current ?? defaultId);
      } catch {
        if (active && request === version) toast.error('Could not load summary templates. Reopen Settings to retry.');
      } finally {
        if (active && request === version) setIsLoading(false);
      }
    };
    void refresh();
    const subscription = listen('summary-templates-changed', () => { void refresh(); });
    return () => { active = false; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, []);

  const handleTemplateSelection = useCallback((id: string, _name?: string) => {
    chosen.current = id;
    setSelectedTemplate(id);
  }, []);

  return { availableTemplates, selectedTemplate, defaultTemplate, isLoading, handleTemplateSelection };
}