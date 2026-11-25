"use client";

import type { SummaryJobSnapshot } from '@/lib/summary-job-status';

import { Input } from '@/components/ui/input';
import type { EditorSummary, MeetingSummary, Summary, Transcript } from '@/types';
import { BlockNoteSummaryView, BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { MeetingChat } from './MeetingChat';
import { ActionIcon } from './ActionIcon';
import { Textarea } from '@/components/ui/textarea';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { SummarySurface } from '@/components/AISummary/SummarySurface';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { SummaryGeneratorButtonGroup } from './SummaryGeneratorButtonGroup';
import { SummaryUpdaterButtonGroup } from './SummaryUpdaterButtonGroup';
import Analytics from '@/lib/analytics';
import { TextReplaceDialog } from './TextReplaceDialog';
import { SummarySources, ResolvedSummarySource } from './SummarySources';
import { useEffect, useRef, useState, RefObject } from 'react';
import { toast } from 'sonner';
import { Languages, ChevronDown, ChevronUp, Search, Sparkles, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Copy } from 'lucide-react';
import { FollowUpEmail } from './FollowUpEmail';
import { SavedLiveAnswers } from './SavedLiveAnswers';
import { isEditorSummary, LOCAL_SUMMARY_HINT, type summaryDetails } from '@/lib/meeting-summary';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { LanguagePickerPopover } from '@/components/LanguagePickerPopover';
import { useRecentLanguages } from '@/hooks/useRecentLanguages';
import { labelForCode, normaliseLanguageCode } from '@/lib/summary-languages';
import {
  readMeetingSummaryLanguage,
  saveMeetingSummaryLanguage,
  SummaryLanguageStorage,
} from '@/lib/summary-language-preferences';

import type { SummaryDraft, SaveState } from '@/lib/meetingDrafts';

interface SummaryPanelProps {
  seriesLanguage?: string | null;
  details: ReturnType<typeof summaryDetails> & { transcriptChanged: boolean };
  saveState: SaveState;
  hasPreviousSummary: boolean;
  summaryRevision: number;
  onRestorePrevious: () => Promise<void>;
  onSummaryDraft: (draft: SummaryDraft, read: () => Promise<SummaryDraft>) => void;
  onRevealSource?: (source: ResolvedSummarySource) => Promise<void>;
  onPlaySource?: (seconds: number) => Promise<void>;
  meeting: {
    id: string;
    title: string;
    created_at: string;
  };
  meetingTitle: string;
  isTitleDirty: boolean;
  summaryRef: RefObject<BlockNoteSummaryViewRef | null>;
  isSaving: boolean;
  onSaveAll: () => Promise<void>;
  onCopySummary: () => Promise<void>;
  onOpenFolder: () => Promise<void>;
  aiSummary: MeetingSummary | null;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  transcripts: Transcript[];
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onRetryTranslation: (target: string, customPrompt: string) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  contextLoading: boolean;
  onPromptChange: (value: string) => void;
  onSaveSummary: (summary: EditorSummary) => Promise<void>;
  onSummaryChange: (summary: Summary) => void;
  summaryError: string | null;
  jobSnapshot: SummaryJobSnapshot | null;
  getSummaryStatusMessage: (status: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error') => string;
  availableTemplates: Array<{ id: string, name: string, description: string }>;
  selectedTemplate: string;
  onTemplateSelect: (templateId: string, templateName: string) => void;
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
}

type SummaryTextMatch = {
  node: Text;
  start: number;
  end: number;
};

export function SummaryPanel({
  details,
  seriesLanguage,
  saveState, hasPreviousSummary, summaryRevision, onRestorePrevious, onSummaryDraft,
  onRevealSource,
  onPlaySource,
  meeting,
  meetingTitle,
  isTitleDirty,
  summaryRef,
  isSaving,
  onSaveAll,
  onCopySummary,
  onOpenFolder,
  aiSummary,
  summaryStatus,
  transcripts,
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onRetryTranslation,
  onStopGeneration,
  customPrompt, contextLoading, onPromptChange,
  onSaveSummary,
  onSummaryChange,
  summaryError,
  jobSnapshot,
  getSummaryStatusMessage,
  availableTemplates,
  selectedTemplate,
  onTemplateSelect,
  isModelConfigLoading = false,
  onOpenModelSettings
}: SummaryPanelProps) {
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [compactToolbar, setCompactToolbar] = useState(true);
  useEffect(() => {
    const node = toolbarRef.current;
    if (!node) return;
    const measure = () => setCompactToolbar(node.clientWidth < 700);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  // The empty view offers the same generate action as the toolbar.
  const generateRef = useRef<(() => void) | null>(null);
  const [checkingModels, setCheckingModels] = useState(false);
  const [contextOpen, setContextOpen] = useState(false);
  const [dockTarget, setDockTarget] = useState<HTMLDivElement | null>(null);
  const [chatDocked, setChatDocked] = useState(false);
  const [summaryLang, setSummaryLang] = useState<string | null>(null);
  const [summaryLangStorage, setSummaryLangStorage] = useState<SummaryLanguageStorage>('metadata');
  const [langPickerOpen, setLangPickerOpen] = useState(false);
  const [replaceOpen, setReplaceOpen] = useState(false);
  const [emailOpen, setEmailOpen] = useState(false);
  const [isFindOpen, setIsFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState('');
  const [findMatchCount, setFindMatchCount] = useState(0);
  const [activeFindIndex, setActiveFindIndex] = useState(-1);
  const languageLoadVersionRef = useRef(0);
  const activeMeetingIdRef = useRef(meeting.id);
  const languageSaveVersionRef = useRef(0);
  const languageSaveLoopRunningRef = useRef(false);
  const findInputRef = useRef<HTMLInputElement>(null);
  const retainFindFocusRef = useRef(false);
  const summarySearchRootRef = useRef<HTMLDivElement>(null);
  const latestLanguageSaveRequestRef = useRef<{
    version: number;
    meetingId: string;
    language: string | null;
    rollback: {
      language: string | null;
      storage: SummaryLanguageStorage;
    };
  } | null>(null);
  activeMeetingIdRef.current = meeting.id;
  const { addRecent } = useRecentLanguages();

  const effectiveLangLabel = summaryLang ? labelForCode(summaryLang) : seriesLanguage ? labelForCode(seriesLanguage) : 'Auto';
  const isLocalFallbackLanguage = summaryLangStorage === 'local_fallback';
  const autoSubtitle = seriesLanguage ? `Use the series setting · ${labelForCode(seriesLanguage)}` : isLocalFallbackLanguage
    ? 'Saved on this device for folderless meetings'
    : 'Uses dominant transcript language';

  useEffect(() => {
    let cancelled = false;
    const loadVersion = languageLoadVersionRef.current + 1;
    languageLoadVersionRef.current = loadVersion;

    const loadSummaryLanguage = async () => {
      try {
        const stored = await readMeetingSummaryLanguage(meeting.id);
        if (!cancelled && languageLoadVersionRef.current === loadVersion) {
          setSummaryLang(stored.language);
          setSummaryLangStorage(stored.storage);
        }
      } catch (err) {
        console.error('Failed to load summary language:', err);
        toast.warning('Could not load saved summary language', {
          description: 'Using Auto until meeting metadata can be read.',
        });
        if (!cancelled && languageLoadVersionRef.current === loadVersion) setSummaryLang(null);
      }
    };

    loadSummaryLanguage();

    return () => {
      cancelled = true;
    };
  }, [meeting.id]);

  const persistLatestLanguageSelection = async () => {
    if (languageSaveLoopRunningRef.current) return;
    languageSaveLoopRunningRef.current = true;

    try {
      while (true) {
        const request = latestLanguageSaveRequestRef.current;
        if (!request) return;

        try {
          const saved = await saveMeetingSummaryLanguage(request.meetingId, request.language);
          const latest = latestLanguageSaveRequestRef.current;
          if (
            latest?.version === request.version &&
            activeMeetingIdRef.current === request.meetingId
          ) {
            setSummaryLang(saved.language);
            setSummaryLangStorage(saved.storage);
            if (saved.storage === 'local_fallback') {
              toast.info('Summary language saved on this device', {
                description: 'This meeting has no recording folder, so the preference cannot be written to meeting metadata.',
              });
            }
            if (request.language) {
              addRecent(request.language);
            }
            return;
          }

          if (latest?.version === request.version) return;
        } catch (err) {
          const latest = latestLanguageSaveRequestRef.current;
          if (
            latest?.version === request.version &&
            activeMeetingIdRef.current === request.meetingId
          ) {
            console.error('Failed to persist summary language:', err);
            toast.error('Failed to save summary language');
            setSummaryLang(request.rollback.language);
            setSummaryLangStorage(request.rollback.storage);
            return;
          }

          console.warn('Ignoring failed stale summary language save:', err);
          if (latest?.version === request.version) return;
        }
      }
    } finally {
      languageSaveLoopRunningRef.current = false;
    }
  };

  const handleLangChange = (code: string | null) => {
    const previous = summaryLang;
    const previousStorage = summaryLangStorage;
    const nextStored = code;
    languageLoadVersionRef.current += 1;
    latestLanguageSaveRequestRef.current = {
      version: languageSaveVersionRef.current + 1,
      meetingId: meeting.id,
      language: nextStored,
      rollback: {
        language: previous,
        storage: previousStorage,
      },
    };
    languageSaveVersionRef.current += 1;
    setSummaryLang(nextStored);
    setLangPickerOpen(false);
    void persistLatestLanguageSelection();
  };

  const isSummaryLoading = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';
  const progress = jobSnapshot?.progress;
  const jobState = jobSnapshot?.status;
  const jobNotice = jobState && ['skipped', 'failed', 'cancelled', 'unavailable'].includes(jobState)
    ? jobState === 'unavailable' ? 'Could not check summary progress. Trying again…' : jobSnapshot?.error
    : null;

  const collectSummaryMatches = (query: string): SummaryTextMatch[] => {
    const root = summarySearchRootRef.current;
    const normalizedQuery = query.trim().toLowerCase();
    if (!root || !normalizedQuery) return [];

    const matches: SummaryTextMatch[] = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode(node) {
        const text = node.textContent ?? '';
        return text.trim()
          ? NodeFilter.FILTER_ACCEPT
          : NodeFilter.FILTER_REJECT;
      },
    });

    let current = walker.nextNode();
    while (current) {
      const node = current as Text;
      const text = node.data.toLowerCase();
      let start = text.indexOf(normalizedQuery);
      while (start !== -1) {
        matches.push({
          node,
          start,
          end: start + normalizedQuery.length,
        });
        start = text.indexOf(normalizedQuery, start + normalizedQuery.length);
      }
      current = walker.nextNode();
    }

    return matches;
  };

  const selectSummaryMatch = (match: SummaryTextMatch) => {
    const range = document.createRange();
    range.setStart(match.node, match.start);
    range.setEnd(match.node, match.end);

    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);

    match.node.parentElement?.scrollIntoView({
      block: 'center',
      inline: 'nearest',
      behavior: 'smooth',
    });
  };

  const runSummaryFind = (direction: 1 | -1 = 1) => {
    const matches = collectSummaryMatches(findQuery);
    setFindMatchCount(matches.length);

    if (matches.length === 0) {
      setActiveFindIndex(-1);
      window.getSelection()?.removeAllRanges();
      return;
    }

    const nextIndex =
      activeFindIndex === -1 || activeFindIndex >= matches.length
        ? direction === 1
          ? 0
          : matches.length - 1
        : (activeFindIndex + direction + matches.length) % matches.length;

    setActiveFindIndex(nextIndex);
    selectSummaryMatch(matches[nextIndex]);
  };

  const openSummaryFind = () => {
    retainFindFocusRef.current = true;
    setIsFindOpen(true);
    window.requestAnimationFrame(() => {
      findInputRef.current?.focus();
      findInputRef.current?.select();
    });
  };

  const closeSummaryFind = () => {
    setIsFindOpen(false);
    setFindQuery('');
    setFindMatchCount(0);
    setActiveFindIndex(-1);
    window.getSelection()?.removeAllRanges();
  };

  const languageSlot = (
    <Popover open={langPickerOpen} onOpenChange={setLangPickerOpen}>
      <Tooltip><TooltipTrigger asChild><PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="h-8 px-2"
          title={`Summary language: ${effectiveLangLabel}${isLocalFallbackLanguage ? ' (saved on this device)' : ''}`}
          aria-label="Set summary language"
        >
          <Languages size={18} />
          {!compactToolbar && <span>{effectiveLangLabel}</span>}

        </Button>
      </PopoverTrigger></TooltipTrigger><TooltipContent>Summary language: {effectiveLangLabel}</TooltipContent></Tooltip>
      <PopoverContent
        align="end"
        className="w-auto p-0 border-0 shadow-none bg-transparent"
      >
        <LanguagePickerPopover
          value={summaryLang}
          onChange={handleLangChange}
          onClose={() => setLangPickerOpen(false)}
          autoSubtitle={autoSubtitle}
        />
      </PopoverContent>
    </Popover>
  );

  return (
    <div id="meeting-summary-panel" tabIndex={0} onKeyDown={event => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'h' && aiSummary && !isSummaryLoading) { event.preventDefault(); setReplaceOpen(true); }
    }} className="flex-1 min-h-[24rem] min-w-0 flex flex-col bg-card overflow-hidden focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
      <div ref={toolbarRef} className="meeting-summary-toolbar shrink-0 border-b border-border px-3 py-2">
        <div className="flex min-w-0 flex-nowrap items-center gap-2">
          <h2 className="shrink-0 text-sm font-semibold">Summary</h2>
          <div className="min-w-0 flex-1">
            {aiSummary && saveState !== 'discarded' && <span className="block truncate text-xs text-muted-foreground" role="status" title={saveState === 'error' ? 'Save failed — edits retained' : undefined}>
              {saveState === 'saving' ? 'Saving…' : saveState === 'saved' ? 'Saved' : saveState === 'error' ? 'Save failed — edits retained' : 'Unsaved changes'}
            </span>}
          </div>
          {aiSummary && <Button variant="ghost" size="sm" aria-label="Copy summary" title="Copy summary" onClick={() => void onCopySummary()}><Copy aria-hidden="true" />{!compactToolbar && 'Copy summary'}</Button>}
          <SummaryGeneratorButtonGroup compact={compactToolbar} generateRef={generateRef} onCheckingChange={setCheckingModels} secondaryGenerate={!aiSummary && !isSummaryLoading && transcripts.length > 0} modelConfig={modelConfig} setModelConfig={setModelConfig} onSaveModelConfig={onSaveModelConfig}
            onActionsCloseAutoFocus={event => {
              if (retainFindFocusRef.current) event.preventDefault();
              retainFindFocusRef.current = false;
            }}
            onGenerateSummary={onGenerateSummary} onStopGeneration={onStopGeneration} customPrompt={customPrompt} summaryStatus={summaryStatus}
            availableTemplates={availableTemplates} selectedTemplate={selectedTemplate} onTemplateSelect={onTemplateSelect}
            hasTranscripts={transcripts.length > 0} hasSummary={!!aiSummary} isModelConfigLoading={isModelConfigLoading} onOpenModelSettings={onOpenModelSettings} languageSlot={languageSlot}
            actionsSlot={<SummaryUpdaterButtonGroup isSaving={isSaving} isDirty={saveState !== 'saved' || isTitleDirty} onSave={onSaveAll}
              onFind={openSummaryFind} onOpenFolder={onOpenFolder} hasSummary={!!aiSummary} disabled={isSummaryLoading} onReplace={() => setReplaceOpen(true)}
              onRestore={hasPreviousSummary ? () => void onRestorePrevious() : undefined} />} />
        </div>
      </div>
      <TextReplaceDialog open={replaceOpen} onOpenChange={setReplaceOpen} title="summary"
        preview={async options => {
          if (!summaryRef.current) throw new Error('The summary is not ready.');
          return summaryRef.current.previewReplacement(options);
        }}
        apply={async (options, token) => {
          if (!summaryRef.current) throw new Error('The summary is not ready.');
          summaryRef.current.replaceText(options, token);
          toast.success('Summary text replaced', { description: 'Changes save automatically. Use Ctrl+Z in the editor to undo.' });
        }} />
      {aiSummary && details && (details.model || details.email || details.transcriptChanged || details.reduction?.passes || details.reduction?.limitRetries || details.localProvider) && <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-1 text-xs text-muted-foreground">
        {details.model && <span>Generated with {details.model}{details.template && ` · ${details.template}`}</span>}
        {details.localProvider && <span>{LOCAL_SUMMARY_HINT}</span>}
        {!!details.reduction?.passes && <span>{details.reduction.sourceExcerpts} excerpts condensed in {details.reduction.passes} {details.reduction.passes === 1 ? 'pass' : 'passes'}</span>}
        {!!details.reduction?.limitRetries && <span>Retried with a smaller model input</span>}
        {details.transcriptChanged && <span role="status" className="font-medium text-warning-foreground">Transcript changed since generation</span>}
        {details.email && <Button variant="ghost" size="sm" className="ml-auto text-xs" onClick={() => setEmailOpen(true)}>Follow-up email</Button>}
      </div>}
      {aiSummary && details?.translation && (
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border px-3 py-2 text-sm">
          <p role="status">
            English notes saved. {labelForCode(normaliseLanguageCode(details.translation.target) ?? details.translation.target)} translation failed.
            <span className="block text-xs text-muted-foreground">{details.translation.message}</span>
          </p>
          {!details.transcriptChanged && <Button variant="outline" size="sm" disabled={isSummaryLoading || contextLoading}
            onClick={() => void onRetryTranslation(details.translation!.target, customPrompt)}>Retry translation</Button>}
        </div>
      )}
      {emailOpen && details?.email && <FollowUpEmail key={meeting.id} meetingId={meeting.id} initialDraft={details.email} close={() => setEmailOpen(false)} />}
      {isFindOpen && aiSummary && !isSummaryLoading && (
        <div className="flex items-center gap-2 border-b border-border bg-muted/40 px-3 py-2">
          <Search className="h-4 w-4 text-muted-foreground" />
          <Input
            ref={findInputRef}
            value={findQuery}
            onChange={(event) => {
              setFindQuery(event.target.value);
              setFindMatchCount(0);
              setActiveFindIndex(-1);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                runSummaryFind(event.shiftKey ? -1 : 1);
              }
              if (event.key === 'Escape') {
                event.preventDefault();
                closeSummaryFind();
              }
            }}
            placeholder="Find in summary"
            className="shadow-none py-0 h-8 min-w-0 flex-1 rounded-md border border-input bg-background px-2 text-sm md:text-sm text-foreground outline-none focus:border-ring focus:ring-1 focus:ring-ring"
          />
          <span className="min-w-[4.5rem] text-right text-xs tabular-nums text-muted-foreground">
            {findQuery.trim()
              ? findMatchCount > 0
                ? `${activeFindIndex + 1}/${findMatchCount}`
                : 'No matches'
              : ''}
          </span>
          <ActionIcon label="Previous match" onClick={() => runSummaryFind(-1)} disabled={!findQuery.trim()}><ChevronUp className="h-4 w-4" /></ActionIcon>
          <ActionIcon label="Next match" onClick={() => runSummaryFind(1)} disabled={!findQuery.trim()}><ChevronDown className="h-4 w-4" /></ActionIcon>
          <ActionIcon label="Close find" onClick={closeSummaryFind}>
            <X className="h-4 w-4" />
          </ActionIcon>
        </div>
      )}

      <div className="flex min-h-0 flex-1 overflow-hidden">
        <div className="min-h-0 min-w-0 flex-1 overflow-y-auto">
          <SavedLiveAnswers key={meeting.id} meetingId={meeting.id} onReveal={onRevealSource} onPlay={onPlaySource}
            summaryBusy={isSummaryLoading || isSaving}
            onInsert={aiSummary ? async markdown => {
              if (isEditorSummary(aiSummary)) {
                if (!summaryRef.current) throw new Error('The summary is still loading. Try again.');
                await summaryRef.current.appendMarkdown(markdown);
              } else {
                const id = globalThis.crypto.randomUUID();
                onSummaryChange({ ...aiSummary, [id]: {
                  title: 'Saved live answer',
                  blocks: [{ id, type: 'text', content: markdown, color: 'default' }],
                } });
              }
            } : undefined} />
          {isSummaryLoading && (
            <p role="status" className="flex items-center gap-2 border-b border-border px-4 py-2 text-sm text-muted-foreground">
              {jobState === 'deferred' ? 'Automatic notes will resume after local inference is free.'
                : aiSummary ? 'Generating a new summary. The saved notes are available below.' : 'Generating summary…'}
            </p>
          )}
          {!aiSummary && !isSummaryLoading ? <div className="flex h-full min-h-48 flex-col items-center justify-center gap-2 p-8 text-center">
              <h3 className="text-base font-semibold">{transcripts.length ? 'No summary yet' : 'No transcript yet'}</h3>
              <p className="max-w-sm text-sm text-muted-foreground">{transcripts.length ? 'Notes use the template and AI model chosen in the toolbar.' : 'Transcribe the recording first, then generate a summary.'}</p>
              {transcripts.length > 0 && (
                <Button className="mt-2" disabled={checkingModels || isModelConfigLoading} onClick={() => { Analytics.trackButtonClick('generate_summary', 'meeting_details_empty'); generateRef.current?.(); }}>
                  <Sparkles className="h-4 w-4" />{jobState === 'failed' || jobState === 'cancelled' ? 'Retry summary' : 'Generate summary'}
                </Button>
              )}
            </div>
          : aiSummary && <div ref={summarySearchRootRef} className="mx-auto w-full max-w-[45rem] px-6 py-5">
              <SummarySources meetingId={meeting.id} revision={aiSummary} onReveal={onRevealSource} onPlay={onPlaySource}>
                <SummarySurface><BlockNoteSummaryView key={`${meeting.id}:${summaryRevision}`} saveState={saveState} onDraftChange={onSummaryDraft} ref={summaryRef}
                  summaryData={aiSummary} readOnly={isSummaryLoading} onSave={onSaveSummary} onSummaryChange={onSummaryChange} status={summaryStatus} error={summaryError}
                  meeting={{ id: meeting.id, title: meetingTitle, created_at: meeting.created_at }} /></SummarySurface>
              </SummarySources>
            </div>}
          {isSummaryLoading && progress && progress.total > 0 && (
            <p role="status" className="px-4 py-2 text-sm text-muted-foreground">
              Excerpts processed {progress.completed} of {progress.total} (pass {progress.level})
            </p>
          )}
          {(jobNotice || summaryStatus === 'error') && (
            <p role={jobState === 'failed' || summaryStatus === 'error' ? 'alert' : 'status'} className="mx-4 mb-3 text-sm text-muted-foreground">
              {jobNotice || summaryError || getSummaryStatusMessage(summaryStatus)}
            </p>
          )}
          {jobState === 'failed' && jobSnapshot?.partial_notes && (
            <section className="mx-4 mb-4 rounded-md border p-3">
              <h3 className="text-sm font-semibold">Recovered excerpt notes</h3>
              <p className="my-2 text-xs text-muted-foreground">These notes cover the excerpts that finished.</p>
              <p className="whitespace-pre-wrap break-words text-sm">{jobSnapshot.partial_notes}</p>
            </section>
          )}
        </div>
        <div ref={setDockTarget} role="complementary" aria-label="Meeting chat panel" className={chatDocked ? 'flex w-[25rem] shrink-0 flex-col border-l border-border' : 'hidden'} />
      </div>
      <footer className="shrink-0 border-t border-border px-3 py-2">
        <div className="flex items-center justify-between gap-2">
          <Button variant="ghost" size="sm" className="min-w-0 px-1 text-xs text-muted-foreground" aria-expanded={contextOpen} aria-controls={`summary-context-${meeting.id}`} disabled={contextLoading} onClick={() => setContextOpen(value => !value)}>
            {customPrompt?.trim() ? 'Context added, edit' : 'Add context for the summary'}
          </Button>
          <MeetingChat key={meeting.id} meetingId={meeting.id} provider={modelConfig?.provider} model={modelConfig?.model} dockTarget={dockTarget} onDockOpenChange={setChatDocked} />
        </div>
        {contextOpen && <label className="mt-2 block text-xs text-muted-foreground" id={`summary-context-${meeting.id}`}>Context for the summary
          <Textarea aria-label="Context for the summary" placeholder="People involved, meeting overview, objectives…" rows={3} className="mt-1 max-h-40 min-h-20 resize-y" disabled={contextLoading} aria-busy={contextLoading} value={customPrompt} onChange={event => onPromptChange(event.target.value)} />
        </label>}
      </footer>
    </div>
  );
}
