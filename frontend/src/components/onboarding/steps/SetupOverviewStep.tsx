import { useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { getSummaryModelSizeLabel } from '@/lib/onboarding-summary-model';
import type { RecordingMode } from '@/contexts/RecordingStateContext';

const PARAKEET_MODEL = 'parakeet-tdt-0.6b-v3-int8';
type SetupState = 'idle' | 'preparing' | 'ready' | 'failed';

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
  const speechPending = useRef(false);
  const notesPending = useRef(false);

  const prepareSpeech = async () => {
    if (speechPending.current) return;
    speechPending.current = true;
    setSpeech('preparing');
    try {
      await invoke('parakeet_init');
      if (!await invoke<boolean>('parakeet_has_available_models')) {
        await invoke('parakeet_download_model', { modelName: PARAKEET_MODEL });
        if (!await invoke<boolean>('parakeet_has_available_models')) throw new Error('Model unavailable');
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
    setNotes('preparing');
    setNotesOptIn(true);
    try {
      const model = await invoke<string>('builtin_ai_get_recommended_model');
      setNotesModel(model);
      const args = { modelName: model, refresh: true };
      if (!await invoke<boolean>('builtin_ai_is_model_ready', args)) {
        await invoke('builtin_ai_download_model', { modelName: model });
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
          {speech === 'preparing' && <p role="status" className="text-sm text-muted-foreground">Preparing the speech model…</p>}
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
          {notes === 'preparing' && <p role="status" className="text-sm text-muted-foreground">
            {notesModel ? `Preparing the notes model (${getSummaryModelSizeLabel(notesModel) || 'download size varies'})…` : 'Finding a notes model…'}
          </p>}
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
