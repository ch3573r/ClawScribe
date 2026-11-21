"use client";

import { useState, useEffect, useCallback, useRef, forwardRef, useImperativeHandle } from 'react';
import { useAppTheme } from '@/hooks/useAppTheme';
import dynamic from 'next/dynamic';
import type { EditorSummary, MeetingSummary, Summary } from '@/types';
import { isEditorSummary } from '@/lib/meeting-summary';
import { AISummary } from './index';
import { Block, BlockNoteEditor } from '@blocknote/core';
import { useCreateBlockNote } from '@blocknote/react';
import { BlockNoteView } from '@blocknote/shadcn';
import { createSummaryPresentationExtension } from './summary-surface';
import { blocksToMarkdownSafely } from '@/lib/blocknote-markdown';
import "@blocknote/shadcn/style.css";
import { planSummaryReplacement, ReplaceOptions, ReplacePreview } from "@/lib/summary-text-replace";

// Dynamically import BlockNote Editor to avoid SSR issues
const Editor = dynamic(() => import('../BlockNoteEditor/Editor'), { ssr: false });

import type { SummaryDraft, SaveState } from '@/lib/meetingDrafts';

interface BlockNoteSummaryViewProps {
  onDraftChange?: (draft: SummaryDraft, read: () => Promise<SummaryDraft>) => void;
  saveState?: SaveState;
  readOnly?: boolean;
  summaryData: MeetingSummary | null;
  onSave?: (data: EditorSummary) => void | Promise<void>;
  onSummaryChange?: (summary: Summary) => void;
  status?: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  error?: string | null;
  meeting?: {
    id: string;
    title: string;
    created_at: string;
  };
}

export interface BlockNoteSummaryViewRef {
  saveSummary: () => Promise<void>;
  getMarkdown: () => Promise<string>;
  previewReplacement: (options: ReplaceOptions) => ReplacePreview;
  replaceText: (options: ReplaceOptions, token: string) => void;
  isDirty: boolean;
}

