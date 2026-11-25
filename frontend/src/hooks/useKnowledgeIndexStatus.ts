"use client";

import { useEffect, useState } from "react";
import { knowledgeService } from "@/services/knowledgeService";
import type { IndexStatus } from "@/types/knowledge";

export function useKnowledgeIndexStatus(active: boolean) {
  const [index, setIndex] = useState<IndexStatus | null>(null);
  const [indexError, setIndexError] = useState(false);
  useEffect(() => {
    if (!active) return;
    let visible = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      try {
        const value = await knowledgeService.indexStatus();
        if (!visible) return;
        setIndex(value);
        setIndexError(false);
        if (value.semantic_enabled && value.pending > 0 && value.reason !== "paused") {
          timer = setTimeout(refresh, 15_000);
        }
      } catch {
        if (!visible) return;
        setIndexError(true);
        timer = setTimeout(refresh, 15_000);
      }
    };
    void refresh();
    return () => { visible = false; clearTimeout(timer); };
  }, [active]);
  return { index, indexError, semanticReady: !!index?.semantic_enabled && index.semantic_ready > 0 && !indexError };
}
