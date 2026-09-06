'use client';

import { motion } from 'framer-motion';
import { RecordingStatus, useRecordingState, useRecordingClock } from '@/contexts/RecordingStateContext';
import { useEffect, useState } from 'react';

interface RecordingStatusBarProps {
  isPaused?: boolean;
}

export const RecordingStatusBar: React.FC<RecordingStatusBarProps> = ({ isPaused = false }) => {
  // Get recording duration from backend-synced context (in seconds)
  // Backend polls every 500ms, providing smooth updates
  const { status } = useRecordingState();
  const { activeDuration } = useRecordingClock();
  const isStarting = status === RecordingStatus.STARTING;

  // Display state synced from backend
  const [displaySeconds, setDisplaySeconds] = useState(0);

  // Sync with backend duration when it changes (handles refresh/navigation)
  useEffect(() => {
    if (activeDuration !== null) {
      // Round to nearest second to avoid decimal issues
      setDisplaySeconds(Math.floor(activeDuration));
    }
  }, [activeDuration]);

  const formatDuration = (seconds: number): string => {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  };

  return (
    <motion.div
      initial={{ opacity: 0, y: -10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -10 }}
      transition={{ duration: 0.2 }}
      className="flex items-center gap-2 px-3 py-2 bg-muted rounded-lg mb-2"
    >
      <div className={`w-2 h-2 rounded-full ${isPaused ? 'bg-orange-500' : isStarting ? 'bg-primary animate-pulse motion-reduce:animate-none' : 'bg-red-500 animate-pulse motion-reduce:animate-none'}`} />
      <span className={`text-sm ${isPaused ? 'text-orange-800 dark:text-orange-300' : 'text-foreground'}`}>
        {isPaused ? 'Paused' : isStarting ? 'Starting recording...' : 'Recording'}
        {!isStarting && ` • ${formatDuration(displaySeconds)}`}
      </span>
    </motion.div>
  );
};
