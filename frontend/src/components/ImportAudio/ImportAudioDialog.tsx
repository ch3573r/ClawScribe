'use client';

import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRouter } from 'next/navigation';
import { useConfig } from '@/contexts/ConfigContext';
import { useTranscriptionModels } from '@/hooks/useTranscriptionModels';
import { importSummarySelection, useSummaryDestination, type ImportSummaryChoice } from '@/hooks/useSummaryDestination';
import { useImportQueue, type AudioFileInfo, type ImportQueueItem } from '@/hooks/useImportAudio';
import { errorText } from '@/lib/errorText';
import { readPinnedSummaryLanguageDefault } from '@/lib/summary-language-preferences';
import { LANGUAGES } from '@/constants/languages';
import { providerLabel } from '@/components/settings/provider-data';
import { Button } from '../ui/button';
import { Checkbox } from '../ui/checkbox';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '../ui/dialog';
import { Input } from '../ui/input';
import { Progress } from '../ui/progress';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../ui/select';

interface ImportAudioDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  preselectedFiles?: string[];
}

type Draft = { file: AudioFileInfo; title: string };
type CloudDestination = { endpoint: string; fingerprint: string };
const isCloud = (provider?: string | null) => provider === 'cloud-whisper' || provider === 'mai-transcribe';

const statusLabels: Record<ImportQueueItem['status'], string> = {
  queued: 'Waiting for the transcription engine',
  running: 'Importing',
  cancelling: 'Cancelling',
  needs_source: 'Needs original file',
  cancelled: 'Cancelled',
  failed: 'Failed',
  saved: 'Meeting saved',
};

