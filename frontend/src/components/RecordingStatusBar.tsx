'use client';

import { motion } from 'framer-motion';
import { RecordingStatus, useRecordingState } from '@/contexts/RecordingStateContext';

interface RecordingStatusBarProps {
  isPaused?: boolean;
}

export const RecordingStatusBar: React.FC<RecordingStatusBarProps> = ({ isPaused = false }) => {
  // State only. The elapsed time is shown once, next to the recording controls.
  const { status } = useRecordingState();
  const isStarting = status === RecordingStatus.STARTING;

  return (
    <motion.div
      initial={{ opacity: 0, y: -10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -10 }}
      transition={{ duration: 0.2 }}
      className="flex items-center gap-2 px-3 py-2 bg-muted rounded-lg mb-2"
    >
      <div className={`w-2 h-2 rounded-full ${isPaused ? 'bg-warning-foreground' : isStarting ? 'bg-primary animate-pulse' : 'bg-error-foreground animate-pulse'}`} />
      <span className={`text-sm ${isPaused ? 'text-warning-foreground' : 'text-foreground'}`}>
        {isPaused ? 'Paused' : isStarting ? 'Starting recording...' : 'Recording'}
      </span>
    </motion.div>
  );
};
