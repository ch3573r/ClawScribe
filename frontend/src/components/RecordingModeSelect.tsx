'use client';
import { RecordingMode, useRecordingState } from '@/contexts/RecordingStateContext';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';

const UNSELECTED_RECORDING_MODE = '__unset_recording_mode__';

export function RecordingModeSelect() {
  const state = useRecordingState();
  return <div className="space-y-2">
    <label className="block space-y-1 text-sm font-medium">Recording mode
      <Select value={state.recordingMode || UNSELECTED_RECORDING_MODE} disabled={state.isRecording || state.isStarting || state.isStopping || state.isSavingMode}
        onValueChange={value => void state.setRecordingMode((value === UNSELECTED_RECORDING_MODE ? '' : value) as RecordingMode)}>
        <SelectTrigger className="h-auto w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-none focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {!state.recordingMode && <SelectItem value={UNSELECTED_RECORDING_MODE}>Choose recording mode</SelectItem>}
          <SelectItem value="live">Record with live transcription</SelectItem>
          <SelectItem value="audio_only">Record now, transcribe later</SelectItem>
        </SelectContent>
      </Select>
    </label>
    <p className="text-xs text-muted-foreground">{state.recordingMode === 'audio_only'
      ? 'Always saves audio. No speech model or automatic notes run during this recording. Use Transcribe in the saved meeting when ready.'
      : 'Shows recognized speech while recording. Changes apply to the next recording.'}</p>
    {state.modeError && <p role="alert" className="text-sm text-destructive">{state.modeError}</p>}
  </div>;
}
