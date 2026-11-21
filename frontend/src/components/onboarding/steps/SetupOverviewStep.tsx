import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DEFAULT_PARAKEET_MODEL } from '@/constants/modelDefaults';
import { Progress } from '@/components/ui/progress';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { getSummaryModelSizeLabel } from '@/lib/onboarding-summary-model';
import type { RecordingMode } from '@/contexts/RecordingStateContext';

type SetupState = 'idle' | 'preparing' | 'ready' | 'failed';

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
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      stopListening.current?.();
      stopListening.current = null;
    };
  }, []);

  const download = async (modelName: string) => {
    if (!mounted.current) return;
    setPercent(null);
    const speech = kind === 'speech';
    const event = speech ? 'parakeet-model-download-progress' : 'builtin-ai-download-progress';
    let observing = true;
    const unlisten = await listen<DownloadEvent>(event, ({ payload }) => {
      if (!observing || !mounted.current || (speech ? payload.modelName : payload.model) !== modelName) return;
      if (payload.status !== 'downloading' && payload.status !== 'completed') return;
      const measured = Number.isFinite(payload.progress) && payload.progress >= 0 && payload.progress <= 100;
      setPercent(measured && (payload.total_mb ?? 0) > 0 ? Math.round(payload.progress) : null);
    }).catch(() => null);
    if (!mounted.current) {
      unlisten?.();
      return;
    }
    stopListening.current = unlisten;
    try {
      await invoke(speech ? 'parakeet_download_model' : 'builtin_ai_download_model', { modelName });
    } finally {
      observing = false;
      stopListening.current?.();
      stopListening.current = null;
      if (mounted.current) setPercent(null);
    }
  };
  return { percent, download };
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
  onChoose: (mode: RecordingMode, localNotesModel: string | null) => void;
  finishing: boolean;
  error: string | null;
}

export function SetupOverviewStep({ onChoose, finishing, error }: SetupOverviewStepProps) {
  const [speech, setSpeech] = useState<SetupState>('idle');
  const [notes, setNotes] = useState<SetupState>('idle');
  const [notesModel, setNotesModel] = useState<string | null>(null);
  const [notesOptIn, setNotesOptIn] = useState(false);
  const speechDownload = useSetupDownload('speech');
  const notesDownload = useSetupDownload('notes');
  const speechPending = useRef(false);
  const notesPending = useRef(false);

  const prepareSpeech = async () => {
    if (speechPending.current) return;
    speechPending.current = true;
    setSpeech('preparing');
    try {
      await invoke('parakeet_init');
      if (!await invoke<boolean>('parakeet_has_available_models')) {
        await speechDownload.download(DEFAULT_PARAKEET_MODEL);
        const models = await invoke<{ name: string; status: unknown }[]>('parakeet_get_available_models');
        if (!models.some(model => model.name === DEFAULT_PARAKEET_MODEL && model.status === 'Available')) {
          throw new Error('Model unavailable');
        }
      }
      setSpeech('ready');
    } catch {
      setSpeech('failed');
    } finally {
      speechPending.current = false;
    }
  };

  const prepareNotes = async () => {
    if (notesPending.current) return;
    notesPending.current = true;
    setNotesModel(null);
    setNotes('preparing');
    setNotesOptIn(true);
    try {
      const model = await invoke<string>('builtin_ai_get_recommended_model');
      setNotesModel(model);
      const args = { modelName: model, refresh: true };
      if (!await invoke<boolean>('builtin_ai_is_model_ready', args)) {
        await notesDownload.download(model);
        if (!await invoke<boolean>('builtin_ai_is_model_ready', args)) throw new Error('Model unavailable');
      }
      setNotes('ready');
    } catch {
      setNotes('failed');
    } finally {
      notesPending.current = false;
    }
  };

  const choose = (mode: RecordingMode) => onChoose(mode, notesOptIn && notes === 'ready' ? notesModel : null);

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
          {speech === 'failed' && <p role="alert" className="text-sm text-destructive">Could not prepare the speech model. Try again or record audio now.</p>}
          {speech === 'ready'
            ? <Button variant="outline" onClick={() => choose('live')} disabled={finishing}>Continue with live transcription</Button>
            : <Button variant="outline" onClick={() => void prepareSpeech()} disabled={finishing || speech === 'preparing'}>
                {speech === 'failed' ? 'Retry speech model setup' : 'Set up live transcription'}
              </Button>}
        </section>

        <section className="space-y-3 rounded-lg border border-border bg-card p-5">
          <h2 className="font-medium">Local meeting notes</h2>
          <p className="text-sm text-muted-foreground">
            Generate notes on this device after transcription. This may download several gigabytes. You can set it up later in Summary settings.
          </p>
          {notes === 'preparing' && <DownloadStatus percent={notesDownload.percent}
            label={notesModel ? `Preparing the notes model (${getSummaryModelSizeLabel(notesModel) || 'download size varies'})…` : 'Finding a notes model…'} />}
          {notes === 'failed' && <p role="alert" className="text-sm text-destructive">Could not prepare the notes model. Try again or continue with recording.</p>}
          {notes === 'ready' && notesOptIn && <p role="status" className="text-sm text-muted-foreground">The notes model is ready.</p>}
          {notesOptIn && notes !== 'failed' ? (
            <Button variant="ghost" onClick={() => setNotesOptIn(false)} disabled={finishing || notes === 'preparing'}>Skip local notes</Button>
          ) : (
            <Button variant="outline" onClick={() => void prepareNotes()} disabled={finishing || notes === 'preparing'}>
              {notes === 'failed' ? 'Retry local notes setup' : 'Download local notes model'}
            </Button>
          )}
        </section>
      </div>
    </OnboardingContainer>
  );
}
