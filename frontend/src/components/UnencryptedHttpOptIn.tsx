import { needsHttpOptIn } from '@/lib/secretDestination';

export function UnencryptedHttpOptIn({ urls, checked, onChange }: {
  urls: string[]; checked: boolean; onChange: (checked: boolean) => void;
}) {
  if (!urls.some(needsHttpOptIn)) return null;
  return <div className="space-y-1 text-sm">
    <label className="flex items-center gap-2">
      <input type="checkbox" checked={checked} onChange={e => onChange(e.target.checked)} />
      Allow unencrypted HTTP to this local-network server
    </label>
    <p className="text-xs text-muted-foreground">The token/key is sent without encryption. Use HTTPS when available.</p>
  </div>;
}
