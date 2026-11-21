interface DeliverySettings {
  enabled: boolean;
  ready: boolean;
  delivery_state: string;
}

export function openClawDeliveryLabel(status: DeliverySettings): { text: string; tone: 'ok' | 'warning' } {
  if (!status.enabled) return { text: 'Off', tone: 'ok' };
  if (!status.ready) return { text: 'Settings incomplete', tone: 'warning' };
  switch (status.delivery_state) {
    case 'submitted': return { text: 'Last handoff accepted', tone: 'ok' };
    case 'pending': return { text: 'Sending', tone: 'ok' };
    case 'deferred': return { text: 'Handoff needs review', tone: 'warning' };
    case 'failed': return { text: 'Delivery failed', tone: 'warning' };
    case 'unknown': return { text: 'Delivery uncertain', tone: 'warning' };
    default: return { text: 'No confirmed delivery', tone: 'ok' };
  }
}
