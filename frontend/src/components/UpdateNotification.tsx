import React from 'react';
import { Download } from 'lucide-react';
import { toast } from 'sonner';
import { UpdateInfo } from '@/services/updateService';

export function showUpdateNotification(updateInfo: UpdateInfo, onUpdateClick: () => void) {
  toast.info(
    <div className="flex items-center justify-between gap-4">
      <div className="flex items-center gap-2">
        <Download className="h-4 w-4" />
        <div>
          <p className="font-medium">Update Available</p>
          <p className="text-sm text-muted-foreground">
            {updateInfo.prerelease ? 'Prerelease' : 'Version'} {updateInfo.version} is now available
          </p>
        </div>
      </div>
      <button
        onClick={(e) => {
          e.stopPropagation();
          onUpdateClick();
        }}
        className="text-sm font-medium text-primary hover:text-primary underline"
      >
        View Details
      </button>
    </div>,
    {
      id: 'clawscribe-update',
      duration: 10000,
      position: 'bottom-center',
    }
  );
}
