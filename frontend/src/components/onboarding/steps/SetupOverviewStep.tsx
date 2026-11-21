import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DEFAULT_PARAKEET_MODEL } from '@/constants/modelDefaults';
import { Progress } from '@/components/ui/progress';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { OnboardingContainer } from '../OnboardingContainer';
import { getSummaryModelSizeLabel } from '@/lib/onboarding-summary-model';
import type { RecordingMode } from '@/contexts/RecordingStateContext';

type SetupState = 'idle' | 'preparing' | 'stopping' | 'ready' | 'failed';
type Preparation = { cancelled: boolean };

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
  const [notesOptIn, setNotesOptIn] = useState(false);
  const [automaticNotes, setAutomaticNotes] = useState(false);
  const speechDownload = useSetupDownload('speech');
  const notesDownload = useSetupDownload('notes');
  const speechPending = useRef<Preparation | null>(null);
  const notesPending = useRef<Preparation | null>(null);

  const prepareSpeech = async () => {
    if (speechPending.current) return;
    const preparation = { cancelled: false };
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
    }
  };

  const prepareNotes = async () => {
    if (notesPending.current) return;
    const preparation = { cancelled: false };
    notesPending.current = preparation;
    setAutomaticNotes(false);
    setNotesModel(null);
    setNotes('preparing');
    setNotesOptIn(true);
    try {
      const model = await invoke<string>('builtin_ai_get_recommended_model');
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
  const choose = (mode: RecordingMode) => {
    cancelSpeech();
    cancelNotes();
    const model = notesOptIn && notes === 'ready' ? notesModel : null;
    if (model && automaticNotes) onChoose(mode, model, true);
    else onChoose(mode, model);
  };

  return (
    <OnboardingContainer title="Choose how to record" hideProgress>
      <div className="mx-auto max-w-lg space-y-3">
        <section className="space-y-3 rounded-lg border border-border bg-card p-5">
          <p className="text-sm text-muted-foreground">
            Save audio on this device and transcribe it from the meeting later. No model download is required.
          </p>
          <Button onClick={() => choose('audio_only')} disabled={finishing}>
            {finishing ? 'Finishing setup…' : 'Record audio now'}
          </Button>
          {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        </section>

        <section className="space-y-3 rounded-lg border border-border bg-card p-5">
          <h2 className="font-medium">Live transcription</h2>
          <p className="text-sm text-muted-foreground">
            Show speech as text while recording. Setting up Parakeet can download about 670 MB.
          </p>
          {speech === 'preparing' && <DownloadStatus percent={speechDownload.percent} label="Preparing the speech model…" />}
          {speech === 'stopping' && <p role="status" className="text-sm text-muted-foreground">Stopping speech model setup…</p>}
          {speech === 'failed' && <p role="alert" className="text-sm text-destructive">Could not prepare the speech model. Try again or record audio now.</p>}
          {speech === 'ready'
            ? <Button variant="outline" onClick={() => choose('live')} disabled={finishing}>Continue with live transcription</Button>
            : speech === 'preparing'
              ? <Button variant="outline" onClick={cancelSpeech}>Cancel speech setup</Button>
              : <Button variant="outline" onClick={() => void prepareSpeech()} disabled={finishing || speech === 'stopping'}>
                  {speech === 'failed' ? 'Retry speech model setup' : 'Set up live transcription'}
                </Button>}
        </section>

        <section className="space-y-3 rounded-lg border border-border bg-card p-5">
          <h2 className="font-medium">Local meeting notes</h2>
          <p className="text-sm text-muted-foreground">
            Prepare a model for notes from saved transcripts. Your meeting text stays on this device. The download may be several gigabytes.
          </p>
          {notes === 'preparing' && <DownloadStatus percent={notesDownload.percent}
            label={notesModel ? `Preparing the notes model (${getSummaryModelSizeLabel(notesModel) || 'download size varies'})…` : 'Finding a notes model…'} />}
          {notes === 'failed' && <p role="alert" className="text-sm text-destructive">Could not prepare the notes model. Try again or continue with recording.</p>}
          {notes === 'stopping' && <p role="status" className="text-sm text-muted-foreground">Stopping notes model setup…</p>}
          {notesOptIn && notes === 'ready' ? (
            <>
              <p role="status" className="text-sm text-muted-foreground">The notes model is ready. You can generate notes from a saved meeting.</p>
              <div className="flex items-center gap-2 text-sm">
                <Switch id="onboarding-automatic-notes" checked={automaticNotes} disabled={finishing} onCheckedChange={setAutomaticNotes} />
                <label htmlFor="onboarding-automatic-notes">Generate notes automatically after transcription</label>
              </div>
              <p className="text-xs text-muted-foreground">You can turn automatic notes off in Summary settings.</p>
              <Button variant="ghost" onClick={() => { setAutomaticNotes(false); setNotesOptIn(false); }} disabled={finishing}>Skip local notes</Button>
            </>
          ) : notes === 'preparing' ? (
            <Button variant="outline" onClick={cancelNotes}>Cancel notes setup</Button>
          ) : (
            <Button variant="outline" onClick={() => void prepareNotes()} disabled={finishing || notes === 'stopping'}>
              {notes === 'failed' ? 'Retry local notes setup' : 'Download local notes model'}
            </Button>
          )}
        </section>
      </div>
    </OnboardingContainer>
  );
}
