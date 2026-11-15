"use client";

import { createSummaryPresentationExtension } from '../AISummary/summary-surface';
import { useAppTheme } from '@/hooks/useAppTheme';
import { useEffect } from "react";
import type { PartialBlock, Block, BlockNoteEditor } from "@blocknote/core";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/shadcn";
import "@blocknote/shadcn/style.css";
import "@blocknote/core/fonts/inter.css";

interface EditorProps {
  initialContent?: Block[];
  summaryPresentation?: boolean;
  onChange?: (blocks: Block[]) => void;
  editable?: boolean;
  onEditorReady?: (editor: BlockNoteEditor) => void;
}

export default function Editor({ initialContent, onChange, editable = true, onEditorReady, summaryPresentation = false }: EditorProps) {
  const theme = useAppTheme();
  console.log('📝 EDITOR: Initializing BlockNote editor with blocks:', {
    hasContent: !!initialContent,
    blocksCount: initialContent?.length || 0,
    editable
  });

  const editor = useCreateBlockNote({
    initialContent: initialContent as PartialBlock[] | undefined,
    extensions: summaryPresentation ? [createSummaryPresentationExtension] : [],
  });

  console.log('📝 EDITOR: BlockNote editor created successfully');

  useEffect(() => { onEditorReady?.(editor); }, [editor, onEditorReady]);

  // Handle content changes
  useEffect(() => {
    if (!onChange) return;

    const handleChange = () => {
      console.log('📝 EDITOR: Content changed, notifying parent...', {
        blocksCount: editor.document.length
      });
      onChange(editor.document);
    };

    const unsubscribe = editor.onChange(handleChange);

    return () => {
      if (typeof unsubscribe === 'function') {
        console.log('📝 EDITOR: Cleaning up onChange listener');
        unsubscribe();
      }
    };
  }, [editor, onChange]);

  return <BlockNoteView editor={editor} editable={editable} theme={theme} />;
}
