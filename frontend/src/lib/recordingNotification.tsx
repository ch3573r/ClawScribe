import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';

/**
 * Shows the recording notification toast with compliance message.
 * Checks user preferences and displays a dismissible toast with:
 * - notice to inform participants
 * - "Don't show again" checkbox
 * - Acknowledgment button
 *
 * @returns Promise<void> - Resolves when notification is shown or skipped
 */
export async function showRecordingNotification(): Promise<void> {
  try {
    const { Store } = await import('@tauri-apps/plugin-store');
    const store = await Store.load('preferences.json');
    const showNotification = await store.get<boolean>('show_recording_notification') ?? true;

    if (showNotification) {
      let dontShowAgain = false;

      const toastId = toast.info('🔴 Recording started', {
        description: (
          <div className="space-y-3 min-w-[280px]">
            <p className="text-sm font-medium text-foreground">
              Inform all participants this meeting is being recorded.
            </p>
            <label className="flex items-center gap-2 text-xs cursor-pointer hover:bg-muted p-2 rounded transition-colors">
              <Checkbox
                onCheckedChange={(checked) => {
                  dontShowAgain = checked === true;
                }}
                className="rounded border-input text-primary focus:ring-ring focus:ring-2"
              />
              <span className="select-none text-muted-foreground">Don't show this again</span>
            </label>
            <Button
              variant="ghost"
              onClick={async () => {
                if (dontShowAgain) {
                  const { Store } = await import('@tauri-apps/plugin-store');
                  const store = await Store.load('preferences.json');
                  await store.set('show_recording_notification', false);
                  await store.save();
                }
                Analytics.trackButtonClick('recording_notification_acknowledged', 'toast');
                toast.dismiss(toastId);
              }}
              className="h-auto whitespace-normal w-full px-3 py-1.5 bg-primary text-primary-foreground text-xs rounded hover:bg-primary/90 transition-colors font-medium hover:text-primary-foreground"
            >
              I've notified participants
            </Button>
          </div>
        ),
        duration: 10000,
        position: 'bottom-right',
      });
    }
  } catch (notificationError) {
    console.error('Failed to show recording notification:', notificationError);
    // Don't fail the recording if notification fails
  }
}
