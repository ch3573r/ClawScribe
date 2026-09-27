import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

export async function openExternal(value: string): Promise<void> {
  try {
    const url = new globalThis.URL(value);
    if (!['http:', 'https:'].includes(url.protocol) || !url.hostname ||
        !/^(?:[a-z0-9.-]+|\[[a-f0-9:]+\])$/i.test(url.hostname)) {
      throw new Error('Only web links can be opened.');
    }
    await invoke('open_external_url', { url: url.href });
  } catch {
    toast.error('Could not open link', { description: 'Only valid web links can be opened. Check the link and your default browser.' });
  }
}
