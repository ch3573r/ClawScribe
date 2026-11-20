import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { loadBehaviorSettings, refreshBehaviorSettings } from '@/lib/behaviorSettings';

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
    // The core owns the preference; it is shown unless the core says it was switched off.
    const showNotification = (await loadBehaviorSettings()).showRecordingNotification;

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
                  await invoke('set_recording_notification_preference', { enabled: false });
                  await refreshBehaviorSettings();
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
