'use client';

import { useState } from 'react';
import { Pin, Plus, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { SettingsRow } from '@/components/ui/settings-layout';
import { LanguagePickerPopover } from '@/components/LanguagePickerPopover';
import { useRecentLanguages } from '@/hooks/useRecentLanguages';
import { labelForCode } from '@/lib/summary-languages';

export function SummaryLanguageSettings() {
  const { recents, pinned, addRecent, removeRecent, setPinned } = useRecentLanguages();
  const [pickerOpen, setPickerOpen] = useState(false);

  const togglePin = (code: string) => {
    setPinned(pinned === code ? null : code);
  };

  return (
    <SettingsRow
      title="Language"
      description={pinned
        ? `Default: ${labelForCode(pinned)}. Click it again to unset. Up to 5 quick-switch languages`
        : 'Click a language to make it the default. With none pinned, Auto uses the dominant transcript language'}
    >
      <div role="group" aria-label="Summary languages" className="flex max-w-full flex-wrap items-center justify-end gap-2">
        {recents.map((code) => {
          const isPinned = pinned === code;
          return (
            <span
              key={code}
              className={`inline-flex items-center rounded-full border text-sm overflow-hidden ${
                isPinned
                  ? 'bg-primary/10 border-primary text-primary'
                  : 'bg-muted border-border text-foreground'
              }`}
            >
              <Button variant="ghost"
                type="button"
                aria-label={isPinned ? `Unpin ${labelForCode(code)} as default` : `Pin ${labelForCode(code)} as default`}
                aria-pressed={isPinned}
                title={isPinned ? 'Click to unset as default' : 'Click to set as default'}
                onClick={() => togglePin(code)}
                className={`h-auto rounded-none gap-1.5 pl-3 pr-2 py-1 hover:bg-muted hover:brightness-95 active:brightness-90 ${
                  isPinned ? 'text-primary' : 'text-foreground'
                }`}
              >
                <Pin
                  size={14}
                  className={isPinned ? 'text-primary' : 'text-muted-foreground'}
                  fill={isPinned ? 'currentColor' : 'none'}
                />
                {labelForCode(code)}
              </Button>
              <Button variant="ghost" size="icon"
                type="button"
                aria-label={`Remove ${labelForCode(code)}`}
                onClick={() => removeRecent(code)}
                className={`h-auto w-auto rounded-none pr-2.5 pl-0.5 py-1 leading-none [&_svg]:size-3.5 ${isPinned ? 'text-primary hover:text-primary' : 'text-muted-foreground hover:text-foreground'}`}
              >
                <X aria-hidden="true" />
              </Button>
            </span>
          );
        })}

        <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
          <PopoverTrigger asChild>
            <Button variant="outline"
              type="button"
              disabled={recents.length >= 5}
              aria-label="Add language"
              className="h-auto gap-1 rounded-full border-dashed border-border px-3 py-1 text-sm text-muted-foreground"
            >
              <Plus aria-hidden="true" /> Add
            </Button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-auto p-0 border-0 shadow-none bg-transparent">
            <LanguagePickerPopover
              mode="settings"
              value={null}
              onChange={(code) => {
                if (code) addRecent(code);
                setPickerOpen(false);
              }}
              onClose={() => setPickerOpen(false)}
            />
          </PopoverContent>
        </Popover>
      </div>
    </SettingsRow>
  );
}
