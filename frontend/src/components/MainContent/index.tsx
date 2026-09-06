'use client';

import React from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePathname } from 'next/navigation';
import { RecordingHealthBanner } from '@/components/RecordingHealthBanner';

interface MainContentProps {
  children: React.ReactNode;
}

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const { isCollapsed } = useSidebar();
  const pathname = usePathname();
  const useCompactSidebar = isCollapsed || pathname === '/settings';

  return (
    <main
      className={`flex flex-col h-[calc(100vh-var(--titlebar-height))] min-h-0 flex-1 overflow-hidden transition-all duration-300 ${
        useCompactSidebar ? 'ml-16' : 'ml-[17.5rem]'
      }`}
    >
      <RecordingHealthBanner />
      <div className="min-h-0 flex-1 overflow-hidden">{children}</div>
    </main>
  );
};

export default MainContent;
