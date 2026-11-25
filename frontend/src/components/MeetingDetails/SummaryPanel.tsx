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
import { useCallback, useEffect, useRef, useState, RefObject } from 'react';
import { toast } from 'sonner';
import { Languages, ChevronDown, ChevronUp, Loader2, Search, Sparkles, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Progress } from '@/components/ui/progress';
import { Copy } from 'lucide-react';
import { FollowUpEmail } from './FollowUpEmail';
import { SavedLiveAnswers } from './SavedLiveAnswers';
import { isEditorSummary, LOCAL_SUMMARY_HINT, type summaryDetails } from '@/lib/meeting-summary';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { LanguagePickerPopover } from '@/components/LanguagePickerPopover';
import { useRecentLanguages } from '@/hooks/useRecentLanguages';
import { DEFAULT_PLAYER_BINDINGS, playerActionForChord, playerChord, type PlayerBindings } from '@/lib/player-bindings';
import { isTextEntry } from '@/lib/playback-controls';
import { MAX_FIND_QUERY_LENGTH, stepMatch } from '@/lib/transcript-find';
import { collectSummaryRanges } from '@/lib/summary-find';
import { recoveredSummary } from '@/lib/recovered-summary';
import { labelForCode, normaliseLanguageCode } from '@/lib/summary-languages';
import {
  readMeetingSummaryLanguage,
  saveMeetingSummaryLanguage,
  SummaryLanguageStorage,
} from '@/lib/summary-language-preferences';

import type { SummaryDraft, SaveState } from '@/lib/meetingDrafts';

