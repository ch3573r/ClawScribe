'use client';

import React from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { RecordingHealthBanner } from '@/components/RecordingHealthBanner';

interface MainContentProps {
  children: React.ReactNode;
}

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const { sidebarOffset, isSidebarResizing } = useSidebar();

  return (
    <main
      style={{ marginLeft: sidebarOffset }}
      className={`flex flex-col h-[calc(100vh-var(--titlebar-height))] min-h-0 flex-1 overflow-hidden ${isSidebarResizing ? '' : 'transition-all duration-300'}`}
    >
      <RecordingHealthBanner />
      <div className="min-h-0 flex-1 overflow-hidden">{children}</div>
    </main>
  );
};

export default MainContent;
