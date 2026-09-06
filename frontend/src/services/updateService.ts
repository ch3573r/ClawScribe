import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { Update, type DownloadEvent } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

export type UpdateChannel = 'stable' | 'preview';
export interface UpdateInfo {
  available: boolean;
  currentVersion: string;
  channel: UpdateChannel;
  prerelease: boolean;
  version?: string;
  date?: string;
  body?: string;
}
export interface UpdateProgress {
  downloaded: number;
  total: number;
  percentage: number;
  phase: 'downloading' | 'installing';
}
interface NativeUpdateMetadata {
  rid: number;
  currentVersion: string;
  version: string;
  date?: string;
  body?: string;
  rawJson: Record<string, unknown>;
  prerelease: boolean;
}
interface NativeUpdateCheck {
  channel: UpdateChannel;
  update: NativeUpdateMetadata | null;
}
export interface UpdateSnapshot {
  ready: boolean;
  channel: UpdateChannel;
  currentVersion: string;
  isChecking: boolean;
  isSavingChannel: boolean;
  isInstalling: boolean;
  updateInfo: UpdateInfo | null;
  error: string | null;
}
const initialSnapshot: UpdateSnapshot = {
  ready: false, channel: 'stable', currentVersion: '', isChecking: false,
  isSavingChannel: false, isInstalling: false, updateInfo: null, error: null,
};

export function updateErrorMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (/access is denied|os error 5|blocked by/i.test(message)) {
    return 'Windows blocked the installer. Ask your administrator to review the device security policy.';
  }
  return message || 'The update could not be completed. Try again.';
}

/** One checked resource is shared by startup, Settings, About and the tray. */
export class UpdateService {
  private snapshot = initialSnapshot;
  private listeners = new Set<() => void>();
  private initialization: Promise<void> | null = null;
  private pendingCheck: { channel: UpdateChannel; promise: Promise<UpdateInfo | null> } | null = null;
  private candidate: Update | null = null;
  private generation = 0;
  private lastSuccess = 0;

  getSnapshot = (): UpdateSnapshot => this.snapshot;
  getServerSnapshot = (): UpdateSnapshot => initialSnapshot;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  private publish(patch: Partial<UpdateSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch };
    this.listeners.forEach(listener => listener());
  }
  private async closeCandidate(): Promise<void> {
    const previous = this.candidate;
    this.candidate = null;
    await previous?.close().catch(() => {});
  }

  async initialize(): Promise<void> {
    if (this.snapshot.ready) return;
    if (this.initialization) return this.initialization;
    this.initialization = (async () => {
      try {
        const [channel, currentVersion] = await Promise.all([
          invoke<UpdateChannel>('get_update_channel'), getVersion(),
        ]);
        this.publish({ channel, currentVersion, ready: true, error: null });
      } catch (error) {
        this.publish({ error: updateErrorMessage(error) });
        throw error;
      } finally { this.initialization = null; }
    })();
    return this.initialization;
  }

  async setChannel(channel: UpdateChannel): Promise<void> {
    await this.initialize();
    if (this.snapshot.isInstalling || this.snapshot.isSavingChannel) {
      throw new Error('Wait for the current update operation to finish.');
    }
    if (channel === this.snapshot.channel) return;
    ++this.generation;
    this.pendingCheck = null;
    this.publish({ isSavingChannel: true, isChecking: false, error: null });
    try {
      await invoke('set_update_channel', { channel });
      await this.closeCandidate();
      this.lastSuccess = 0;
      this.publish({ channel, updateInfo: null });
    } catch (error) {
      this.publish({ error: updateErrorMessage(error) });
      throw error;
    } finally { this.publish({ isSavingChannel: false }); }
  }

  async checkForUpdates(force = false): Promise<UpdateInfo | null> {
    await this.initialize();
    if (this.snapshot.isSavingChannel || this.snapshot.isInstalling) return null;
    const channel = this.snapshot.channel;
    if (this.pendingCheck?.channel === channel) return this.pendingCheck.promise;
    if (!force && this.lastSuccess && Date.now() - this.lastSuccess < 24 * 60 * 60 * 1000) {
      return this.snapshot.updateInfo;
    }
    const generation = this.generation;
    this.publish({ isChecking: true, error: null });
    const promise = (async () => {
      try {
        const result = await invoke<NativeUpdateCheck>('check_app_update');
        const candidate = result.update ? new Update(result.update) : null;
        if (generation !== this.generation || result.channel !== this.snapshot.channel) {
          await candidate?.close().catch(() => {});
          return null;
        }
        await this.closeCandidate();
        if (generation !== this.generation) {
          await candidate?.close().catch(() => {});
          return null;
        }
        this.candidate = candidate;
        const info: UpdateInfo = {
          available: candidate !== null,
          currentVersion: candidate?.currentVersion ?? this.snapshot.currentVersion,
          channel, prerelease: result.update?.prerelease ?? false,
          version: candidate?.version, date: candidate?.date, body: candidate?.body,
        };
        this.lastSuccess = Date.now();
        this.publish({ updateInfo: info });
        return info;
      } catch (error) {
        if (generation !== this.generation) return null;
        await this.closeCandidate();
        if (generation !== this.generation) return null;
        this.lastSuccess = 0;
        this.publish({ updateInfo: null, error: updateErrorMessage(error) });
        throw error;
      } finally {
        if (generation === this.generation) {
          this.pendingCheck = null;
          this.publish({ isChecking: false });
        }
      }
    })();
    this.pendingCheck = { channel, promise };
    return promise;
  }

  async downloadAndInstall(expected: UpdateInfo, onProgress: (progress: UpdateProgress) => void, canInstall: () => boolean): Promise<void> {
    if (this.snapshot.isInstalling) return;
    if (this.snapshot.isChecking || this.snapshot.isSavingChannel
        || expected !== this.snapshot.updateInfo || !this.candidate
        || expected.channel !== this.snapshot.channel) {
      throw new Error('The selected update changed. Check for updates again.');
    }
    const candidate = this.candidate;
    this.publish({ isInstalling: true, error: null });
    try {
      const assertIdle = async () => {
        if (!canInstall() || await invoke<boolean>('is_recording')) {
          throw new Error('Finish recording and saving your meeting before installing an update.');
        }
      };
      await assertIdle();
      let downloaded = 0;
      let total = 0;
      await candidate.download((event: DownloadEvent) => {
        if (event.event === 'Started') total = event.data.contentLength ?? 0;
        if (event.event === 'Progress') downloaded += event.data.chunkLength;
        onProgress({ downloaded, total, percentage: total > 0 ? Math.min(100, Math.round(downloaded / total * 100)) : 0, phase: 'downloading' });
      }, { timeout: 300_000 });
      await assertIdle();
      onProgress({ downloaded, total, percentage: 100, phase: 'installing' });
      await candidate.install();
      await relaunch();
    } catch (error) {
      await this.closeCandidate();
      this.lastSuccess = 0;
      this.publish({ updateInfo: null, error: updateErrorMessage(error) });
      throw error;
    } finally { this.publish({ isInstalling: false }); }
  }
}

export const updateService = new UpdateService();
