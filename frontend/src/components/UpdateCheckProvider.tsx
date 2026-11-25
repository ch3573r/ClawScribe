'use client';

import React, { createContext, useContext, useState, useCallback, useEffect, useSyncExternalStore } from 'react';
import { updateService, type UpdateChannel, type UpdateSnapshot } from '@/services/updateService';
import { UpdateDialog } from './UpdateDialog';
import { showUpdateNotification } from './UpdateNotification';
import { toast } from 'sonner';
import { AUTO_UPDATE_CHECK_CHANGED_EVENT, getAutoUpdateCheckEnabled, loadAutoUpdateCheckEnabled } from '@/lib/updatePreferences';

interface UpdateCheckContextType extends UpdateSnapshot {
  checkForUpdates: (force?: boolean) => Promise<void>;
  setChannel: (channel: UpdateChannel) => Promise<void>;
  showUpdateDialog: () => void;
}
const UpdateCheckContext = createContext<UpdateCheckContextType | undefined>(undefined);
const AUTOMATIC_CHECK_INTERVAL_MS = 60 * 60 * 1000;

export function UpdateCheckProvider({ children }: { children: React.ReactNode }) {
  const snapshot = useSyncExternalStore(updateService.subscribe, updateService.getSnapshot, updateService.getServerSnapshot);
  const [showDialog, setShowDialog] = useState(false);
  const [autoCheck, setAutoCheck] = useState(false);
  const handleShowDialog = useCallback(() => setShowDialog(true), []);

  useEffect(() => { toast.dismiss('clawscribe-update'); }, [snapshot.channel]);

  useEffect(() => {
    let active = true;
    let preferenceLoaded = false;
    void updateService.initialize().catch(() => {});
    const sync = (event?: Event) => {
      if (event instanceof CustomEvent && typeof event.detail === 'boolean') setAutoCheck(event.detail);
      else if (preferenceLoaded) setAutoCheck(getAutoUpdateCheckEnabled());
    };
    void loadAutoUpdateCheckEnabled().then(() => {
      if (!active) return;
      preferenceLoaded = true;
      sync();
    });
    window.addEventListener('storage', sync);
    window.addEventListener(AUTO_UPDATE_CHECK_CHANGED_EVENT, sync);
    return () => {
      active = false;
      window.removeEventListener('storage', sync);
      window.removeEventListener(AUTO_UPDATE_CHECK_CHANGED_EVENT, sync);
    };
  }, []);

  const checkForUpdates = useCallback(async (force = true) => {
    if (force) setShowDialog(true);
    try { await updateService.checkForUpdates(force); }
    catch { /* Manual checks show the shared error. */ }
  }, []);

  useEffect(() => {
    if (!autoCheck) return;
    let active = true;
    let notifiedVersion: string | undefined;
    const check = async () => {
      try {
        const info = await updateService.checkForUpdates(false);
        if (active && info?.available && info.version !== notifiedVersion
            && info === updateService.getSnapshot().updateInfo) {
          notifiedVersion = info.version;
          showUpdateNotification(info, handleShowDialog);
        }
      } catch { /* Keep automatic checks quiet and retry on the next tick. */ }
    };
    // The main webview stays mounted while the window is hidden in the tray.
    // The service shares in-flight checks and limits successful checks to daily.
    const startup = setTimeout(() => void check(), 2000);
    const periodic = setInterval(() => void check(), AUTOMATIC_CHECK_INTERVAL_MS);
    return () => {
      active = false;
      clearTimeout(startup);
      clearInterval(periodic);
    };
  }, [autoCheck, snapshot.channel, handleShowDialog]);

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
