'use client';
import { useSyncExternalStore } from 'react';
import { useLiveAssistanceOwner } from '@/contexts/LiveAssistanceContext';

/** Panels and settings subscribe to the same owner without native listeners. */
export function useLiveAssistance() {
  const actions = useLiveAssistanceOwner();
  const state = useSyncExternalStore(actions.subscribe, actions.getSnapshot, actions.getSnapshot);
  return { state, actions };
}
