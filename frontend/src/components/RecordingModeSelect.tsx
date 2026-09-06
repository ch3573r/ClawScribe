'use client';
import { RecordingMode, useRecordingState } from '@/contexts/RecordingStateContext';

export function RecordingModeSelect() {
  const state = useRecordingState();
  return <div className="space-y-2">
    <label className="block space-y-1 text-sm font-medium">Recording mode
      <select value={state.recordingMode ?? ''} disabled={state.isRecording || state.isStarting || state.isStopping || state.isSavingMode}
        onChange={e => void state.setRecordingMode(e.target.value as RecordingMode)}
        className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50">
        {!state.recordingMode && <option value="">Choose recording mode</option>}
        <option value="live">Record with live transcription</option>
        <option value="audio_only">Record now, transcribe later</option>
      </select>
    </label>
    <p className="text-xs text-muted-foreground">{state.recordingMode === 'audio_only'
      ? 'Always saves audio. No speech model or automatic notes run during this recording. Use Transcribe in the saved meeting when ready.'
      : 'Shows recognized speech while recording. Changes apply to the next recording.'}</p>
    {state.modeError && <p role="alert" className="text-sm text-destructive">{state.modeError}</p>}
  </div>;
}
