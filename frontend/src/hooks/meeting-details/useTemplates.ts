import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { MEETING_SERIES_TEMPLATE_CHANGED } from '@/lib/meetingCalendarStore';

export interface TemplateInfo {
  id: string; name: string; description: string; personal: boolean; bundled: boolean;
}

export function useTemplates(meetingId?: string) {
  const [availableTemplates, setAvailableTemplates] = useState<TemplateInfo[]>([]);
  const [selectedTemplate, setSelectedTemplate] = useState('');
  const [defaultTemplate, setDefaultTemplate] = useState('');
  const [seriesTemplate, setSeriesTemplate] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const chosen = useRef<string | null>(null);

  useEffect(() => {
    let active = true;
    let version = 0;
    chosen.current = null;
    const refresh = async () => {
      const request = ++version;
      try {
        const [templates, defaultId, seriesId] = await Promise.all([
          invoke<TemplateInfo[]>('api_list_templates'),
          invoke<string>('api_get_default_template'),
          meetingId ? invoke<string | null>('get_meeting_series_template', { meetingId }) : null,
        ]);
        if (!active || request !== version) return;
        setAvailableTemplates(templates);
        setDefaultTemplate(defaultId);
        setSeriesTemplate(seriesId);
        if (chosen.current && !templates.some(t => t.id === chosen.current)) chosen.current = null;
        setSelectedTemplate(chosen.current ?? seriesId ?? defaultId);
      } catch {
        if (active && request === version) toast.error('Could not load summary templates. Reopen Settings to retry.');
      } finally {
        if (active && request === version) setIsLoading(false);
      }
    };
    void refresh();
    const subscription = listen('summary-templates-changed', () => { void refresh(); });
    const refreshSeries = () => { void refresh(); };
    window.addEventListener(MEETING_SERIES_TEMPLATE_CHANGED, refreshSeries);
    return () => {
      active = false;
      window.removeEventListener(MEETING_SERIES_TEMPLATE_CHANGED, refreshSeries);
      void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, [meetingId]);

  const handleTemplateSelection = useCallback((id: string) => {
    chosen.current = id;
    setSelectedTemplate(id);
  }, []);

  return { availableTemplates, selectedTemplate, requestedTemplate: chosen.current ?? '',
    defaultTemplate, seriesTemplate, isLoading, handleTemplateSelection };
}
