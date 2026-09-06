'use client';

import React, { createContext, useContext, useState, useCallback, useEffect, useSyncExternalStore } from 'react';
import { updateService, type UpdateChannel, type UpdateSnapshot } from '@/services/updateService';
import { UpdateDialog } from './UpdateDialog';
import { showUpdateNotification } from './UpdateNotification';
import { toast } from 'sonner';
import { AUTO_UPDATE_CHECK_CHANGED_EVENT, getAutoUpdateCheckEnabled } from '@/lib/updatePreferences';

interface UpdateCheckContextType extends UpdateSnapshot {
  checkForUpdates: (force?: boolean) => Promise<void>;
  setChannel: (channel: UpdateChannel) => Promise<void>;
  showUpdateDialog: () => void;
}
const UpdateCheckContext = createContext<UpdateCheckContextType | undefined>(undefined);

export function UpdateCheckProvider({ children }: { children: React.ReactNode }) {
  const snapshot = useSyncExternalStore(updateService.subscribe, updateService.getSnapshot, updateService.getServerSnapshot);
  const [showDialog, setShowDialog] = useState(false);
  const [autoCheck, setAutoCheck] = useState(false);
  const handleShowDialog = useCallback(() => setShowDialog(true), []);

  useEffect(() => { toast.dismiss('clawscribe-update'); }, [snapshot.channel]);

  useEffect(() => {
    void updateService.initialize().catch(() => {});
    const sync = (event?: Event) => {
      setAutoCheck(event instanceof CustomEvent && typeof event.detail === 'boolean' ? event.detail : getAutoUpdateCheckEnabled());
    };
    sync();
    window.addEventListener('storage', sync);
    window.addEventListener(AUTO_UPDATE_CHECK_CHANGED_EVENT, sync);
    return () => {
      window.removeEventListener('storage', sync);
      window.removeEventListener(AUTO_UPDATE_CHECK_CHANGED_EVENT, sync);
    };
  }, []);

  const checkForUpdates = useCallback(async (force = true) => {
    if (force) setShowDialog(true);
    try {
      const info = await updateService.checkForUpdates(force);
      if (info?.available && !force) showUpdateNotification(info, handleShowDialog);
    } catch { /* Manual checks show the shared error; startup stays quiet. */ }
  }, [handleShowDialog]);

  useEffect(() => {
    if (!autoCheck || !snapshot.ready) return;
    const timer = setTimeout(() => void checkForUpdates(false), 2000);
    return () => clearTimeout(timer);
  }, [autoCheck, snapshot.ready, snapshot.channel, checkForUpdates]);

  useEffect(() => {
    const handleTrayCheck = () => void checkForUpdates(true);
    window.addEventListener('check-updates-from-tray', handleTrayCheck);
    return () => window.removeEventListener('check-updates-from-tray', handleTrayCheck);
  }, [checkForUpdates]);

  const setChannel = useCallback((channel: UpdateChannel) => updateService.setChannel(channel), []);
  return (
    <UpdateCheckContext.Provider value={{ ...snapshot, checkForUpdates, setChannel, showUpdateDialog: handleShowDialog }}>
      {children}
      <UpdateDialog open={showDialog} onOpenChange={setShowDialog} />
    </UpdateCheckContext.Provider>
  );
}

export function useUpdateCheckContext() {
  const context = useContext(UpdateCheckContext);
  if (!context) throw new Error('useUpdateCheckContext must be used within UpdateCheckProvider');
  return context;
}
