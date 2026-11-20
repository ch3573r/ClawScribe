import React from 'react';
import { Download } from 'lucide-react';
import { toast } from 'sonner';
import { UpdateInfo } from '@/services/updateService';

export function showUpdateNotification(updateInfo: UpdateInfo, onUpdateClick: () => void) {
  toast.info('Update available', {
    id: 'clawscribe-update',
    icon: <Download className="h-4 w-4" />,
    description: `${updateInfo.prerelease ? 'Prerelease' : 'Version'} ${updateInfo.version} is now available`,
    action: { label: 'View details', onClick: onUpdateClick },
    duration: 10000,
  });
}
