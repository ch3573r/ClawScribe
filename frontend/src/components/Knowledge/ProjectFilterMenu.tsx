"use client";

import { useMemo, type Dispatch, type SetStateAction } from "react";
import { ChevronDown } from "lucide-react";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem,
  DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ProjectFilter } from "@/lib/library";

export function ProjectFilterMenu({ filter, setFilter }: {
  filter: ProjectFilter;
  setFilter: Dispatch<SetStateAction<ProjectFilter>>;
}) {
  const { projectTags, projectTagsLoading, projectTagsError, refreshProjectTags } = useSidebar();
  const tags = useMemo(() => [...new Map(projectTags.map(row => [row.tag.toLowerCase(), row.tag])).values()]
    .sort((a, b) => a.localeCompare(b)), [projectTags]);
  const label = filter.untagged ? "Untagged"
    : filter.tags.length === 0 ? "All projects"
    : filter.tags.length === 1 ? filter.tags[0]
    : `${filter.tags.length} projects`;

  return <>
    <div className="flex items-center gap-2 text-sm">
      Project
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button type="button" variant="outline" aria-label={`Project: ${label}`}
            className="max-w-48 justify-between" disabled={projectTagsLoading || !!projectTagsError}>
            <span className="truncate">{label}</span><ChevronDown className="shrink-0 opacity-50" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" data-project-filter-menu className="max-h-72 overflow-y-auto" onEscapeKeyDown={event => event.stopPropagation()}>
          <DropdownMenuRadioGroup aria-label="Project matching" value={filter.mode}
            onValueChange={mode => setFilter(current => ({ ...current, mode: mode as ProjectFilter["mode"] }))}>
            <DropdownMenuRadioItem value="any" disabled={filter.tags.length < 2} onSelect={event => event.preventDefault()}>Match any</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="all" disabled={filter.tags.length < 2} onSelect={event => event.preventDefault()}>Match all</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuCheckboxItem checked={filter.untagged} onSelect={event => event.preventDefault()}
            onCheckedChange={checked => setFilter(current => ({ ...current, tags: [], untagged: checked === true }))}>Untagged</DropdownMenuCheckboxItem>
          {tags.map(tag => <DropdownMenuCheckboxItem key={tag.toLowerCase()}
            checked={filter.tags.some(selected => selected.toLowerCase() === tag.toLowerCase())}
            onSelect={event => event.preventDefault()}
            onCheckedChange={checked => setFilter(current => ({
              ...current, untagged: false,
              tags: [...current.tags.filter(selected => selected.toLowerCase() !== tag.toLowerCase()), ...(checked ? [tag] : [])],
            }))}>{tag}</DropdownMenuCheckboxItem>)}
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => setFilter({ tags: [], untagged: false, mode: "any" })}>Clear filter</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
    {projectTagsLoading && <span role="status" className="text-xs text-muted-foreground">Loading project tags…</span>}
    {projectTagsError && <Button variant="ghost" size="sm" className="text-destructive" onClick={() => void refreshProjectTags()}>Retry loading tags</Button>}
  </>;
}