interface SummaryPanelProps {
  bindings?: PlayerBindings;
  findOpenRequest?: number;
  seriesLanguage?: string | null;
  details: ReturnType<typeof summaryDetails> & { transcriptChanged: boolean };
  saveState: SaveState;
  hasPreviousSummary: boolean;
  summaryRevision: number;
  onRestorePrevious: () => Promise<void>;
  onSummaryDraft: (draft: SummaryDraft, read: () => Promise<SummaryDraft>) => void;
  onRevealSource?: (source: ResolvedSummarySource, current: () => boolean) => Promise<void>;
  onPlaySource?: (seconds: number, current: () => boolean) => Promise<void>;
  onNavigateSource?: () => void;
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
  aiSummary: MeetingSummary | null;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  checkingModels?: boolean;
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

const SUMMARY_FIND_HIGHLIGHT = 'clawscribe-summary-find';
const SUMMARY_CURRENT_FIND_HIGHLIGHT = 'clawscribe-summary-find-current';

export function SummaryPanel({
  bindings = DEFAULT_PLAYER_BINDINGS,
  findOpenRequest = 0,
  details,
  seriesLanguage,
  saveState, hasPreviousSummary, summaryRevision, onRestorePrevious, onSummaryDraft,
  onRevealSource,
  onPlaySource,
  onNavigateSource,
  meeting,
  meetingTitle,
  isTitleDirty,
  summaryRef,
  isSaving,
  onSaveAll,
  onCopySummary,
  aiSummary,
  summaryStatus,
  checkingModels = false,
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
  const [contextOpen, setContextOpen] = useState(false);
  const [dockTarget, setDockTarget] = useState<HTMLDivElement | null>(null);
  const [chatDocked, setChatDocked] = useState(false);
  const [summaryLang, setSummaryLang] = useState<string | null>(null);
  const [summaryLangStorage, setSummaryLangStorage] = useState<SummaryLanguageStorage>('metadata');
  const [langPickerOpen, setLangPickerOpen] = useState(false);
  const [replaceOpen, setReplaceOpen] = useState(false);
  const [emailOpen, setEmailOpen] = useState(false);
  const [isFindOpen, setIsFindOpen] = useState(false);
  const [findFocus, setFindFocus] = useState(0);
  const [findQuery, setFindQuery] = useState('');
  const [findMatches, setFindMatches] = useState<Range[]>([]);
  const [activeFindIndex, setActiveFindIndex] = useState(0);
  const [recoveryCandidate, setRecoveryCandidate] = useState<string | null>(null);
  const [appliedRecovery, setAppliedRecovery] = useState<string | null>(null);
  const [savingRecovery, setSavingRecovery] = useState(false);
  const [recoveryError, setRecoveryError] = useState('');
  const recoveryPending = useRef(false);
  const recoveryButton = useRef<HTMLButtonElement>(null);
  const languageLoadVersionRef = useRef(0);
  const activeMeetingIdRef = useRef(meeting.id);
  const languageSaveVersionRef = useRef(0);
  const languageSaveLoopRunningRef = useRef(false);
  const findInputRef = useRef<HTMLInputElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const retainFindFocusRef = useRef(false);
  const summarySearchRootRef = useRef<HTMLDivElement>(null);
  const summaryScrollRef = useRef<HTMLDivElement>(null);
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
  const generationDisabled = contextLoading || isModelConfigLoading || checkingModels || savingRecovery;
  const recoveredNotes = jobState === 'failed' ? jobSnapshot?.partial_notes : null;
  const canUseRecovery = !isSummaryLoading && !checkingModels && !isSaving && Boolean(recoveredNotes);

  const useRecoveredNotes = async () => {
    if (!canUseRecovery || !recoveryCandidate || recoveryCandidate !== recoveredNotes || recoveryPending.current) return;
    recoveryPending.current = true;
    setSavingRecovery(true);
    setRecoveryError('');
    try {
      await onSaveSummary(recoveredSummary(recoveryCandidate));
      setAppliedRecovery(recoveryCandidate);
      setRecoveryCandidate(null);
      toast.success('Recovered notes saved as summary');
    } catch {
      setRecoveryError('Could not save recovered notes. Try again.');
    } finally {
      recoveryPending.current = false;
      setSavingRecovery(false);
    }
  };

  const copyRecoveredNotes = async () => {
    if (!recoveredNotes) return;
    try {
      await navigator.clipboard.writeText(recoveredNotes);
      toast.success('Recovered notes copied');
    } catch {
      toast.error('Could not copy recovered notes');
    }
  };

  const hasSummary = Boolean(aiSummary);
  const openSummaryFind = useCallback(() => {
    if (!hasSummary) return;
    setIsFindOpen(true);
    setFindFocus(value => value + 1);
  }, [hasSummary]);
  useEffect(() => {
    if (findOpenRequest > 0) openSummaryFind();
  }, [findOpenRequest, openSummaryFind]);
  useEffect(() => {
    if (!isFindOpen) return;
    findInputRef.current?.focus();
    findInputRef.current?.select();
  }, [isFindOpen, findFocus]);

  useEffect(() => {
    const root = summarySearchRootRef.current;
    if (!root || !isFindOpen || !findQuery.trim()) {
      setFindMatches([]);
      return;
    }
    const refresh = () => {
      const matches = collectSummaryRanges(root, findQuery);
      setFindMatches(matches);
      setActiveFindIndex(index => index < matches.length ? index : 0);
    };
    refresh();
    // The editor loads asynchronously and can change under an open search.
    const observer = new window.MutationObserver(refresh);
    observer.observe(root, { childList: true, characterData: true, subtree: true });
    return () => observer.disconnect();
  }, [isFindOpen, findQuery, hasSummary]);

  useEffect(() => {
    if (!isFindOpen || findMatches.length === 0) return;
    const highlights = window.CSS?.highlights;
    if (!window.Highlight || !highlights) return;
    highlights.set(SUMMARY_FIND_HIGHLIGHT, new window.Highlight(...findMatches));
    return () => { highlights.delete(SUMMARY_FIND_HIGHLIGHT); };
  }, [isFindOpen, findMatches]);

  const activeFindMatch = isFindOpen ? findMatches[activeFindIndex] : undefined;
  useEffect(() => {
    const scroll = summaryScrollRef.current;
    if (!activeFindMatch || !scroll) return;
    const match = activeFindMatch.getBoundingClientRect();
    const viewport = scroll.getBoundingClientRect();
    if (match.top < viewport.top || match.bottom > viewport.bottom) {
      scroll.scrollTop += match.top - viewport.top - (viewport.height - match.height) / 2;
    }
    const highlights = window.CSS?.highlights;
    if (!window.Highlight || !highlights) return;
    const highlight = new window.Highlight(activeFindMatch);
    highlight.priority = 1;
    highlights.set(SUMMARY_CURRENT_FIND_HIGHLIGHT, highlight);
    return () => { highlights.delete(SUMMARY_CURRENT_FIND_HIGHLIGHT); };
  }, [activeFindMatch]);

  const runSummaryFind = (direction: 1 | -1) => {
    if (findMatches.length > 0) setActiveFindIndex(index => stepMatch(index, findMatches.length, direction));
  };

  const closeSummaryFind = () => {
    setIsFindOpen(false);
    setFindQuery('');
    setFindMatches([]);
    setActiveFindIndex(0);
    panelRef.current?.focus();
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
    <div ref={panelRef} id="meeting-summary-panel" tabIndex={0} onKeyDownCapture={event => {
      // The player shortcuts stand down while typing. Find still works inside
      // the summary editor and its find input, but leaves portaled menus alone.
      const target = event.target as HTMLElement;
      if (event.defaultPrevented || !hasSummary || !event.currentTarget.contains(target) || !isTextEntry(target) || !(event.ctrlKey || event.metaKey)) return;
      const chord = playerChord(event.nativeEvent);
      if (chord && playerActionForChord(bindings, chord)?.key === 'find') {
        event.preventDefault();
        event.stopPropagation();
        openSummaryFind();
      }
    }} onKeyDown={event => {
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
          <SummaryGeneratorButtonGroup compact={compactToolbar} modelConfig={modelConfig} setModelConfig={setModelConfig} onSaveModelConfig={onSaveModelConfig}
            onActionsCloseAutoFocus={event => {
              if (retainFindFocusRef.current) {
                event.preventDefault();
                findInputRef.current?.focus();
                findInputRef.current?.select();
              }
              retainFindFocusRef.current = false;
            }}
            onGenerateSummary={onGenerateSummary} onStopGeneration={onStopGeneration} customPrompt={customPrompt} summaryStatus={summaryStatus}
            availableTemplates={availableTemplates} selectedTemplate={selectedTemplate} onTemplateSelect={onTemplateSelect}
            showRegenerate={hasSummary && !details.transcriptChanged && transcripts.length > 0}
            isCheckingModels={checkingModels} generationDisabled={generationDisabled}
            onOpenModelSettings={onOpenModelSettings} languageSlot={languageSlot}
            actionsSlot={<SummaryUpdaterButtonGroup isSaving={isSaving} isDirty={saveState !== 'saved' || isTitleDirty} onSave={onSaveAll}
              onFind={() => { retainFindFocusRef.current = true; openSummaryFind(); }} hasSummary={hasSummary} disabled={isSummaryLoading} onReplace={() => setReplaceOpen(true)}
              onRestore={hasPreviousSummary ? () => void onRestorePrevious() : undefined} />} />
        </div>
      </div>
      {isSummaryLoading && <div className="shrink-0 space-y-2 border-b border-border bg-muted/40 px-3 py-2 text-sm">
        <p role="status" className="flex items-center gap-2">
          <Loader2 aria-hidden="true" className="h-4 w-4 shrink-0 animate-spin text-primary motion-reduce:animate-none" />
          {jobState === 'deferred' ? 'Automatic notes will resume when local inference is available.'
            : hasSummary ? 'Regenerating summary…' : 'Generating summary…'}
        </p>
        {progress && progress.total > 0 && <>
          <Progress aria-label="Summary excerpt progress" value={Math.min(100, 100 * progress.completed / progress.total)}
            aria-valuetext={`${progress.completed} of ${progress.total} excerpts, pass ${progress.level}`} />
          <p className="text-xs text-muted-foreground">{progress.completed} of {progress.total} excerpts processed · Pass {progress.level}</p>
        </>}
      </div>}
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
      {aiSummary && details && (details.model || details.email || details.reduction?.passes || details.reduction?.limitRetries || details.localProvider) && <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-1 text-xs text-muted-foreground">
        {details.model && <span>Generated with {details.model}{details.template && ` · ${details.template}`}</span>}
        {details.localProvider && <span>{LOCAL_SUMMARY_HINT}</span>}
        {!!details.reduction?.passes && <span>{details.reduction.sourceExcerpts} excerpts condensed in {details.reduction.passes} {details.reduction.passes === 1 ? 'pass' : 'passes'}</span>}
        {!!details.reduction?.limitRetries && <span>Retried with a smaller model input</span>}
        {details.email && <Button variant="ghost" size="sm" className="ml-auto text-xs" onClick={() => setEmailOpen(true)}>Follow-up email</Button>}
      </div>}
      {aiSummary && details.transcriptChanged && <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border px-3 py-2">
        <p role="status" className="text-xs font-medium text-warning-foreground">Transcript changed since generation</p>
        {!isSummaryLoading && transcripts.length > 0 && <Button size="sm" disabled={generationDisabled}
          onClick={() => { Analytics.trackButtonClick('generate_summary', 'meeting_details_transcript_changed'); void onGenerateSummary(customPrompt); }}>
          {checkingModels ? 'Checking models…' : 'Regenerate'}
        </Button>}
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
      {isFindOpen && hasSummary && (
        <div role="search" aria-label="Find in summary" className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/40 px-3 py-2">
          <Search aria-hidden="true" className="h-4 w-4 text-muted-foreground" />
          <Input
            ref={findInputRef}
            value={findQuery}
            onChange={(event) => {
              setFindQuery(event.target.value);
              setActiveFindIndex(0);
            }}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === 'Enter') {
                event.preventDefault();
                runSummaryFind(event.shiftKey ? -1 : 1);
              }
              if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                closeSummaryFind();
              }
            }}
            placeholder="Find in summary"
            aria-label="Find in summary"
            autoComplete="off"
            maxLength={MAX_FIND_QUERY_LENGTH}
            className="shadow-none py-0 h-8 min-w-0 flex-1 rounded-md border border-input bg-background px-2 text-sm md:text-sm text-foreground outline-none focus:border-ring focus:ring-1 focus:ring-ring"
          />
          <span role="status" className="min-w-[4.5rem] text-right text-xs tabular-nums text-muted-foreground">
            {findQuery.trim()
              ? findMatches.length > 0
                ? `${activeFindIndex + 1} of ${findMatches.length}`
                : 'No matches'
              : ''}
          </span>
          <ActionIcon label="Previous match" onClick={() => runSummaryFind(-1)} disabled={!findMatches.length}><ChevronUp className="h-4 w-4" /></ActionIcon>
          <ActionIcon label="Next match" onClick={() => runSummaryFind(1)} disabled={!findMatches.length}><ChevronDown className="h-4 w-4" /></ActionIcon>
          <ActionIcon label="Close find" onClick={closeSummaryFind}>
            <X className="h-4 w-4" />
          </ActionIcon>
        </div>
      )}

      <div className="flex min-h-0 flex-1 overflow-hidden">
        <div ref={summaryScrollRef} className="min-h-0 min-w-0 flex-1 overflow-y-auto">
          <SavedLiveAnswers key={meeting.id} meetingId={meeting.id} onReveal={onRevealSource && (source => onRevealSource(source, () => true))} onPlay={onPlaySource && (seconds => onPlaySource(seconds, () => true))}
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
          {!aiSummary && !isSummaryLoading ? <div className="flex h-full min-h-48 flex-col items-center justify-center gap-2 p-8 text-center">
              <h3 className="text-base font-semibold">{transcripts.length ? 'No summary yet' : 'No transcript yet'}</h3>
              <p className="max-w-sm text-sm text-muted-foreground">{transcripts.length ? 'Notes use the template and AI model chosen in the toolbar.' : 'Transcribe the recording first, then generate a summary.'}</p>
              {transcripts.length > 0 && (
                <Button className="mt-2" disabled={generationDisabled} onClick={() => { Analytics.trackButtonClick('generate_summary', 'meeting_details_empty'); void onGenerateSummary(customPrompt); }}>
                  {checkingModels ? <Loader2 className="h-4 w-4 animate-spin motion-reduce:animate-none" /> : <Sparkles className="h-4 w-4" />}
                  {checkingModels ? 'Checking models…' : jobState === 'failed' || jobState === 'cancelled' ? 'Retry summary' : 'Generate summary'}
                </Button>
              )}
            </div>
          : aiSummary && <div className="mx-auto w-full max-w-[45rem] px-6 py-5">
              <SummarySources meetingId={meeting.id} revision={aiSummary} onReveal={onRevealSource} onPlay={onPlaySource} onNavigate={onNavigateSource}>
                <div ref={summarySearchRootRef} className="[&_*::highlight(clawscribe-summary-find)]:bg-warning [&_*::highlight(clawscribe-summary-find)]:text-foreground [&_*::highlight(clawscribe-summary-find-current)]:bg-primary [&_*::highlight(clawscribe-summary-find-current)]:text-primary-foreground">
                <SummarySurface><BlockNoteSummaryView key={`${meeting.id}:${summaryRevision}`} saveState={saveState} onDraftChange={onSummaryDraft} ref={summaryRef}
                  summaryData={aiSummary} readOnly={isSummaryLoading || savingRecovery} onSave={onSaveSummary} onSummaryChange={onSummaryChange} status={summaryStatus} error={summaryError}
                  meeting={{ id: meeting.id, title: meetingTitle, created_at: meeting.created_at }} /></SummarySurface>
                </div>
              </SummarySources>
            </div>}
          {(jobNotice || summaryStatus === 'error') && (
            <p role={jobState === 'failed' || summaryStatus === 'error' ? 'alert' : 'status'} className="mx-4 mb-3 text-sm text-muted-foreground">
              {jobNotice || summaryError || getSummaryStatusMessage(summaryStatus)}
            </p>
          )}
          {recoveredNotes && (
            <section className="mx-4 mb-4 rounded-md border border-border p-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h3 className="text-sm font-semibold">Recovered excerpt notes</h3>
                <div className="flex flex-wrap gap-1">
                  <Button variant="ghost" size="sm" onClick={() => void copyRecoveredNotes()}>Copy</Button>
                  {appliedRecovery !== recoveredNotes && <Button ref={recoveryButton} variant="outline" size="sm" disabled={!canUseRecovery || savingRecovery}
                    onClick={() => { setRecoveryError(''); setRecoveryCandidate(recoveredNotes); }}>Use as summary</Button>}
                </div>
              </div>
              <p className="my-2 text-xs text-muted-foreground">These notes cover the excerpts that finished.</p>
              <p className="whitespace-pre-wrap break-words text-sm">{recoveredNotes}</p>
            </section>
          )}
        </div>
        <div ref={setDockTarget} role="complementary" aria-label="Meeting chat panel" className={chatDocked ? 'flex w-[25rem] shrink-0 flex-col border-l border-border' : 'hidden'} />
      </div>
      <Dialog open={recoveryCandidate !== null} onOpenChange={open => { if (!open && !savingRecovery) setRecoveryCandidate(null); }}>
        <DialogContent onCloseAutoFocus={event => {
          event.preventDefault();
          (recoveryButton.current ?? panelRef.current)?.focus();
        }}>
          <DialogHeader>
            <DialogTitle>Use recovered notes as summary?</DialogTitle>
            <DialogDescription>{hasSummary ? 'This replaces the current summary with notes from the excerpts that finished.' : 'This saves notes from the excerpts that finished as the meeting summary.'}</DialogDescription>
          </DialogHeader>
          {recoveryError && <p role="alert" className="text-sm text-destructive">{recoveryError}</p>}
          <DialogFooter>
            <Button type="button" variant="outline" disabled={savingRecovery} onClick={() => setRecoveryCandidate(null)}>Cancel</Button>
            <Button type="button" disabled={!canUseRecovery || savingRecovery || recoveryCandidate !== recoveredNotes}
              onClick={() => void useRecoveredNotes()}>{savingRecovery ? 'Saving…' : 'Use as summary'}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
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
