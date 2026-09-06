'use client';

import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Plus, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';

interface Section {
  title: string; instruction: string; format: string;
  item_format?: string; example_item_format?: string;
}
interface Template { name: string; description: string; sections: Section[] }
const emptyTemplate = (): Template => ({
  name: '', description: '', sections: [{ title: 'Summary', instruction: 'Summarize the discussion and agreed decisions.', format: 'paragraph' }],
});
const fieldClass = 'w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring';

export function SummaryTemplateSettings() {
  const templates = useTemplates();
  const [editingId, setEditingId] = useState('');
  const [draft, setDraft] = useState<Template | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const loadVersion = useRef(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; loadVersion.current++; }; }, []);

  const open = async (id: string) => {
    const request = ++loadVersion.current;
    setBusy(true); setError('');
    try {
      const value = await invoke<Template>('api_get_template_editor', { templateId: id });
      if (!mounted.current || request !== loadVersion.current) return;
      setEditingId(id); setDraft(value); setDirty(false);
    } catch { if (mounted.current && request === loadVersion.current) setError('Could not load the template. Choose it again to retry.'); }
    finally { if (mounted.current && request === loadVersion.current) setBusy(false); }
  };

  const update = (value: Template) => { setDraft(value); setDirty(true); setError(''); };
  const updateSection = (index: number, value: Partial<Section>) => {
    if (draft) update({ ...draft, sections: draft.sections.map((section, i) => i === index ? { ...section, ...value } : section) });
  };
  const save = async () => {
    if (!draft || busy) return;
    setBusy(true); setError('');
    const id = editingId || `custom-${crypto.randomUUID()}`;
    try {
      await invoke('api_save_template', { templateId: id, template: draft });
      if (!mounted.current) return;
      setEditingId(id); setDirty(false); toast.success('Template saved');
    } catch (e) { if (mounted.current) setError(typeof e === 'string' ? e : 'Could not save the template. Your changes are retained.'); }
    finally { if (mounted.current) setBusy(false); }
  };
  const setDefault = async (id: string) => {
    setBusy(true); setError('');
    try { await invoke('api_save_default_template', { templateId: id }); toast.success('Default template saved'); }
    catch { if (mounted.current) setError('Could not save the default template. Please retry.'); }
    finally { if (mounted.current) setBusy(false); }
  };

  return <section className="space-y-4 rounded-lg border border-border bg-card p-6">
    <div><h3 className="text-lg font-semibold">Summary templates</h3>
      <p className="text-sm text-muted-foreground">Choose the default for new summaries, or adapt the sections and instructions to your meetings.</p></div>
    <label className="block space-y-1 text-sm font-medium">Default template
      <select className={fieldClass} value={templates.defaultTemplate} disabled={busy || templates.isLoading || dirty} onChange={e => void setDefault(e.target.value)}>
        {!templates.defaultTemplate && <option value="">Loading templates…</option>}
        {templates.availableTemplates.map(t => <option key={t.id} value={t.id}>{t.name}</option>)}
      </select>
    </label>
    <div className="flex flex-wrap items-end gap-2">
      <label className="min-w-0 flex-1 space-y-1 text-sm font-medium">Edit a template
        <select className={fieldClass} value={editingId} disabled={busy || dirty || templates.isLoading} onChange={e => e.target.value && void open(e.target.value)}>
          <option value="">Choose a template</option>
          {templates.availableTemplates.map(t => <option key={t.id} value={t.id}>{t.name}</option>)}
        </select>
      </label>
      <Button variant="outline" disabled={busy || dirty} onClick={() => { loadVersion.current++; setEditingId(''); setDraft(emptyTemplate()); setDirty(true); setError(''); }}><Plus className="mr-2 h-4 w-4" />New template</Button>
    </div>
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    {draft && <fieldset disabled={busy} className="space-y-4">
      <label className="block space-y-1 text-sm font-medium">Name<Input maxLength={200} value={draft.name} onChange={e => update({ ...draft, name: e.target.value })} /></label>
      <label className="block space-y-1 text-sm font-medium">Description<Textarea maxLength={4000} value={draft.description} onChange={e => update({ ...draft, description: e.target.value })} /></label>
      {draft.sections.map((section, index) => <div key={index} className="space-y-3 rounded-md border border-border p-3">
        <div className="flex items-center justify-between"><span className="text-sm font-semibold">Section {index + 1}</span>
          <Button variant="ghost" size="icon" aria-label={`Remove section ${index + 1}`} disabled={draft.sections.length === 1} onClick={() => update({ ...draft, sections: draft.sections.filter((_, i) => i !== index) })}><Trash2 className="h-4 w-4" /></Button></div>
        <label className="block space-y-1 text-sm">Heading<Input maxLength={200} value={section.title} onChange={e => updateSection(index, { title: e.target.value })} /></label>
        <label className="block space-y-1 text-sm">Instructions<Textarea rows={3} maxLength={16000} value={section.instruction} onChange={e => updateSection(index, { instruction: e.target.value })} /></label>
        <label className="block space-y-1 text-sm">Format<select className={fieldClass} value={section.format} onChange={e => updateSection(index, { format: e.target.value })}><option value="paragraph">Paragraph</option><option value="list">List</option><option value="string">Short text</option></select></label>
        <label className="block space-y-1 text-sm">Formatting example (optional)<Input value={section.item_format ?? section.example_item_format ?? ''} onChange={e => updateSection(index, { item_format: e.target.value, example_item_format: undefined })} /></label>
      </div>)}
      <Button variant="outline" disabled={draft.sections.length >= 30} onClick={() => update({ ...draft, sections: [...draft.sections, { title: '', instruction: '', format: 'list' }] })}><Plus className="mr-2 h-4 w-4" />Add section</Button>
      <p className="text-xs text-muted-foreground">Changes to a built-in template are saved as your personal override. Existing meeting notes stay as they are.</p>
      <div className="flex flex-wrap gap-2"><Button disabled={!dirty || !draft.name.trim() || !draft.description.trim() || draft.sections.some(s => !s.title.trim() || !s.instruction.trim())} onClick={() => void save()}>{busy ? 'Saving…' : 'Save template'}</Button>
        <Button variant="outline" onClick={() => { setDraft(null); setEditingId(''); setDirty(false); setError(''); }}>Discard changes</Button></div>
    </fieldset>}
  </section>;
}
