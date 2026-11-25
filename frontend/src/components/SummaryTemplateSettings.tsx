'use client';

import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { confirm } from '@tauri-apps/plugin-dialog';
import { Plus, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { SettingsFooter, SettingsRow } from '@/components/ui/settings-layout';
import { ROW_SELECT } from '@/components/settings/settings-parts';
import { SaveMark, withError } from '@/components/settings/summary-provider-parts';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';

interface Section {
  title: string; instruction: string; format: string;
  item_format?: string; example_item_format?: string;
}
interface Template { name: string; description: string; sections: Section[] }
const emptyTemplate = (): Template => ({
  name: '', description: '', sections: [{ title: 'Summary', instruction: 'Summarize the discussion and agreed decisions.', format: 'paragraph' }],
});
const EMPTY_TEMPLATE_SELECTION = '__clawscribe-empty-template-selection__';

export function SummaryTemplateSettings() {
  const templates = useTemplates();
  const [editingId, setEditingId] = useState('');
  const [draft, setDraft] = useState<Template | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [dialogOpen, setDialogOpen] = useState(false);
  const [defaultStatus, setDefaultStatus] = useState<'idle' | 'saving' | 'saved'>('idle');
  const [defaultError, setDefaultError] = useState('');
  const loadVersion = useRef(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; loadVersion.current += 1; }; }, []);
  useEffect(() => {
    if (typeof window !== 'undefined' && new URLSearchParams(window.location?.search).get('manageTemplates') === '1') {
      setDialogOpen(true);
    }
  }, []);

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
  // The default template saves as soon as it is chosen.
  const setDefault = async (id: string) => {
    setBusy(true); setDefaultError(''); setDefaultStatus('saving');
    try {
      await invoke('api_save_default_template', { templateId: id });
      if (mounted.current) setDefaultStatus('saved');
    } catch {
      if (mounted.current) { setDefaultError('Could not save the default template. Try again'); setDefaultStatus('idle'); }
    } finally { if (mounted.current) setBusy(false); }
  };
  const closeEditor = () => { loadVersion.current += 1; setDraft(null); setEditingId(''); setDirty(false); setError(''); };
  const savedTemplate = templates.availableTemplates.find(template => template.id === editingId);
  const remove = async () => {
    if (!savedTemplate?.personal || busy || dirty) return;
    setBusy(true); setError('');
    const reset = savedTemplate.bundled;
    try {
      const confirmed = await confirm(
        reset ? 'Remove your changes and use the original template?' : 'Delete this personal template? Meetings that used it for future summaries will use the default template.',
        { title: reset ? 'Reset template' : 'Delete template', kind: 'warning' },
      );
      if (!confirmed || !mounted.current) return;
      await invoke('api_remove_template', { templateId: savedTemplate.id });
      if (!mounted.current) return;
      closeEditor();
      toast.success(reset ? 'Template reset' : 'Template deleted');
    } catch {
      if (mounted.current) setError(reset ? 'Could not reset the template. Try again.' : 'Could not delete the template. Try again.');
    } finally {
      if (mounted.current) setBusy(false);
    }
  };

  const defaultSelect = <>
    <SaveMark status={defaultStatus} />
    <Select value={templates.defaultTemplate || EMPTY_TEMPLATE_SELECTION} disabled={busy || templates.isLoading} onValueChange={value => void setDefault(value === EMPTY_TEMPLATE_SELECTION ? '' : value)}>
      <SelectTrigger id="summary-default-template" aria-label="Default template" className={ROW_SELECT}><SelectValue /></SelectTrigger>
      <SelectContent>
        {!templates.defaultTemplate && <SelectItem value={EMPTY_TEMPLATE_SELECTION}>{templates.isLoading ? 'Loading templates…' : 'No default template'}</SelectItem>}
        {templates.availableTemplates.map(t => <SelectItem key={t.id} value={t.id}>{t.name}</SelectItem>)}
      </SelectContent>
    </Select>
  </>;

  return <>
    <SettingsRow title="Default template" htmlFor="summary-default-template" description={withError(null, defaultError)}>{defaultSelect}</SettingsRow>
    <SettingsFooter actions={<Button variant="outline" size="sm" onClick={() => setDialogOpen(true)}>Edit templates</Button>}>
      Templates set the sections and instructions of a summary
    </SettingsFooter>
    <Dialog open={dialogOpen} onOpenChange={open => { if (open) setDialogOpen(true); else if (!dirty && !busy) { closeEditor(); setDialogOpen(false); } }}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Edit templates</DialogTitle>
          <DialogDescription>{dirty ? 'Save or discard your changes to close' : 'Adapt the sections and instructions to your meetings'}</DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="flex flex-wrap items-end gap-2">
            <label className="min-w-0 flex-1 space-y-1 text-sm font-medium">Template
              <Select value={editingId || EMPTY_TEMPLATE_SELECTION} disabled={busy || dirty || templates.isLoading} onValueChange={value => { const id = value === EMPTY_TEMPLATE_SELECTION ? '' : value; if (id) void open(id); }}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value={EMPTY_TEMPLATE_SELECTION}>{templates.isLoading ? 'Loading templates…' : 'Choose a template'}</SelectItem>
                  {templates.availableTemplates.map(t => <SelectItem key={t.id} value={t.id}>{t.name}</SelectItem>)}
                </SelectContent>
              </Select>
            </label>
            <Button variant="outline" disabled={busy || dirty} onClick={() => { loadVersion.current += 1; setEditingId(''); setDraft(emptyTemplate()); setDirty(true); setError(''); }}><Plus className="mr-2 h-4 w-4" />New template</Button>
          </div>
          {busy && !draft && <p role="status" className="text-sm text-muted-foreground">Loading…</p>}
          {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
          {draft && <fieldset disabled={busy} className="space-y-4">
      <label className="block space-y-1 text-sm font-medium">Name<Input maxLength={200} value={draft.name} onChange={e => update({ ...draft, name: e.target.value })} /></label>
      <label className="block space-y-1 text-sm font-medium">Description<Textarea maxLength={4000} value={draft.description} onChange={e => update({ ...draft, description: e.target.value })} /></label>
      {draft.sections.map((section, index) => <div key={index} className="space-y-3 rounded-md border border-border p-3">
        <div className="flex items-center justify-between"><span className="text-sm font-semibold">Section {index + 1}</span>
          <Button variant="ghost" size="icon" aria-label={`Remove section ${index + 1}`} disabled={draft.sections.length === 1} onClick={() => update({ ...draft, sections: draft.sections.filter((_, i) => i !== index) })}><Trash2 className="h-4 w-4" /></Button></div>
        <label className="block space-y-1 text-sm">Heading<Input maxLength={200} value={section.title} onChange={e => updateSection(index, { title: e.target.value })} /></label>
        <label className="block space-y-1 text-sm">Instructions<Textarea rows={3} maxLength={16000} value={section.instruction} onChange={e => updateSection(index, { instruction: e.target.value })} /></label>
        <label className="block space-y-1 text-sm">Format<Select value={section.format} disabled={busy} onValueChange={value => updateSection(index, { format: value })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="paragraph">Paragraph</SelectItem><SelectItem value="list">List</SelectItem><SelectItem value="string">Short text</SelectItem></SelectContent></Select></label>
        <label className="block space-y-1 text-sm">Formatting example (optional)<Input value={section.item_format ?? section.example_item_format ?? ''} onChange={e => updateSection(index, { item_format: e.target.value, example_item_format: undefined })} /></label>
      </div>)}
      <Button variant="outline" disabled={draft.sections.length >= 30} onClick={() => update({ ...draft, sections: [...draft.sections, { title: '', instruction: '', format: 'list' }] })}><Plus className="mr-2 h-4 w-4" />Add section</Button>
      <p className="text-xs text-muted-foreground">Changes to a built-in template are saved as your personal override. Existing meeting notes stay as they are.</p>
      <div className="flex flex-wrap gap-2"><Button disabled={!dirty || !draft.name.trim() || !draft.description.trim() || draft.sections.some(s => !s.title.trim() || !s.instruction.trim())} onClick={() => void save()}>{busy ? 'Saving…' : 'Save template'}</Button>
        {dirty && <Button variant="outline" disabled={busy} onClick={closeEditor}>Discard changes</Button>}
        {savedTemplate?.personal && !dirty && (
          <Button variant="ghost" disabled={busy} onClick={() => void remove()}>
            {savedTemplate.bundled ? 'Reset template' : 'Delete template'}
          </Button>
        )}
      </div>
          </fieldset>}
        </div>
      </DialogContent>
    </Dialog>
  </>;
}
