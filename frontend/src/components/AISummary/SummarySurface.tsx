'use client';

import type { ReactNode } from 'react';

// Presentation is owned by each editor's ProseMirror decoration plugin.
export function SummarySurface({ children }: { children: ReactNode }) {
  return <div className="meeting-summary-surface">{children}</div>;
}
