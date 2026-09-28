import type { MeetingExportOptions } from '@/lib/meetingExportContent';

export function ExportContentOptions({ value, onChange, disabled = false }: {
  value: MeetingExportOptions;
  onChange: (value: MeetingExportOptions) => void;
  disabled?: boolean;
}) {
  return <div className="space-y-4">
    <label className="grid gap-2 text-sm">Include
      <select className="rounded-md border border-input bg-background p-2" value={value.content}
        onChange={event => onChange({ ...value, content: event.target.value as MeetingExportOptions['content'] })} disabled={disabled}>
        <option value="both">Summary and transcript</option>
        <option value="summary">Summary only</option>
        <option value="transcript">Transcript only</option>
      </select>
    </label>
    {value.content !== 'summary' && <label className="flex items-center gap-2 text-sm">
      <input className="accent-[hsl(var(--primary))]" type="checkbox" checked={value.speakers}
        disabled={disabled} onChange={event => onChange({ ...value, speakers: event.target.checked })} />
      Speaker labels
    </label>}
    <label className="flex items-center gap-2 text-sm">
      <input className="accent-[hsl(var(--primary))]" type="checkbox" checked={value.timestamps}
        disabled={disabled} onChange={event => onChange({ ...value, timestamps: event.target.checked })} />
      <span>Timestamps<span className="block text-xs text-muted-foreground">Source times in the summary and line times in the transcript</span></span>
    </label>
  </div>;
}
