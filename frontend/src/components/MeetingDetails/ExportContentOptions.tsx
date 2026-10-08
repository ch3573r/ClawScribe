import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { MeetingExportOptions } from '@/lib/meetingExportContent';

export function ExportContentOptions({ value, onChange, disabled = false }: {
  value: MeetingExportOptions;
  onChange: (value: MeetingExportOptions) => void;
  disabled?: boolean;
}) {
  return <div className="space-y-4">
    <label className="grid gap-2 text-sm">Include
      <Select value={value.content}
        onValueChange={content => onChange({ ...value, content: content as MeetingExportOptions['content'] })} disabled={disabled}>
        <SelectTrigger className="h-auto rounded-md border border-input bg-background p-2 shadow-none"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="both">Summary and transcript</SelectItem>
          <SelectItem value="summary">Summary only</SelectItem>
          <SelectItem value="transcript">Transcript only</SelectItem>
        </SelectContent>
      </Select>
    </label>
    {value.content !== 'summary' && <label className="flex items-center gap-2 text-sm">
      <Checkbox className="h-[13px] w-[13px]" checked={value.speakers}
        disabled={disabled} onCheckedChange={checked => onChange({ ...value, speakers: checked === true })} />
      Speaker labels
    </label>}
    <label className="flex items-center gap-2 text-sm">
      <Checkbox className="h-[13px] w-[13px]" checked={value.timestamps}
        disabled={disabled} onCheckedChange={checked => onChange({ ...value, timestamps: checked === true })} />
      <span>Timestamps<span className="block text-xs text-muted-foreground">Source times in the summary and line times in the transcript</span></span>
    </label>
  </div>;
}
