'use client';

import { useRef, useEffect, useState } from 'react';
import { ActionIcon } from './MeetingDetails/ActionIcon';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { Textarea } from '@/components/ui/textarea';

interface EditableTitleProps {
  meetingTitle?: boolean;
  disabled?: boolean;
  title: string;
  isEditing: boolean;
  onStartEditing: () => void;
  onFinishEditing: () => void;
  onChange: (value: string) => void;
  onDelete?: () => void;
}

export const EditableTitle: React.FC<EditableTitleProps> = ({
  title,
  meetingTitle = false,
  disabled = false,
  isEditing,
  onStartEditing,
  onFinishEditing,
  onChange,
  onDelete,
}) => {
  const labelRef = useRef<HTMLSpanElement>(null);
  const [truncated, setTruncated] = useState(false);
  useEffect(() => {
    const node = labelRef.current;
    if (!node) return;
    const measure = () => setTruncated(node.scrollWidth > node.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, [title, isEditing]);
  const titleInputRef = useRef<HTMLTextAreaElement>(null);

  // Auto-resize textarea height based on content
  useEffect(() => {
    if (titleInputRef.current && isEditing) {
      titleInputRef.current.style.height = 'auto';
      titleInputRef.current.style.height = `${titleInputRef.current.scrollHeight}px`;
    }
  }, [title, isEditing]);

  if (meetingTitle) return isEditing ? (
    <Textarea autoFocus rows={1} disabled={disabled} aria-label="Meeting title" value={title} onChange={event => onChange(event.target.value.replace(/[\r\n\u2028\u2029]+/g, ' '))} onBlur={onFinishEditing}
      onKeyDown={event => { if (event.key === 'Enter' || event.key === 'Escape') { event.preventDefault(); onFinishEditing(); } }}
      className="min-h-0 w-full resize-none py-0 text-xl font-semibold leading-7 md:text-xl" />
  ) : (
    <h1 className="min-w-0 text-xl font-semibold leading-7">
      <Tooltip open={truncated ? undefined : false}><TooltipTrigger asChild>
        <Button variant="ghost" disabled={disabled} onClick={onStartEditing} aria-label={`Edit meeting title: ${title}`} className="block h-auto w-full min-w-0 justify-start p-0 text-left text-xl font-semibold hover:bg-muted">
          <span ref={labelRef} className="block truncate">{title}</span>
        </Button>
      </TooltipTrigger>{truncated && <TooltipContent className="max-w-xl">{title}</TooltipContent>}</Tooltip>
    </h1>
  );

  return isEditing ? (
    <div className="flex-1">
      <Textarea
        ref={titleInputRef}
        value={title}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onFinishEditing}
        onKeyDown={(e) => {
          // Allow Enter for new line only with Shift key
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            onFinishEditing();
          }
        }}
        className="inline-block min-h-0 shadow-none md:text-[17px] focus-visible:ring-2 text-[17px] font-semibold bg-muted border border-border focus:outline-none focus:ring-2 focus:ring-ring rounded px-3 py-1 w-full resize-none overflow-hidden"
        style={{ minWidth: '300px', minHeight: '40px' }}
        autoFocus
        rows={1}
      />
    </div>
  ) : (
    <div className="group flex items-center space-x-2 flex-1">
      <h3
        className="text-[17px] font-semibold cursor-pointer hover:bg-muted rounded px-1 flex-1 whitespace-pre-wrap"
        onClick={onStartEditing}
      >
        {title}
      </h3>
      <div className="flex space-x-1">
        <ActionIcon label="Edit section title"
          onClick={onStartEditing}
          className="h-auto w-auto gap-0 font-normal text-base hover:text-foreground opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity duration-200 p-1 hover:bg-muted rounded"
          title="Edit section title"
          aria-label="Edit section title"
        >
          <svg 
            xmlns="http://www.w3.org/2000/svg" 
            width="16" 
            height="16" 
            viewBox="0 0 24 24" 
            fill="none" 
            stroke="currentColor" 
            strokeWidth="2" 
            strokeLinecap="round" 
            strokeLinejoin="round"
          >
            <path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" />
          </svg>
        </ActionIcon>
        {onDelete && (
          <ActionIcon label="Delete section"
            onClick={onDelete}
            className="h-auto w-auto gap-0 font-normal text-base hover:text-error-foreground opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity duration-200 p-1 hover:bg-muted rounded text-error-foreground"
            title="Delete section"
            aria-label="Delete section"
          >
            <svg 
              xmlns="http://www.w3.org/2000/svg" 
              width="16" 
              height="16" 
              viewBox="0 0 24 24" 
              fill="none" 
              stroke="currentColor" 
              strokeWidth="2" 
              strokeLinecap="round" 
              strokeLinejoin="round"
            >
              <path d="M3 6h18" />
              <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
              <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
            </svg>
          </ActionIcon>
        )}
      </div>
    </div>
  );
};
