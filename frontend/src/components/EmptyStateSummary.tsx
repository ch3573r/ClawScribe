'use client';

import { motion } from 'framer-motion';
import { FileQuestion, Sparkles } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface EmptyStateSummaryProps {
  onGenerate: () => void;
  hasModel: boolean;
  hasTranscript: boolean;
  isGenerating?: boolean;
}

export function EmptyStateSummary({ onGenerate, hasModel, hasTranscript, isGenerating = false }: EmptyStateSummaryProps) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex flex-col items-center justify-center h-full p-8 text-center"
    >
      <FileQuestion className="w-16 h-16 text-muted-foreground mb-4" />
      <h3 className="text-lg font-semibold text-foreground mb-2">
        {hasTranscript ? 'No summary generated yet' : 'No transcript yet'}
      </h3>
      <p className="text-sm text-muted-foreground mb-6 max-w-md">
        {hasTranscript
          ? 'Generate an AI-powered summary of your meeting transcript to get key points, action items, and decisions.'
          : 'Transcribe the recording first, then generate a summary.'}
      </p>

      {hasTranscript && <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <div>
              <Button
                onClick={onGenerate}
                disabled={!hasModel || isGenerating}
                className="gap-2"
              >
                <Sparkles className="w-4 h-4" />
                {isGenerating ? 'Generating...' : 'Generate summary'}
              </Button>
            </div>
          </TooltipTrigger>
          {!hasModel && (
            <TooltipContent>
              <p>Please select a model in Settings first</p>
            </TooltipContent>
          )}
        </Tooltip>
      </TooltipProvider>}

      {hasTranscript && !hasModel && (
        <p className="text-xs text-[hsl(var(--theme-warning-fg))] mt-3">
          Please select a model in Settings first
        </p>
      )}
    </motion.div>
  );
}
