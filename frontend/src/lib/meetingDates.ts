export function formatSidebarMeetingDate(value?: string, now = new Date()): string {
  if (!value) return 'Saved meeting';
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return 'Saved meeting';

  const sameDay = (other: Date) => date.getFullYear() === other.getFullYear()
    && date.getMonth() === other.getMonth() && date.getDate() === other.getDate();
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (sameDay(now) || sameDay(yesterday)) {
    const time = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' }).format(date);
    return `${sameDay(now) ? 'Today' : 'Yesterday'}, ${time}`;
  }

  return new Intl.DateTimeFormat(undefined, {
    day: 'numeric',
    month: 'short',
    ...(date.getFullYear() !== now.getFullYear() ? { year: 'numeric' as const } : {}),
  }).format(date);
}
