import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DEFAULT_PARAKEET_MODEL } from '@/constants/modelDefaults';
import { Progress } from '@/components/ui/progress';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { OnboardingContainer } from '../OnboardingContainer';
import { getSummaryModelSizeLabel } from '@/lib/onboarding-summary-model';
import type { RecordingMode } from '@/contexts/RecordingStateContext';

type SetupState = 'idle' | 'preparing' | 'stopping' | 'ready' | 'failed';
const FINISH_CANCEL_TIMEOUT_MS = 10_000;

function createPreparation() {
  let settle!: () => void;
  const done = new Promise<void>(resolve => { settle = resolve; });
  return { cancelled: false, done, settle };
}
type Preparation = ReturnType<typeof createPreparation>;
type CancelIntent = 'finish' | 'speech' | 'notes';

type DownloadEvent = {
  modelName?: string;
  model?: string;
  progress: number;
  total_mb?: number;
  status: string;
};

function useSetupDownload(kind: 'speech' | 'notes') {
  const [percent, setPercent] = useState<number | null>(null);
  const mounted = useRef(false);
  const stopListening = useRef<UnlistenFn | null>(null);
  const downloading = useRef<string | null>(null);
  const cancelCommand = kind === 'speech' ? 'parakeet_cancel_download' : 'builtin_ai_cancel_download';
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (downloading.current) void invoke(cancelCommand, { modelName: downloading.current }).catch(() => {});
      stopListening.current?.();
      stopListening.current = null;
    };
  }, [cancelCommand]);

  const cancel = () => {
    if (downloading.current) void invoke(cancelCommand, { modelName: downloading.current }).catch(() => {});
  };
  const download = async (modelName: string, preparation: Preparation) => {
    if (!mounted.current || preparation.cancelled) return;
    setPercent(null);
    const speech = kind === 'speech';
    const event = speech ? 'parakeet-model-download-progress' : 'builtin-ai-download-progress';
    let observing = true;
    let cancelAfterReservation = false;
    const unlisten = await listen<DownloadEvent>(event, ({ payload }) => {
      if (!observing || !mounted.current || (speech ? payload.modelName : payload.model) !== modelName) return;
      if (preparation.cancelled) {
        // The first progress event confirms the native transfer reserved its slot.
        if (!cancelAfterReservation && payload.status === 'downloading') {
          cancelAfterReservation = true;
          cancel();
        }
        return;
      }
      if (payload.status !== 'downloading' && payload.status !== 'completed') return;
      const measured = Number.isFinite(payload.progress) && payload.progress >= 0 && payload.progress <= 100;
      setPercent(measured && (payload.total_mb ?? 0) > 0 ? Math.round(payload.progress) : null);
    }).catch(() => null);
    if (!mounted.current || preparation.cancelled) {
      unlisten?.();
      return;
    }
    stopListening.current = unlisten;
    downloading.current = modelName;
    try {
      await invoke(speech ? 'parakeet_download_model' : 'builtin_ai_download_model', { modelName });
    } finally {
      observing = false;
      downloading.current = null;
      stopListening.current?.();
      stopListening.current = null;
      if (mounted.current) setPercent(null);
    }
  };
  return { percent, download, cancel };
}

function DownloadStatus({ percent, label }: { percent: number | null; label: string }) {
  return percent === null
    ? <p role="status" className="text-sm text-muted-foreground">{label}</p>
    : <div className="space-y-1 text-sm text-muted-foreground">
        <p>Downloading · {percent}%</p>
        <Progress value={percent} aria-label={label} aria-valuenow={percent} />
      </div>;
}

interface SetupOverviewStepProps {
  onChoose: (mode: RecordingMode, localNotesModel: string | null, automaticNotes?: boolean) => void;
  finishing: boolean;
  error: string | null;
}

