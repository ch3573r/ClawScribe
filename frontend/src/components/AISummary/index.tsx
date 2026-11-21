'use client';

import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { Summary, Block } from '@/types';
import { Section } from './Section';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { ExclamationTriangleIcon } from '@heroicons/react/24/outline';

interface Props {
  summary: Summary | null;
  status: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  error: string | null;
  onSummaryChange: (summary: Summary) => void;
}

const generateUniqueId = (sectionKey: string) => {
  return `${sectionKey}-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
};

const ensureUniqueBlockIds = (summary: Summary): Summary => {
  // Deep clone to avoid mutating readonly props
  const updatedSummary: Summary = {};

  Object.entries(summary).forEach(([sectionKey, section]) => {
    // Ensure section has blocks array before mapping
    if (section && Array.isArray(section.blocks)) {
      updatedSummary[sectionKey] = {
        ...section,
        blocks: section.blocks.map(block => ({
          ...block,
          id: block.id.includes(sectionKey) ? block.id : generateUniqueId(sectionKey)
        }))
      };
    } else {
      // Initialize empty blocks array if missing or invalid
      updatedSummary[sectionKey] = {
        title: section?.title || sectionKey,
        blocks: []
      };
    }
  });

  return updatedSummary;
};

export const AISummary = ({ summary, status, error, onSummaryChange }: Props) => {
  const currentSummary = useMemo(() => {
    if (!summary) {
      return {
        Agenda: { title: "Agenda", blocks: [] },
        Decisions: { title: "Decisions", blocks: [] },
        ActionItems: { title: "Action Items", blocks: [] },
        ClosingRemarks: { title: "Closing Remarks", blocks: [] }
      };
    }
    return ensureUniqueBlockIds(summary);
  }, [summary]);

  const [selectedBlocks, setSelectedBlocks] = useState<string[]>([]);
  const [lastSelectedBlock, setLastSelectedBlock] = useState<string | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [dragStartBlock, setDragStartBlock] = useState<string | null>(null);
  const hiddenInputRef = useRef<HTMLTextAreaElement>(null);

  // Undo and redo echo an existing entry back through the parent. Only a new
  // summary should truncate the redo branch and extend the history.
  const history = useRef({ entries: [summary ?? currentSummary], index: 0 });
  useEffect(() => {
    const saved = history.current;
    if (summary && summary !== saved.entries[saved.index]) {
      saved.entries = [...saved.entries.slice(0, saved.index + 1), summary];
      saved.index = saved.entries.length - 1;
    }
  }, [summary]);

  const handleUndo = useCallback(() => {
    const saved = history.current;
    if (saved.index > 0) {
      saved.index -= 1;
      onSummaryChange(saved.entries[saved.index]);
    }
  }, [onSummaryChange]);

  const handleRedo = useCallback(() => {
    const saved = history.current;
    if (saved.index < saved.entries.length - 1) {
      saved.index += 1;
      onSummaryChange(saved.entries[saved.index]);
    }
  }, [onSummaryChange]);

  const getAllBlocks = () => {
    const allBlocks: { id: string; sectionKey: string }[] = [];
    Object.entries(currentSummary).forEach(([sectionKey, section]) => {
      section.blocks.forEach(block => {
        allBlocks.push({ id: block.id, sectionKey });
      });
    });
    return allBlocks;
  };

  const handleBlockNavigate = (blockId: string, direction: 'up' | 'down') => {
    const allBlocks = getAllBlocks();
    const currentIndex = allBlocks.findIndex(b => b.id === blockId);
    
    if (currentIndex === -1) return;
    
    let targetIndex: number;
    if (direction === 'up') {
      targetIndex = currentIndex > 0 ? currentIndex - 1 : currentIndex;
    } else {
      targetIndex = currentIndex < allBlocks.length - 1 ? currentIndex + 1 : currentIndex;
    }
    
    if (targetIndex !== currentIndex) {
      const targetBlock = allBlocks[targetIndex];
      setSelectedBlocks([targetBlock.id]);
      setLastSelectedBlock(targetBlock.id);
    }
  };

  const getBlockRange = (startId: string, endId: string) => {
    const allBlocks = getAllBlocks();
    const startIndex = allBlocks.findIndex(b => b.id === startId);
    const endIndex = allBlocks.findIndex(b => b.id === endId);
    
    if (startIndex === -1 || endIndex === -1) return [];
    
    const start = Math.min(startIndex, endIndex);
    const end = Math.max(startIndex, endIndex);
    
    return allBlocks.slice(start, end + 1).map(b => b.id);
  };

  const handleBlockMouseDown = (blockId: string, e: React.MouseEvent<HTMLDivElement>) => {
    if (!e.shiftKey) {
      setDragStartBlock(blockId);
      setLastSelectedBlock(blockId);
      setSelectedBlocks([blockId]);
    }
    setIsDragging(true);
  };

  const handleBlockMouseEnter = (blockId: string) => {
    if (isDragging && dragStartBlock) {
      const range = getBlockRange(dragStartBlock, blockId);
      setSelectedBlocks(range);
    }
  };

  const handleBlockMouseUp = (blockId: string, e: React.MouseEvent<HTMLDivElement>) => {
    if (e.shiftKey && lastSelectedBlock) {
      const range = getBlockRange(lastSelectedBlock, blockId);
      setSelectedBlocks(range);
    }
    setIsDragging(false);
  };

  const handleBlockChange = (sectionKey: keyof Summary, blockId: string, newContent: string) => {
    onSummaryChange({
      ...currentSummary,
      [sectionKey]: {
        ...currentSummary[sectionKey],
        blocks: currentSummary[sectionKey].blocks.map(block => 
          block.id === blockId ? { ...block, content: newContent } : block
        )
      }
    });
  };

  const handleBlockTypeChange = (blockId: string, newType: Block['type']) => {
    // Find the section key for this block
    let blockSectionKey: string | null = null;
    for (const [sectionKey, section] of Object.entries(currentSummary)) {
      if (section.blocks.some(b => b.id === blockId)) {
        blockSectionKey = sectionKey;
        break;
      }
    }

    if (!blockSectionKey) return;

    onSummaryChange({
      ...currentSummary,
      [blockSectionKey]: {
        ...currentSummary[blockSectionKey],
        blocks: currentSummary[blockSectionKey].blocks.map(block => 
          block.id === blockId ? { ...block, type: newType } : block
        )
      }
    });
  };

  const handleTitleChange = (sectionKey: keyof Summary, newTitle: string) => {
    const updatedSummary = {
      ...currentSummary,
      [sectionKey]: {
        ...currentSummary[sectionKey],
        title: newTitle
      }
    };
    onSummaryChange(updatedSummary);
  };

  const handleDeleteSelectedBlocks = useCallback(() => {
    // Group selected blocks by section
    const blocksBySection = new Map<string, string[]>();
    selectedBlocks.forEach(blockId => {
      Object.entries(currentSummary).forEach(([sectionKey, section]) => {
        if (section.blocks.some(b => b.id === blockId)) {
          const blocks = blocksBySection.get(sectionKey) || [];
          blocks.push(blockId);
          blocksBySection.set(sectionKey, blocks);
        }
      });
    });

    // Create new summary with blocks removed
    const newSummary = { ...currentSummary };
    blocksBySection.forEach((blockIds, sectionKey) => {
      newSummary[sectionKey] = {
        ...newSummary[sectionKey],
        blocks: newSummary[sectionKey].blocks.filter(b => !blockIds.includes(b.id))
      };
    });

    onSummaryChange(newSummary);
    setSelectedBlocks([]);
    setLastSelectedBlock(null);
  }, [currentSummary, onSummaryChange, selectedBlocks]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if ((e.key === 'Delete' || e.key === 'Backspace') && selectedBlocks.length > 1) {
      // Handle multi-block deletion
      e.preventDefault();
      handleDeleteSelectedBlocks();
    }
  };

  const handleCreateNewBlock = (blockId: string, newBlockContent: string, blockType: Block['type'], currentBlockContent?: string) => {
    // Find the section key for this block
    let blockSectionKey: string | null = null;
    let currentBlockIndex = -1;
    
    for (const [sectionKey, section] of Object.entries(currentSummary)) {
      currentBlockIndex = section.blocks.findIndex(b => b.id === blockId);
      if (currentBlockIndex !== -1) {
        blockSectionKey = sectionKey;
        break;
      }
    }

    if (!blockSectionKey) return;

    const currentBlock = currentSummary[blockSectionKey].blocks[currentBlockIndex];
    if (!currentBlock) return;
    
    const newId = generateUniqueId(blockSectionKey);
    
    // Update the blocks array for the specific section
    const updatedBlocks = [...currentSummary[blockSectionKey].blocks];
    
    // Get the type of the new block (inherit from current block for bullets)
    const newBlockType = blockType === 'bullet' ? 'bullet' : 'text';
    
    // Update the current block's content if provided
    if (currentBlockContent !== undefined) {
      updatedBlocks[currentBlockIndex] = {
        ...currentBlock,
        content: currentBlockContent
      };
    }
    
    // Insert new block after current block
    updatedBlocks.splice(currentBlockIndex + 1, 0, {
      id: newId,
      type: newBlockType,
      content: newBlockContent,
      color: currentBlock.color || 'default'
    });
    
    onSummaryChange({
      ...currentSummary,
      [blockSectionKey]: {
        ...currentSummary[blockSectionKey],
        blocks: updatedBlocks
      }
    });
    
    // Focus and select the new block
    setSelectedBlocks([newId]);
    setLastSelectedBlock(newId);
    
    // Use setTimeout to ensure the textarea is mounted
    setTimeout(() => {
      const newTextarea = document.querySelector(`[data-block-id="${newId}"]`) as HTMLTextAreaElement;
      if (newTextarea) {
        newTextarea.focus();
        newTextarea.setSelectionRange(0, 0);
      }
    }, 0);
  };

  const handleBlockDelete = (blockId: string, mergeContent?: string) => {
    // Find the section key for this block
    let blockSectionKey: string | null = null;
    let currentBlockIndex = -1;

    for (const [sectionKey, section] of Object.entries(currentSummary)) {
      currentBlockIndex = section.blocks.findIndex(b => b.id === blockId);
      if (currentBlockIndex !== -1) {
        blockSectionKey = sectionKey;
        break;
      }
    }

    if (!blockSectionKey) return;

    const updatedBlocks = [...currentSummary[blockSectionKey].blocks];
    
    // If there's content to merge and a previous block exists
    if (mergeContent && currentBlockIndex > 0) {
      const previousBlock = updatedBlocks[currentBlockIndex - 1];
      const previousContent = previousBlock.content;
      const cursorPosition = previousContent.length;
      
      // Update previous block with merged content
      updatedBlocks[currentBlockIndex - 1] = {
        ...previousBlock,
        content: previousContent + mergeContent
      };
      
      // Remove current block
      updatedBlocks.splice(currentBlockIndex, 1);
      
      onSummaryChange({
        ...currentSummary,
        [blockSectionKey]: {
          ...currentSummary[blockSectionKey],
          blocks: updatedBlocks
        }
      });

      // Select the previous block and set cursor at merge point
      setSelectedBlocks([previousBlock.id]);
      setLastSelectedBlock(previousBlock.id);
      
      // Use setTimeout to ensure the textarea is mounted
      setTimeout(() => {
        const textarea = document.querySelector(`[data-block-id="${previousBlock.id}"]`) as HTMLTextAreaElement;
        if (textarea) {
          textarea.focus();
          textarea.setSelectionRange(cursorPosition, cursorPosition);
        }
      }, 0);
    } else {
      // Just remove the block if no content to merge
      updatedBlocks.splice(currentBlockIndex, 1);
      
      onSummaryChange({
        ...currentSummary,
        [blockSectionKey]: {
          ...currentSummary[blockSectionKey],
          blocks: updatedBlocks
        }
      });

      // Select the previous block if it exists, otherwise the next block
      if (updatedBlocks.length > 0) {
        const newSelectedBlock = updatedBlocks[Math.max(0, currentBlockIndex - 1)];
        setSelectedBlocks([newSelectedBlock.id]);
        setLastSelectedBlock(newSelectedBlock.id);
      } else {
        setSelectedBlocks([]);
        setLastSelectedBlock(null);
      }
    }
  };

  const getSelectedBlocksContent = useCallback(() => {
    return selectedBlocks
      .map(blockId => {
        for (const section of Object.values(currentSummary)) {
          const block = section.blocks.find(b => b.id === blockId);
          if (block) {
            return block.content;
          }
        }
        return '';
      })
      .filter(Boolean)
      .join('\n');
  }, [selectedBlocks, currentSummary]);

  useEffect(() => {
    if (hiddenInputRef.current && selectedBlocks.length > 1) {
      const content = getSelectedBlocksContent();
      hiddenInputRef.current.value = content;
      hiddenInputRef.current.select();
    }
  }, [selectedBlocks, getSelectedBlocksContent]);

  useEffect(() => {
    const handleMouseUp = () => {
      setIsDragging(false);
    };

    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey)) {
        if (e.key === 'z') {
          e.preventDefault();
          if (e.shiftKey) {
            handleRedo();
          } else {
            handleUndo();
          }
        } else if (e.key === 'c') {
          const blockContents = selectedBlocks.map(blockId => {
            for (const section of Object.values(currentSummary)) {
              const block = section.blocks.find(b => b.id === blockId);
              if (block) {
                return block.content;
              }
            }
            return '';
          }).filter(Boolean);

          navigator.clipboard.writeText(blockContents.join('\n'));
        }
      } else if ((e.key === 'Delete' || e.key === 'Backspace') && selectedBlocks.length > 1) {
        e.preventDefault();
        handleDeleteSelectedBlocks();
      }
    };

    document.addEventListener('mouseup', handleMouseUp);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      document.removeEventListener('mouseup', handleMouseUp);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [selectedBlocks, currentSummary, handleUndo, handleRedo, handleDeleteSelectedBlocks]);

  // Context menu state
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    visible: boolean;
  }>({ x: 0, y: 0, visible: false });

  // Close context menu when clicking outside
  useEffect(() => {
    const handleClickOutside = () => {
      setContextMenu(prev => ({ ...prev, visible: false }));
    };
    document.addEventListener('click', handleClickOutside);
    return () => document.removeEventListener('click', handleClickOutside);
  }, []);

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    
    const menuWidth = 160;
    const menuHeight = 80; // Approximate height for 2 items
    
    let x = e.clientX;
    let y = e.clientY;
    
    // Check right boundary
    if (x + menuWidth > window.innerWidth) {
      x = window.innerWidth - menuWidth - 10;
    }
    
    // Check bottom boundary
    if (y + menuHeight > window.innerHeight) {
      y = window.innerHeight - menuHeight - 10;
    }
    
    // Check left boundary
    if (x < 10) {
      x = 10;
    }
    
    // Check top boundary
    if (y < 10) {
      y = 10;
    }
    
    setContextMenu({
      x,
      y,
      visible: true
    });
  };

  const handleCopyBlocks = useCallback(() => {
    const content = getSelectedBlocksContent();
    navigator.clipboard.writeText(content);
    setContextMenu(prev => ({ ...prev, visible: false }));
  }, [getSelectedBlocksContent]);

  const handleDeleteBlocks = () => {
    handleDeleteSelectedBlocks();
    setContextMenu(prev => ({ ...prev, visible: false }));
  };

  const handleSectionDelete = (sectionKey: keyof Summary) => {
    const newSummary = { ...currentSummary };
    delete newSummary[sectionKey];
    onSummaryChange(newSummary);
  };

  const renderErrorState = () => (
    <div className="w-full p-4 bg-error border border-error-border/30 rounded-lg">
      <div className="flex items-center mb-2">
        <ExclamationTriangleIcon className="h-5 w-5 text-error-foreground mr-2" />
        <h3 className="text-error-foreground font-medium">Error generating summary</h3>
      </div>
      <p className="text-error-foreground text-sm">{error}</p>
      <p className="text-error-foreground text-xs mt-2">Please check your model configuration and API keys, or try again.</p>
    </div>
  );

  const renderLoadingState = () => (
    <div className="w-full p-4 bg-primary/10 border border-primary rounded-lg">
      <div className="flex items-center space-x-3">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-primary border-t-transparent"></div>
        <div>
          <h3 className="text-primary font-medium">
            {status === 'processing' ? 'Processing transcript' : 'Generating summary'}
          </h3>
          <p className="text-primary text-sm">
            {status === 'processing' 
              ? 'Analyzing your transcript...' 
              : 'Creating a detailed summary of your meeting...'}
          </p>
        </div>
      </div>
    </div>
  );

  if (error) {
    return renderErrorState();
  }

  if (status === 'processing' || status === 'summarizing' || status === 'regenerating') {
    return renderLoadingState();
  }

  const hasContent = Object.values(currentSummary).some(section => 
    section?.blocks?.length > 0 && section?.blocks?.some(block => block.content.trim())
  );

  if (!hasContent && status === 'completed') {
    return (
      <div className="w-full p-4 bg-muted border border-border rounded-lg text-center">
        <p className="text-muted-foreground">No summary content available.</p>
        <p className="text-muted-foreground text-sm mt-1">Try generating a new summary.</p>
      </div>
    );
  }

  return (
    <div className="relative">

      
      {selectedBlocks.length > 1 && (
        <Textarea
          ref={hiddenInputRef}
          className="inline-block sr-only w-px h-px min-h-0 p-0 border-0"
          readOnly
          value={getSelectedBlocksContent()}
          tabIndex={-1}
        />
      )}
      
      {/* Context Menu */}
      {contextMenu.visible && selectedBlocks.length > 0 && (
        <div
          className="fixed z-50 bg-card shadow-lg rounded-lg py-1 min-w-[160px] border border-border
                     animate-in fade-in zoom-in-95 duration-150"
          style={{ 
            left: contextMenu.x, 
            top: contextMenu.y
          }}
          onClick={e => e.stopPropagation()}
        >
          <Button variant="ghost"
            className="h-auto rounded-none justify-start gap-0 font-normal text-base whitespace-normal hover:text-foreground w-full px-4 py-2 text-left hover:bg-muted flex items-center space-x-2"
            onClick={handleCopyBlocks}
          >
            <span className="text-muted-foreground">📋</span>
            <span>Copy {selectedBlocks.length > 1 ? `${selectedBlocks.length} blocks` : 'block'}</span>
          </Button>
          <Button variant="ghost"
            className="h-auto rounded-none justify-start gap-0 font-normal text-base whitespace-normal hover:text-error-foreground w-full px-4 py-2 text-left hover:bg-muted text-error-foreground flex items-center space-x-2"
            onClick={handleDeleteBlocks}
          >
            <span>🗑️</span>
            <span>Delete {selectedBlocks.length > 1 ? `${selectedBlocks.length} blocks` : 'block'}</span>
          </Button>
        </div>
      )}

      {Object.keys(currentSummary)
        .filter(key => currentSummary[key]?.blocks?.length > 0)
        .map(key => {
          const section = currentSummary[key];
          return (
            <Section
              key={key}
              section={section}
              sectionKey={key}
              selectedBlocks={selectedBlocks}
              onBlockTypeChange={handleBlockTypeChange}
              onBlockChange={(blockId, content) => handleBlockChange(key, blockId, content)}
              onBlockMouseDown={handleBlockMouseDown}
              onBlockMouseEnter={handleBlockMouseEnter}
              onBlockMouseUp={handleBlockMouseUp}
              onKeyDown={handleKeyDown}
              onTitleChange={handleTitleChange}
              onSectionDelete={handleSectionDelete}
              onBlockDelete={(blockId, mergeContent) => handleBlockDelete(blockId, mergeContent)}
              onContextMenu={handleContextMenu}
              onBlockNavigate={(blockId, direction) => handleBlockNavigate(blockId, direction)}
              onCreateNewBlock={handleCreateNewBlock}
            />
          );
        })}

    </div>
  );
};
