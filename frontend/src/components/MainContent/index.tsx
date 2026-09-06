'use client';

import React from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useCompactLayout } from '@/hooks/useCompactLayout';
import { usePathname } from 'next/navigation';

interface MainContentProps {
  children: React.ReactNode;
}

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const { isCollapsed } = useSidebar();
  const pathname = usePathname();
  const compact = useCompactLayout();
  const useCompactSidebar = compact || isCollapsed || pathname === '/settings';

  return (
    <main tabIndex={-1}
      className={`h-[calc(100vh-var(--titlebar-height))] min-h-0 min-w-0 flex-1 overflow-hidden transition-[margin] duration-300 motion-reduce:transition-none ${
        useCompactSidebar ? 'ml-16' : 'ml-[17.5rem]'
      }`}
    >
      {children}
    </main>
  );
};

export default MainContent;
