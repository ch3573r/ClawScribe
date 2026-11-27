import { toast } from 'sonner';

export async function copyPlainText(text: string, success: string, failure: string): Promise<void> {
  try {
    await globalThis.navigator.clipboard.writeText(text);
    toast.success(success);
  } catch {
    toast.error(failure);
  }
}