export function ImportAudioDialog({ open, onOpenChange, preselectedFiles }: ImportAudioDialogProps) {
  const router = useRouter();
  const { selectedLanguage, transcriptModelConfig, modelConfig, isAutoSummary } = useConfig();
  const { items, error: queueError, recoveryNotice, loading } = useImportQueue(open);
  const { availableModels, selectedModelKey, setSelectedModelKey, loadingModels, fetchModels } = useTranscriptionModels(transcriptModelConfig);
  const model = availableModels.find(option => `${option.provider}:${option.name}` === selectedModelKey);
  const cloudProvider = isCloud(model?.provider) ? model?.provider : undefined;
  const { destination: summaryDestination, error: summaryError } = useSummaryDestination(open, modelConfig, isAutoSummary);
  const [summaryChoice, setSummaryChoice] = useState<ImportSummaryChoice>('default');
  const summary = importSummarySelection(summaryChoice, isAutoSummary, summaryDestination);
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [language, setLanguage] = useState(selectedLanguage || 'auto');
  const [meetingDate, setMeetingDate] = useState('');
  const [useFileDate, setUseFileDate] = useState(false);
  const [destination, setDestination] = useState<CloudDestination | null>(null);
  const [destinationError, setDestinationError] = useState<string | null>(null);
  const [approvedDestination, setApprovedDestination] = useState<string | null>(null);
  const [retryDraft, setRetryDraft] = useState<{ id: string; destination: CloudDestination; chooseFile: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const inFlight = useRef(false);
  const lastDropped = useRef<string[] | undefined>(undefined);

  useEffect(() => { if (open) void fetchModels(); }, [open, fetchModels]);

  useEffect(() => {
    let active = true;
    setDestination(null);
    setApprovedDestination(null);
    setDestinationError(null);
    if (open && cloudProvider) {
      void invoke<CloudDestination>('import_cloud_destination', { provider: cloudProvider }).then(
        value => { if (active) setDestination(value); },
        caught => { if (active) setDestinationError(errorText(caught, 'Could not read the cloud destination')); },
      );
    }
    return () => { active = false; };
  }, [open, cloudProvider]);

  function addDrafts(files: AudioFileInfo[]) {
    setDrafts(current => {
      const paths = new Set(current.map(({ file }) => file.path));
      return [...current, ...files.filter(file => !paths.has(file.path)).map(file => ({ file, title: file.filename }))];
    });
    setApprovedDestination(null);
  }

  useEffect(() => {
    if (!open || !preselectedFiles?.length || lastDropped.current === preselectedFiles) return;
    lastDropped.current = preselectedFiles;
    let active = true;
    void Promise.allSettled(preselectedFiles.map(path => invoke<AudioFileInfo>('validate_audio_file_command', { path })))
      .then(results => {
        if (!active) return;
        addDrafts(results.flatMap(result => result.status === 'fulfilled' ? [result.value] : []));
        const failed = results.find(result => result.status === 'rejected');
        if (failed) setError(errorText(failed.reason, 'Could not validate the dropped file'));
      });
    return () => { active = false; if (lastDropped.current === preselectedFiles) lastDropped.current = undefined; };
  }, [open, preselectedFiles]);

  async function perform(action: string, work: () => Promise<void>) {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(action);
    setError(null);
    try { await work(); }
    catch (caught) { setError(errorText(caught, 'Could not update the import queue')); }
    finally { inFlight.current = false; setBusy(null); }
  }

  async function enqueue() {
    const date = meetingDate && !useFileDate ? new Date(meetingDate) : null;
    if (date && Number.isNaN(date.getTime())) throw new Error('Choose a valid meeting date and time');
    const submitted = new Set(drafts.map(({ file }) => file.path));
    await invoke('enqueue_imports', {
      files: drafts.map(({ file, title }) => ({ sourcePath: file.path, title })),
      options: {
        language: model?.provider === 'parakeet' || language === 'auto' ? null : language,
        model: model?.name ?? null,
        provider: model?.provider ?? null,
        meetingAt: date?.toISOString() ?? null,
        useFileDate,
        summarize: summary !== null,
        summaryDestinationFingerprint: summary?.fingerprint ?? null,
        summaryUseAutomaticPreference: summary?.useAutomaticPreference ?? false,
        summaryLanguage: readPinnedSummaryLanguageDefault(),
        cloudDestination: cloudProvider ? approvedDestination : null,
      },
    });
    setDrafts(current => current.filter(({ file }) => !submitted.has(file.path)));
    setApprovedDestination(null);
  }

  async function retry(item: ImportQueueItem, chooseFile: boolean, fingerprint?: string) {
    let source: AudioFileInfo | null = null;
    if (chooseFile) {
      source = await invoke<AudioFileInfo | null>('select_and_validate_audio_command');
      if (!source) return;
    }
    await invoke('retry_import_item', { id: item.id, sourcePath: source?.path ?? null, cloudDestination: fingerprint ?? null });
    setRetryDraft(null);
  }

  async function prepareRetry(item: ImportQueueItem, chooseFile = item.requires_file) {
    if (isCloud(item.provider)) {
      const reviewed = await invoke<CloudDestination>('import_cloud_destination', { provider: item.provider });
      setRetryDraft({ id: item.id, destination: reviewed, chooseFile });
    } else {
      await retry(item, chooseFile);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Import audio or video</DialogTitle>
          <DialogDescription>Each file creates one meeting. Imports run one at a time and continue when you close this dialog.</DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {queueError && <p role="alert" className="text-sm text-destructive">{queueError}</p>}
          {recoveryNotice && <p role="status" className="text-sm text-warning-foreground">{recoveryNotice}</p>}
          {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
          {loading && <p role="status" className="text-sm text-muted-foreground">Loading imports…</p>}
          {items.length > 0 && <section className="space-y-2" aria-label="Import queue">
            <div className="flex items-center justify-between gap-2">
              <h3 className="text-sm font-medium">Import queue</h3>
              {items.some(item => ['saved', 'failed', 'cancelled'].includes(item.status)) &&
                <Button variant="ghost" size="sm" disabled={busy !== null} onClick={() => void perform('clear', () => invoke('clear_finished_imports'))}>Clear finished items</Button>}
            </div>
            <ul className="divide-y divide-border rounded-md border border-border">
              {items.map(item => <li key={item.id} className="space-y-2 p-3">
                <div className="flex flex-wrap items-start justify-between gap-2">
                  <div className="min-w-0 flex-1">
                    <p className="break-words text-sm font-medium">{item.title}</p>
                    <p className="break-words text-xs text-muted-foreground">
                      {item.status === 'cancelling' ? statusLabels.cancelling : item.progress?.message ?? statusLabels[item.status]}
                      {item.result && (item.result.segments_count === 0 ? ' · no speech detected' : ` · ${item.result.segments_count} segments`)}
                      {item.result?.transcription_provider && isCloud(item.provider) && !isCloud(item.result.transcription_provider) && ' · transcribed locally'}
                      {item.elapsed_seconds && item.result?.duration_seconds ? ` · ${(item.result.duration_seconds / item.elapsed_seconds).toFixed(1)}× realtime` : ''}
                    </p>
                    {item.message && <p className="break-words text-xs text-muted-foreground">{item.message}</p>}
                    {item.result?.files_pending && <p className="text-xs text-warning-foreground">The meeting is saved. Open it to retry the folder copy.</p>}
                    {item.result?.transcription_incomplete && <p className="text-xs text-warning-foreground">Some audio could not be transcribed. Open the meeting to review it.</p>}
                  </div>
                  {item.result && <Button variant="outline" size="sm" onClick={() => {
                    onOpenChange(false);
                    router.push(`/meeting-details?id=${encodeURIComponent(item.result!.meeting_id)}`);
                  }}>Open meeting</Button>}
                  {['queued', 'running', 'needs_source'].includes(item.status) &&
                    <Button variant="ghost" size="sm" disabled={busy !== null} aria-label={`Cancel import of ${item.title}`} onClick={() => void perform(item.id, () => invoke('cancel_import_item', { id: item.id }))}>Cancel</Button>}
                  {['failed', 'cancelled', 'needs_source'].includes(item.status) && retryDraft?.id !== item.id &&
                    <Button variant="outline" size="sm" disabled={busy !== null} onClick={() => void perform(item.id, () => prepareRetry(item))}>
                      {item.requires_file ? 'Choose file to retry' : 'Retry'}
                    </Button>}
                  {item.status === 'failed' && !item.requires_file && retryDraft?.id !== item.id &&
                    <Button variant="ghost" size="sm" disabled={busy !== null} onClick={() => void perform(item.id, () => prepareRetry(item, true))}>Choose original</Button>}
                </div>
                {item.progress && <Progress value={item.progress.progress_percentage} aria-valuenow={item.progress.progress_percentage} aria-label={`Import progress for ${item.title}`} />}
                {retryDraft?.id === item.id && <div className="space-y-2 rounded-md bg-muted p-2 text-xs">
                  <p className="break-all">Retrying sends this audio to {retryDraft.destination.endpoint}.</p>
                  <Button size="sm" variant="outline" disabled={busy !== null}
                    onClick={() => void perform(item.id, () => retry(item, retryDraft.chooseFile, retryDraft.destination.fingerprint))}>
                    {retryDraft.chooseFile ? 'Choose file and approve upload' : 'Approve upload and retry'}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setRetryDraft(null)}>Dismiss</Button>
                </div>}
              </li>)}
            </ul>
          </section>}

          <section className="space-y-3" aria-label="Files to import">
            <Button variant="outline" disabled={busy !== null} onClick={() => void perform('select', async () => {
              addDrafts(await invoke<AudioFileInfo[]>('select_audio_files_command'));
            })}>Choose files</Button>
            {drafts.length > 0 && <>
              <div className="space-y-2">
                {drafts.map(({ file, title }, index) => <div key={file.path} className="flex items-end gap-2">
                  <label className="min-w-0 flex-1 text-sm">
                    <span className="block truncate text-xs text-muted-foreground">{file.filename}</span>
                    <Input aria-label={`Meeting title for ${file.filename}`} value={title} maxLength={150}
                      onChange={event => setDrafts(current => current.map(draft => draft.file.path === file.path ? { ...draft, title: event.target.value } : draft))} />
                  </label>
                  <Button variant="ghost" size="sm" disabled={busy !== null} aria-label={`Remove ${file.filename}`} onClick={() => setDrafts(current => current.filter((_, i) => i !== index))}>Remove</Button>
                </div>)}
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1 text-sm">
                  <label htmlFor="import-date">Meeting date and time (local)</label>
                  <Input id="import-date" type="datetime-local" value={meetingDate} disabled={useFileDate} onChange={event => setMeetingDate(event.target.value)} />
                  <label className="flex items-center gap-2 text-xs text-muted-foreground">
                    <Checkbox checked={useFileDate} onCheckedChange={value => setUseFileDate(value === true)} /> Use each file’s modified time
                  </label>
                  {!useFileDate && <p className="text-xs text-muted-foreground">Uses the selected date for all files, or the import time if left blank.</p>}
                </div>
                <div className="space-y-1 text-sm">
                  <label htmlFor="import-model">Transcription model</label>
                  <Select value={selectedModelKey} onValueChange={setSelectedModelKey} disabled={loadingModels}>
                    <SelectTrigger id="import-model"><SelectValue placeholder={loadingModels ? 'Loading models…' : 'Default model'} /></SelectTrigger>
                    <SelectContent>{availableModels.map(option => <SelectItem key={`${option.provider}:${option.name}`} value={`${option.provider}:${option.name}`}>{option.displayName}</SelectItem>)}</SelectContent>
                  </Select>
                  <label htmlFor="import-language">Language</label>
                  <Select value={model?.provider === 'parakeet' ? 'auto' : language} onValueChange={setLanguage} disabled={model?.provider === 'parakeet'}>
                    <SelectTrigger id="import-language"><SelectValue /></SelectTrigger>
                    <SelectContent>{LANGUAGES.map(option => <SelectItem key={option.code} value={option.code}>{option.name}</SelectItem>)}</SelectContent>
                  </Select>
                </div>
              </div>
              {cloudProvider && <div className="space-y-1 rounded-md border border-border p-2 text-sm">
                {destination ? <label className="flex items-start gap-2">
                  <Checkbox checked={approvedDestination === destination.fingerprint} onCheckedChange={checked => setApprovedDestination(checked === true ? destination.fingerprint : null)} />
                  <span className="break-all">Allow all {drafts.length} selected files to upload to {destination.endpoint} for transcription.</span>
                </label> : <p>{destinationError ?? 'Checking the cloud transcription destination…'}</p>}
              </div>}
              <div className="flex items-start gap-2">
                <Checkbox id="import-summarize" checked={summary !== null} disabled={!summaryDestination}
                  onCheckedChange={checked => setSummaryChoice(checked === true && summaryDestination ? { fingerprint: summaryDestination.fingerprint } : 'off')} />
                <div className="min-w-0">
                  <label htmlFor="import-summarize" className="text-sm font-medium">Generate a summary for each meeting</label>
                  <p className="break-all text-xs text-muted-foreground">{summaryDestination ? summaryDestination.external
                    ? `Sends the transcripts and notes context to ${providerLabel(summaryDestination.provider)} (${summaryDestination.endpoint}). The context includes each meeting’s local date and weekday, duration when known, and title when permitted.`
                    : `Generates summaries locally with ${providerLabel(summaryDestination.provider)}.`
                    : summaryError ?? 'Checking the summary provider…'}</p>
                </div>
              </div>
            </>}
          </section>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Close</Button>
          {drafts.length > 0 && <Button disabled={busy !== null || loading || Boolean(queueError) || Boolean(cloudProvider && (!destination || approvedDestination !== destination.fingerprint))}
            onClick={() => void perform('enqueue', enqueue)}>{busy === 'enqueue' ? 'Adding files…' : `Import ${drafts.length === 1 ? 'file' : `${drafts.length} files`}`}</Button>}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
