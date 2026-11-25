import { useState, type ComponentProps, type ReactNode } from 'react';
import { Check, ChevronsUpDown, RefreshCw } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command';
import { Input } from '@/components/ui/input';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { SettingsRow, type StatusTone } from '@/components/ui/settings-layout';
import { ROW_SELECT } from '@/components/settings/settings-parts';
import { CommitInputRow, SaveMark, withError } from '@/components/settings/summary-provider-parts';
import type { CommitResult, FeedbackState } from '@/components/settings/commit-field';
import type { ModelConfig } from '@/services/configService';
import { cn } from '@/lib/utils';

export type UpdateConfig = (next: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
export type CommitConfig = (next: ModelConfig, failure: string) => Promise<CommitResult>;
export type ProviderStatus = { tone: StatusTone; text: string; meta?: string };
export type ProviderView = {
  content: ReactNode;
  advanced?: ReactNode;
  status?: ProviderStatus;
  check?: () => void | Promise<unknown>;
  checkLabel?: string;
  checking?: boolean;
  checkDisabled?: boolean;
};

export function urlProblem(value: string): string | null {
  try {
    const parsed = new URL(value);
    if (parsed.protocol === 'http:' || parsed.protocol === 'https:') return null;
  } catch { /* Invalid URL */ }
  return 'Enter a URL that starts with http:// or https://';
}

export const requiredProblem = (label: string) => (value: string) => value.trim() ? null : `${label} is required`;

export const numberProblem = (min: number, max: number, integer: boolean, required = false) => (value: string): string | null => {
  if (!value.trim()) return required ? 'This value is required' : null;
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || (integer && !Number.isInteger(parsed)) || parsed < min || parsed > max) {
    return `Enter ${integer ? 'a whole number' : 'a number'} from ${min} to ${max}`;
  }
  return null;
};

export function readModelMap(): Record<string, string> {
  try { return JSON.parse(localStorage.getItem('providerModelMap') || '{}') ?? {}; }
  catch { return {}; }
}

export function rememberModel(provider: string, model: string) {
  try { localStorage.setItem('providerModelMap', JSON.stringify({ ...readModelMap(), [provider]: model })); }
  catch { /* Optional cache */ }
}

type ModelControlProps = {
  value: string;
  onChoose: (model: string) => void;
  feedback?: FeedbackState;
  disabled?: boolean;
  loading?: boolean;
};

export function ModelPicker({ value, onChoose, feedback, disabled, loading, options, id = 'summary-model', title = 'Model' }: ModelControlProps & {
  options: string[];
  id?: string;
  title?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <SettingsRow title={title} htmlFor={id} description={withError(null, feedback?.status === 'error' ? feedback.message : null)}>
      <SaveMark status={feedback?.status} />
      <Popover open={open} onOpenChange={setOpen} modal>
        <PopoverTrigger asChild>
          <Button id={id} type="button" variant="outline" role="combobox" aria-expanded={open} disabled={disabled}
            className={cn(ROW_SELECT, 'justify-between font-normal')}>
            <span className="truncate">{value || (loading ? 'Loading models…' : 'Select a model')}</span>
            <ChevronsUpDown aria-hidden="true" className="size-4 shrink-0 opacity-50" />
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-[250px] p-0" align="end">
          <Command>
            <CommandInput aria-label="Search models" placeholder="Search models" />
            <CommandList className="max-h-[300px]">
              {loading ? <div role="status" className="py-6 text-center text-sm text-muted-foreground">
                <RefreshCw aria-hidden="true" className="mx-auto mb-2 size-4 animate-spin motion-reduce:animate-none" />
                Loading models
              </div> : <>
                <CommandEmpty>No models found</CommandEmpty>
                <CommandGroup>
                  {options.map(model => (
                    <CommandItem key={model} value={model} onSelect={() => { setOpen(false); onChoose(model); }}>
                      <Check aria-hidden="true" className={cn('mr-2 size-4', value === model ? 'opacity-100' : 'opacity-0')} />
                      <span className="truncate">{model}</span>
                    </CommandItem>
                  ))}
                </CommandGroup>
              </>}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </SettingsRow>
  );
}

export function SimpleModelSelect({ value, onChoose, feedback, disabled, loading, options }: ModelControlProps & {
  options: { value: string; label: string }[];
}) {
  return (
    <SettingsRow title="Model" htmlFor="summary-model" description={withError(null, feedback?.status === 'error' ? feedback.message : null)}>
      <SaveMark status={feedback?.status} />
      <Select value={value} onValueChange={onChoose} disabled={disabled || loading}>
        <SelectTrigger id="summary-model" aria-label="Model" className={ROW_SELECT}>
          <SelectValue placeholder={loading ? 'Loading models…' : 'Select a model'} />
        </SelectTrigger>
        <SelectContent>
          {options.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
        </SelectContent>
      </Select>
    </SettingsRow>
  );
}

export function ProviderTextField({ onDraft, ...props }: ComponentProps<typeof CommitInputRow> & {
  onDraft?: (value: string) => void;
}) {
  if (!onDraft) return <CommitInputRow {...props} />;
  const { id, title, description, placeholder, type, inputMode, maxLength, value, disabled, validate } = props;
  const error = validate?.(value);
  return <SettingsRow title={title} htmlFor={id} description={withError(description, error)}>
    <Input id={id} value={value} type={type} inputMode={inputMode} placeholder={placeholder} maxLength={maxLength}
      disabled={disabled} onChange={event => onDraft(event.target.value)} aria-invalid={error ? true : undefined}
      spellCheck={false} autoComplete="off" className="w-72 max-w-full" />
  </SettingsRow>;
}

export function ModelSaveFeedback({ feedback }: { feedback?: FeedbackState }) {
  if (feedback?.status === 'error') return <p role="alert" className="text-sm text-error-foreground">{feedback.message}</p>;
  return <SaveMark status={feedback?.status} />;
}
