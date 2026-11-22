import { needsHttpOptIn, secretDestinationError } from '@/lib/secretDestination';
import { Checkbox } from '@/components/ui/checkbox';

export function UnencryptedHttpOptIn({ urls, checked, onChange, destinationProblem, includeLoopback, description, disabled }: {
  urls: string[]; checked: boolean; onChange: (checked: boolean) => void;
  destinationProblem?: string | null;
  includeLoopback?: boolean;
  description?: string;
  disabled?: boolean;
}) {
  const error = destinationProblem || urls.map(url => secretDestinationError(url, checked, includeLoopback)).find(Boolean);
  const needsOptIn = urls.some(url => needsHttpOptIn(url, includeLoopback));
  if (!needsOptIn && !error) return null;
  return <div className="space-y-1 text-sm">
    {error && <p role="status" className="text-warning-foreground">Settings need attention: {error}</p>}
    {needsOptIn && <><label className="flex items-center gap-2">
      <Checkbox checked={checked} disabled={disabled} onCheckedChange={value => onChange(value === true)} className="h-[13px] w-[13px]" />
      Allow unencrypted HTTP to this local-network server
    </label>
    <p className="text-xs text-muted-foreground">{description ?? "HTTP itself does not encrypt the token/key. Use HTTPS or an encrypted private network such as Tailscale."}</p></>}
  </div>;
}
