import Link from 'next/link';
import { File, Pencil, Trash2 } from 'lucide-react';

interface MeetingNavigationItemProps {
  title: string;
  href: string;
  active: boolean;
  matchContext?: string;
  onOpen: () => void;
  onEdit?: () => void;
  onDelete?: () => void;
}

/** Navigation and editing are siblings, never nested interactive controls. */
export function MeetingNavigationItem({ title, href, active, matchContext, onOpen, onEdit, onDelete }: MeetingNavigationItemProps) {
  return (
    <div className={`group relative rounded-md border ${active ? 'border-primary/25 bg-primary/10' : matchContext ? 'border-amber-400/30 bg-amber-400/10' : 'border-transparent hover:border-sidebar-border hover:bg-sidebar-hover'}`}>
      <Link href={href} prefetch={false} onClick={onOpen} aria-current={active ? 'page' : undefined}
        className="block rounded-md px-2.5 py-2.5 text-sidebar-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        <div className={`flex items-start gap-2.5 ${onEdit || onDelete ? 'pr-20' : ''}`}>
          <span className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-background/60 text-muted-foreground"><File aria-hidden="true" className="h-4 w-4" /></span>
          <span className="min-w-0 flex-1">
            <span className="line-clamp-2 text-sm font-medium leading-5">{title}</span>
            <span className="mt-1 block text-xs text-muted-foreground">Recent meeting</span>
          </span>
        </div>
        {matchContext && <span className="mt-2 block rounded-md border border-amber-400/30 bg-amber-400/10 p-2 text-xs leading-5 text-amber-800 dark:text-amber-100"><span className="font-medium">Match: </span>{matchContext}</span>}
      </Link>
      {(onEdit || onDelete) && <div className="absolute right-1.5 top-2 flex items-center gap-1 opacity-100 transition-opacity sm:opacity-0 sm:group-hover:opacity-100 sm:group-focus-within:opacity-100 motion-reduce:transition-none">
        {onEdit && <button type="button" onClick={onEdit} aria-label={`Edit meeting title: ${title}`}
          className="flex h-9 w-9 items-center justify-center rounded-md text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"><Pencil aria-hidden="true" className="h-4 w-4" /></button>}
        {onDelete && <button type="button" onClick={onDelete} aria-label={`Delete meeting: ${title}`}
          className="flex h-9 w-9 items-center justify-center rounded-md text-muted-foreground hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"><Trash2 aria-hidden="true" className="h-4 w-4" /></button>}
      </div>}
    </div>
  );
}
