'use client';
import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useConfig } from '@/contexts/ConfigContext';
import { knowledgeService } from '@/services/knowledgeService';
import { createLiveAssistanceStore } from '@/lib/live-assistance-state';

const LiveAssistanceContext = createContext<ReturnType<typeof createLiveAssistanceStore> | null>(null);

/** Mounted once in RootLayout, outside route and panel lifetimes. */
export function LiveAssistanceProvider({ children }: { children: ReactNode }) {
  const recording = useRecordingState();
  const { modelConfig } = useConfig();
  const [owner] = useState(() => createLiveAssistanceStore({
    service: knowledgeService,
    listen: (event, callback) => listen<{ session_id?: string; recording_generation?: string; recording_mode?: string }>(event, message => callback(message.payload)),
    interval: (callback, milliseconds) => { const timer = setInterval(callback, milliseconds); return () => clearInterval(timer); },
    uuid: () => globalThis.crypto.randomUUID(),
  }));
  useEffect(() => {
    const unsubscribe = recording.subscribeLifecycle(owner.setRecording);
    const disconnect = owner.connect();
    return () => { unsubscribe(); disconnect(); };
  }, [owner, recording.subscribeLifecycle]);
  useEffect(() => { owner.setRecording(recording.status); }, [owner, recording.status]);
  useEffect(() => {
    owner.configureProvider(modelConfig.provider, modelConfig.provider === 'custom-openai'
      ? modelConfig.customOpenAIModel || modelConfig.model : modelConfig.model);
  }, [owner, modelConfig.provider, modelConfig.model, modelConfig.customOpenAIModel]);
  return <LiveAssistanceContext.Provider value={owner}>{children}</LiveAssistanceContext.Provider>;
}

export function useLiveAssistanceOwner() {
  const owner = useContext(LiveAssistanceContext);
  if (!owner) throw new Error('Live assistance requires the app provider.');
  return owner;
}
