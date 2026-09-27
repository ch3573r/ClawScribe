import { needsHttpOptIn, secretDestinationError } from '@/lib/secretDestination';

export function UnencryptedHttpOptIn({ urls, checked, onChange, destinationProblem }: {
  urls: string[]; checked: boolean; onChange: (checked: boolean) => void;
  destinationProblem?: string | null;
}) {
  const error = destinationProblem || urls.map(url => secretDestinationError(url, checked)).find(Boolean);
  const needsOptIn = urls.some(needsHttpOptIn);
  if (!needsOptIn && !error) return null;
  return <div className="space-y-1 text-sm">
    {error && <p role="status" className="text-amber-600 dark:text-amber-400">Settings need attention: {error}</p>}
    {needsOptIn && <><label className="flex items-center gap-2">
      <input type="checkbox" checked={checked} onChange={e => onChange(e.target.checked)} />
      Allow unencrypted HTTP to this local-network server
    </label>
    <p className="text-xs text-muted-foreground">HTTP itself does not encrypt the token/key. Use HTTPS or an encrypted private network such as Tailscale.</p></>}
  </div>;
}