export function SetupOverviewStep({ onChoose, finishing, error }: SetupOverviewStepProps) {
  const [speech, setSpeech] = useState<SetupState>('idle');
  const [notes, setNotes] = useState<SetupState>('idle');
  const [notesModel, setNotesModel] = useState<string | null>(null);
  const [recommendedNotes, setRecommendedNotes] = useState<string | null>(null);
  const [useLive, setUseLive] = useState(true);
  const [cancelIntent, setCancelIntent] = useState<CancelIntent | null>(null);
  const [finishingAfterCancel, setFinishingAfterCancel] = useState(false);
  const [notesOptIn, setNotesOptIn] = useState(false);
  const [automaticNotes, setAutomaticNotes] = useState(false);
  const speechDownload = useSetupDownload('speech');
  const notesDownload = useSetupDownload('notes');
  const speechPending = useRef<Preparation | null>(null);
  const notesPending = useRef<Preparation | null>(null);

  useEffect(() => {
    let mounted = true;
    void invoke<string>('builtin_ai_get_recommended_model')
      .then(model => { if (mounted) setRecommendedNotes(model); })
      .catch(() => {});
    return () => { mounted = false; };
  }, []);

  const prepareSpeech = async () => {
    if (speechPending.current) return;
    const preparation = createPreparation();
    speechPending.current = preparation;
    setSpeech('preparing');
    try {
      await invoke('parakeet_init');
      if (preparation.cancelled) return;
      if (!await invoke<boolean>('parakeet_has_available_models')) {
        await speechDownload.download(DEFAULT_PARAKEET_MODEL, preparation);
        if (preparation.cancelled) return;
        const models = await invoke<{ name: string; status: unknown }[]>('parakeet_get_available_models');
        if (!models.some(model => model.name === DEFAULT_PARAKEET_MODEL && model.status === 'Available')) {
          throw new Error('Model unavailable');
        }
      }
      if (!preparation.cancelled) setSpeech('ready');
    } catch {
      if (!preparation.cancelled) setSpeech('failed');
    } finally {
      if (preparation.cancelled) setSpeech('idle');
      speechPending.current = null;
      preparation.settle();
    }
  };

  const prepareNotes = async () => {
    if (notesPending.current) return;
    const preparation = createPreparation();
    notesPending.current = preparation;
    setAutomaticNotes(false);
    setNotesModel(null);
    setNotes('preparing');
    setNotesOptIn(true);
    try {
      const model = recommendedNotes ?? await invoke<string>('builtin_ai_get_recommended_model');
      if (preparation.cancelled) return;
      setNotesModel(model);
      const args = { modelName: model, refresh: true };
      if (!await invoke<boolean>('builtin_ai_is_model_ready', args)) {
        await notesDownload.download(model, preparation);
        if (preparation.cancelled) return;
        if (!await invoke<boolean>('builtin_ai_is_model_ready', args)) throw new Error('Model unavailable');
      }
      if (!preparation.cancelled) setNotes('ready');
    } catch {
      if (!preparation.cancelled) setNotes('failed');
    } finally {
      if (preparation.cancelled) {
        setNotes('idle');
        setNotesOptIn(false);
      }
      notesPending.current = null;
      preparation.settle();
    }
  };

  const cancelSpeech = () => {
    if (!speechPending.current) return;
    speechPending.current.cancelled = true;
    setSpeech('stopping');
    speechDownload.cancel();
  };
  const cancelNotes = () => {
    if (!notesPending.current) return;
    notesPending.current.cancelled = true;
    setNotes('stopping');
    notesDownload.cancel();
  };
  const complete = () => {
    const mode = speech === 'ready' && useLive ? 'live' : 'audio_only';
    const model = notesOptIn && notes === 'ready' ? notesModel : null;
    if (model && automaticNotes) onChoose(mode, model, true);
    else onChoose(mode, model);
  };

  const confirmStop = async () => {
    const intent = cancelIntent;
    if (!intent) return;
    setCancelIntent(null);
    if (intent === 'speech') return cancelSpeech();
    if (intent === 'notes') return cancelNotes();
    const waiting = [speechPending.current?.done, notesPending.current?.done];
    setFinishingAfterCancel(true);
    cancelSpeech();
    cancelNotes();
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const timeout = new Promise<void>(resolve => {
        timer = setTimeout(resolve, FINISH_CANCEL_TIMEOUT_MS);
      });
      await Promise.race([Promise.all(waiting), timeout]);
      complete();
    } finally {
      clearTimeout(timer);
      setFinishingAfterCancel(false);
    }
  };

  const busyFinishing = finishing || finishingAfterCancel;
  const requestFinish = () => {
    if (speechPending.current || notesPending.current) setCancelIntent('finish');
    else complete();
  };
  const notesSize = recommendedNotes && getSummaryModelSizeLabel(recommendedNotes);

  return (
    <OnboardingContainer title="Choose how to record" step={2}>
      <div className="mx-auto max-w-lg space-y-3">
        <section className="space-y-3 rounded-lg border border-border bg-card p-5">
          <h2 className="font-medium">Live transcription</h2>
          <p className="text-sm text-muted-foreground">
            Show speech as text while recording. The Parakeet download is about 670 MB.
          </p>
          {speech === 'preparing' && <DownloadStatus percent={speechDownload.percent} label="Preparing the speech model…" />}
          {speech === 'stopping' && <p role="status" className="text-sm text-muted-foreground">Stopping speech model setup…</p>}
          {speech === 'failed' && <p role="alert" className="text-sm text-destructive">Could not prepare the speech model. Try again or finish with audio only.</p>}
          {speech === 'ready'
            ? <div className="flex items-center gap-2 text-sm">
                <Switch id="onboarding-live-transcription" checked={useLive} disabled={busyFinishing} onCheckedChange={setUseLive} />
                <label htmlFor="onboarding-live-transcription">Use live transcription</label>
              </div>
            : speech === 'preparing'
              ? <Button variant="outline" onClick={() => setCancelIntent('speech')}>Cancel speech setup</Button>
              : <Button variant="outline" onClick={() => void prepareSpeech()} disabled={busyFinishing || speech === 'stopping'}>
                  {speech === 'failed' ? 'Retry speech model setup' : 'Set up live transcription'}
                </Button>}
        </section>

        <section className="space-y-3 rounded-lg border border-border bg-card p-5">
          <h2 className="font-medium">Local meeting notes</h2>
          <p className="text-sm text-muted-foreground">
            Generate notes from saved transcripts on this device. {notesSize
              ? `Recommended download: ${notesSize}.`
              : 'The model download can be several gigabytes.'}
          </p>
          {notes === 'preparing' && <DownloadStatus percent={notesDownload.percent}
            label={notesModel ? `Preparing the notes model (${getSummaryModelSizeLabel(notesModel) || 'download size varies'})…` : 'Finding a notes model…'} />}
          {notes === 'failed' && <p role="alert" className="text-sm text-destructive">Could not prepare the notes model. Try again or continue with recording.</p>}
          {notes === 'stopping' && <p role="status" className="text-sm text-muted-foreground">Stopping notes model setup…</p>}
          {notesOptIn && notes === 'ready' ? (
            <>
              <p role="status" className="text-sm text-muted-foreground">The notes model is ready. You can generate notes from a saved meeting.</p>
              <div className="flex items-center gap-2 text-sm">
                <Switch id="onboarding-automatic-notes" checked={automaticNotes} disabled={busyFinishing} onCheckedChange={setAutomaticNotes} />
                <label htmlFor="onboarding-automatic-notes">Generate notes automatically after transcription</label>
              </div>
              <p className="text-xs text-muted-foreground">You can turn automatic notes off in Summary settings.</p>
              <Button variant="ghost" onClick={() => { setAutomaticNotes(false); setNotesOptIn(false); }} disabled={busyFinishing}>Skip local notes</Button>
            </>
          ) : notes === 'preparing' ? (
            <Button variant="outline" onClick={() => setCancelIntent('notes')}>Cancel notes setup</Button>
          ) : (
            <Button variant="outline" onClick={() => void prepareNotes()} disabled={busyFinishing || notes === 'stopping'}>
              {notes === 'failed' ? 'Retry local notes setup' : 'Download local notes model'}
            </Button>
          )}
        </section>
        <div className="space-y-2 pb-2">
          {!(speech === 'ready' && useLive) && <p className="text-sm text-muted-foreground">
            Audio-only recording is ready. You can transcribe a saved meeting later.
          </p>}
          <Button onClick={requestFinish} disabled={busyFinishing}>
            {busyFinishing ? 'Finishing setup…' : 'Finish setup'}
          </Button>
          {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        </div>
      </div>
      <Dialog open={cancelIntent !== null} onOpenChange={open => { if (!open) setCancelIntent(null); }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Cancel model setup?</DialogTitle>
            <DialogDescription>{cancelIntent === 'finish'
              ? 'Setup will cancel active downloads and use any models that are already ready.'
              : 'The model download will stop. You can start it again later.'}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCancelIntent(null)}>Keep downloading</Button>
            <Button onClick={() => void confirmStop()}>{cancelIntent === 'finish' ? 'Cancel and finish' : 'Cancel download'}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </OnboardingContainer>
  );
}