export const BlockNoteSummaryView = forwardRef<BlockNoteSummaryViewRef, BlockNoteSummaryViewProps>(({
  summaryData,
  onSave,
  onSummaryChange,
  status = 'idle',
  error = null,
  meeting,
  onDraftChange,
  saveState,
  readOnly = false,
}, ref) => {
  const theme = useAppTheme();
  const saveInFlight = useRef(false);
  const hasEditorData = summaryData !== null && isEditorSummary(summaryData);
  const data = hasEditorData ? summaryData : null;
  const format = data?.summary_json ? 'blocknote' : hasEditorData ? 'markdown' : 'legacy';
  const [isDirty, setIsDirty] = useState(false);
  const [currentBlocks, setCurrentBlocks] = useState<Block[]>([]);
  const [loadError, setLoadError] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const isContentLoaded = useRef(false);
  const directEditor = useRef<BlockNoteEditor | null>(null);
  const onEditorReady = useCallback((value: BlockNoteEditor) => { directEditor.current = value; }, []);

  // Create BlockNote editor for markdown parsing
  const editor = useCreateBlockNote({
    initialContent: undefined,
    extensions: [createSummaryPresentationExtension]
  });

  // Ignore late parses and reset dirty state only when a new saved summary loads.
  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    isContentLoaded.current = false;
    setLoadError(false);
    setIsDirty(false);
    const ready = () => { timer = setTimeout(() => { if (active) isContentLoaded.current = true; }, 100); };
    if (format === 'markdown' && data?.markdown) {
      void editor.tryParseMarkdownToBlocks(data.markdown).then(blocks => {
        if (!active) return;
        editor.replaceBlocks(editor.document, blocks);
        setCurrentBlocks(blocks);
        ready();
      }).catch(() => { if (active) setLoadError(true); });
    } else if (format === 'blocknote' && data?.summary_json) {
      setCurrentBlocks(data.summary_json);
      ready();
    }
    return () => { active = false; if (timer) clearTimeout(timer); };
  }, [format, data?.markdown, data?.summary_json, editor]);

  useEffect(() => { if (saveState === 'saved') setIsDirty(false); }, [saveState]);

  const handleEditorChange = useCallback((blocks: Block[]) => {
    // Only set dirty flag if content has finished loading
    if (isContentLoaded.current && !readOnly) {
      setCurrentBlocks(blocks);
      setIsDirty(true);
      // Retain immutable blocks immediately; navigation may destroy the editor ref.
      const snapshot = structuredClone(blocks);
      onDraftChange?.({ summary_json: snapshot }, async () => {
        const result = await blocksToMarkdownSafely(editor, snapshot, { source: 'summary-autosave' });
        return { summary_json: snapshot, ...(result.markdown === undefined ? {} : { markdown: result.markdown }) };
      });
    }
  }, [editor, onDraftChange, readOnly]);

  const handleSave = useCallback(async () => {
    if (readOnly || !onSave || !isDirty || saveInFlight.current) return;
    saveInFlight.current = true;

    setIsSaving(true);
    try {
      // Generate markdown from current blocks; preserve BlockNote JSON even if markdown conversion fails.
      const markdownResult = await blocksToMarkdownSafely(editor, currentBlocks, {
        source: 'BlockNoteSummaryView.handleSave',
      });

      const saveData: EditorSummary = {
        summary_json: currentBlocks
      };

      if (markdownResult.markdown !== undefined) {
        saveData.markdown = markdownResult.markdown;
      }

      await onSave(saveData);

      setIsDirty(false);
    } catch {
      throw new Error("Could not save the summary. Your edits are still in the editor.");
    } finally {
      saveInFlight.current = false;
      setIsSaving(false);
    }
  }, [onSave, isDirty, currentBlocks, editor, readOnly]);

  // Expose methods to parent via ref
  useImperativeHandle(ref, () => ({
    saveSummary: handleSave,
    previewReplacement: (options) => {
      const active = format === 'markdown' ? editor : format === 'blocknote' ? directEditor.current : null;
      if (!active || !isContentLoaded.current) throw new Error('This summary is not ready for replacement.');
      return planSummaryReplacement(active.document, options);
    },
    replaceText: (options, token) => {
      const active = format === 'markdown' ? editor : format === 'blocknote' ? directEditor.current : null;
      if (!active || !isContentLoaded.current || isSaving) throw new Error('Wait for the summary to finish loading or saving.');
      const plan = planSummaryReplacement(active.document, options);
      if (plan.token !== token) throw new Error('The summary changed. Preview the replacements again.');
      if (plan.matches) {
        active.replaceBlocks(active.document, plan.blocks);
        handleEditorChange(active.document);
      }
    },
    getMarkdown: async () => {
      try {
        // For markdown format - use the main editor
        if (format === 'markdown' && editor) {
          const markdownResult = await blocksToMarkdownSafely(editor, editor.document, {
            source: 'BlockNoteSummaryView.getMarkdown.markdown',
            fallbackMarkdown: data?.markdown,
          });
          return markdownResult.markdown || '';
        }

        // For blocknote format - use currentBlocks state
        if (format === 'blocknote') {
          const blocks = currentBlocks.length > 0
            ? currentBlocks
            : data?.summary_json || [];

          if (blocks.length > 0 && editor) {
            const markdownResult = await blocksToMarkdownSafely(editor, blocks, {
              source: 'BlockNoteSummaryView.getMarkdown.blocknote',
              fallbackMarkdown: data?.markdown,
            });
            return markdownResult.markdown || '';
          }
          // Fallback: if we have the original data with markdown
          if (data?.markdown) {
            return data.markdown;
          }
        }

        // For legacy format - return empty (handled by parent)
        return '';
      } catch {
        console.warn('Could not read Markdown from the summary editor.');
        return '';
      }
    },
    isDirty
  }), [handleSave, handleEditorChange, isDirty, isSaving, editor, format, currentBlocks, data]);

  if (loadError) return <p role="alert" className="text-sm text-destructive">Could not open the summary editor. Reopen this meeting to retry; the saved notes are retained.</p>;

  // Render legacy format
  if (format === 'legacy') {
    if (readOnly && summaryData && !isEditorSummary(summaryData)) {
      return Object.entries(summaryData).map(([key, section]) => (
        <section key={key} className="mb-4 space-y-1">
          <h3 className="font-semibold">{section.title}</h3>
          {section.blocks.map(block => <p key={block.id} className="whitespace-pre-wrap">{block.content}</p>)}
        </section>
      ));
    }
    return (
      <AISummary
        summary={hasEditorData ? null : summaryData}
        status={status}
        error={error}
        onSummaryChange={onSummaryChange || (() => { })}
      />
    );
  }

  // Render BlockNote format (has summary_json)
  if (format === 'blocknote') {
    return (
      <div className="flex flex-col w-full">
        <div className="w-full">
          <Editor
            key={meeting?.id}
            initialContent={data?.summary_json}
            summaryPresentation
            onEditorReady={onEditorReady}
            onChange={handleEditorChange}
            editable={!isSaving && !readOnly}
          />
        </div>
      </div>
    );
  }

  // Render Markdown format (parse and display in BlockNote)
  if (format === 'markdown') {
    return (
      <div className="flex flex-col w-full">
        <div className="w-full">
          <BlockNoteView
            editor={editor}
            editable={!isSaving && !readOnly}
            onChange={() => {
              if (isContentLoaded.current) {
                handleEditorChange(editor.document);
              }
            }}
            theme={theme}
          />
        </div>
      </div>
    );
  }

  return null;
});

BlockNoteSummaryView.displayName = 'BlockNoteSummaryView';
