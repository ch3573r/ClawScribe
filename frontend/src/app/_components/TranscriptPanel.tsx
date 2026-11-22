import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { PermissionWarning } from '@/components/PermissionWarning';
import { LiveConversationMetrics } from '@/components/LiveConversationMetrics';
import { LiveSourceLanguages } from '@/components/LiveSourceLanguages';
import { RecordingCalendarReview } from '@/components/MeetingAttendance';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Copy } from 'lucide-react';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useConfig } from '@/contexts/ConfigContext';
import { RecordingStatus, useRecordingState } from '@/contexts/RecordingStateContext';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { useIsLinux } from '@/hooks/usePlatform';
import { useMemo, type ReactNode } from 'react';

/**
 * TranscriptPanel Component
 *
 * Displays transcript content with controls for copying and language settings.
 * Uses TranscriptContext, ConfigContext, and RecordingStateContext internally.
 */

interface TranscriptPanelProps {
  // indicates stop-processing state for transcripts; derived from backend statuses.
  isProcessingStop: boolean;
  isStopping: boolean;
  assistance?: ReactNode;
}

export function TranscriptPanel({
  isProcessingStop,
  isStopping,
  assistance,
}: TranscriptPanelProps) {
  // Contexts
  const { transcripts, copyTranscript } = useTranscripts();
  const { selectedLanguage } = useConfig();
  const { isRecording, isPaused, status, sessionMode, sessionId } = useRecordingState();
  const { checkPermissions, isChecking, hasSystemAudio, hasMicrophone } = usePermissionCheck();
  const isLinux = useIsLinux();
  const isStarting = status === RecordingStatus.STARTING;

  // Convert transcripts to segments for virtualized view
  const segments = useMemo(() =>
    transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      confidence: t.confidence,
      speaker: t.speaker,
    })),
    [transcripts]
  );

  const header = (
    <div className="flex flex-col gap-2 p-4">
      <div className="flex justify-center">
        <ButtonGroup>
          {transcripts.length > 0 && (
            <Button variant="outline" size="sm" onClick={copyTranscript} title="Copy transcript">
              <Copy />
              <span className="hidden md:inline">Copy</span>
            </Button>
          )}
        </ButtonGroup>
      </div>
      {isRecording && sessionId && <RecordingCalendarReview key={sessionId} sessionId={sessionId} disabled={isStopping || isProcessingStop} />}
      {isRecording && sessionMode === 'live' && sessionId && (
        <div className="mx-auto w-full max-w-[750px] empty:hidden">
          <LiveConversationMetrics sessionId={sessionId} />
        </div>
      )}
      {isRecording && sessionMode === 'live' && sessionId && selectedLanguage === 'auto' && (
        <LiveSourceLanguages key={sessionId} sessionId={sessionId} />
      )}
    </div>
  );

  const permissionWarning = !isRecording && !isStarting && !isChecking && !isLinux && (
    <div className="flex justify-center px-4 pt-4">
      <PermissionWarning
        hasMicrophone={hasMicrophone}
        hasSystemAudio={hasSystemAudio}
        onRecheck={checkPermissions}
        isRechecking={isChecking}
      />
    </div>
  );

  return (
    <div className="min-w-0 w-full border-r border-border bg-card">
      {isRecording && sessionMode === 'audio_only' ? (
        <div className="h-full overflow-y-auto pb-20">
          <div className="sticky top-0 z-10 bg-card">{header}</div>
          <div className="mx-auto w-2/3 max-w-[750px]">
            {assistance}
            <div role="status" className="py-12 text-center">
              <h2 className="font-semibold">Recording audio</h2>
              <p className="mt-2 text-sm text-muted-foreground">Live transcription is off. Your audio is being saved; transcribe it from the saved meeting when ready.</p>
            </div>
          </div>
        </div>
      ) : (
        <VirtualizedTranscriptView
          header={header}
          beforeTranscript={<>{permissionWarning}{assistance}</>}
          contentClassName="mx-auto w-2/3 max-w-[750px]"
          segments={segments}
          isRecording={isRecording || isStarting}
          isPaused={isPaused}
          isProcessing={isProcessingStop}
          isStopping={isStopping}
          enableStreaming={isRecording}
          showConfidence={true}
        />
      )}
    </div>
  );
}
