type IntegrationIconProps = {
  className?: string;
};

export function Microsoft365Icon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="2" y="2" width="9" height="9" rx="1" fill="hsl(var(--theme-error-fg))" />
      <rect x="13" y="2" width="9" height="9" rx="1" fill="hsl(var(--theme-success-fg))" />
      <rect x="2" y="13" width="9" height="9" rx="1" fill="hsl(var(--chart-3))" />
      <rect x="13" y="13" width="9" height="9" rx="1" fill="hsl(var(--theme-warning-fg))" />
    </svg>
  );
}

export function WordIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <path d="M15 4h4a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-4z" fill="hsl(var(--chart-3))" />
      <rect x="3" y="4" width="14" height="16" rx="2" fill="hsl(var(--primary))" />
      <path d="M5 8h1.8l1.1 5.2L9.1 8h1.8l1.2 5.2L13.2 8H15l-2 8h-1.8L10 10.8 8.8 16H7z" fill="hsl(var(--primary-foreground))" />
    </svg>
  );
}

export function OneNoteIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="4" width="13" height="16" rx="2" fill="hsl(var(--chart-5))" />
      <path d="M16 6.5h3.5A1.5 1.5 0 0 1 21 8v8a1.5 1.5 0 0 1-1.5 1.5H16z" fill="hsl(var(--chart-5) / 0.65)" />
      <path d="M7 8h2.1l3 5.1V8H14v8h-2.1l-3-5.1V16H7z" fill="hsl(var(--primary-foreground))" />
      <path d="M17.5 9h2M17.5 12h2M17.5 15h2" stroke="hsl(var(--primary-foreground))" strokeLinecap="round" strokeWidth="1.25" />
    </svg>
  );
}

export function PlannerIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="3" width="18" height="18" rx="4" fill="hsl(var(--theme-success-fg))" />
      <rect x="7" y="7" width="4.5" height="4" rx="1" fill="hsl(var(--chart-2))" />
      <rect x="13" y="7" width="4" height="9.5" rx="1" fill="hsl(var(--primary))" />
      <rect x="7" y="13" width="4.5" height="4" rx="1" fill="hsl(var(--theme-success-bg))" />
      <path d="m7.8 14.9 1.1 1.1 2-2.3" fill="none" stroke="hsl(var(--theme-success-fg))" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.25" />
    </svg>
  );
}

export function ToDoIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="3" width="18" height="18" rx="4" fill="hsl(var(--primary))" />
      <path
        d="m7.1 12.3 2.7 2.7 7-7.2"
        fill="none"
        stroke="hsl(var(--primary-foreground))"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="2.1"
      />
      <path
        d="M6.5 7.2h4.1M6.5 17h10.2"
        stroke="hsl(var(--primary-foreground) / 0.65)"
        strokeLinecap="round"
        strokeWidth="1.35"
      />
    </svg>
  );
}

export function OneDriveIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <path
        d="M10.2 8.6a5.4 5.4 0 0 1 9.4 2.8 4.2 4.2 0 0 1-.5 8.4H7.4a4.9 4.9 0 0 1-1.1-9.7 5.7 5.7 0 0 1 3.9-1.5z"
        fill="hsl(var(--chart-1))"
      />
      <path
        d="M10.2 8.6a5.7 5.7 0 0 0-3.9 1.5 4.9 4.9 0 0 0-3.6 4.7 4.8 4.8 0 0 0 .8 2.7l8.4-5.1 5.2 3.2 2.5-4.2a5.4 5.4 0 0 0-9.4-2.8z"
        fill="hsl(var(--chart-3))"
      />
      <path
        d="M7.4 19.8h11.7a4.2 4.2 0 0 0 3-7.2l-5 3-5.2-3.2-8.4 5.1a4.9 4.9 0 0 0 3.9 2.3z"
        fill="hsl(var(--chart-4))"
      />
    </svg>
  );
}

export function TeamsIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <circle cx="17.5" cy="7" r="3" fill="hsl(var(--chart-3))" />
      <circle cx="18.5" cy="14.5" r="4.5" fill="hsl(var(--chart-1))" />
      <rect x="3" y="6" width="12" height="12" rx="2.5" fill="hsl(var(--primary))" />
      <path d="M6.3 9h6.2v1.6h-2.2V15H8.5v-4.4H6.3z" fill="hsl(var(--primary-foreground))" />
    </svg>
  );
}

export function OutlookCalendarIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="4" width="18" height="17" rx="3" fill="hsl(var(--primary))" />
      <rect x="6" y="8" width="12" height="10" rx="1.5" fill="hsl(var(--primary-foreground))" />
      <path d="M6 10h12" stroke="hsl(var(--primary))" strokeWidth="1.25" />
      <path d="M9 3v3M15 3v3" stroke="hsl(var(--chart-3))" strokeLinecap="round" strokeWidth="1.5" />
      <rect x="8" y="12" width="2.4" height="2.4" rx=".5" fill="hsl(var(--primary))" />
      <rect x="11" y="12" width="2.4" height="2.4" rx=".5" fill="hsl(var(--primary))" opacity=".65" />
      <rect x="14" y="12" width="2.4" height="2.4" rx=".5" fill="hsl(var(--primary))" opacity=".35" />
    </svg>
  );
}

export function ConfluenceIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <path
        d="M5.1 14.6c1.1 1.7 2.6 2.7 4.5 2.7 2 0 3.4-.9 4.8-3l1.2-1.8c.7-1 1.2-1.4 2-1.4.9 0 1.5.5 2.1 1.5l2.3-1.4c-1.1-1.9-2.6-3-4.6-3s-3.4 1-4.8 3l-1.2 1.8c-.7 1-1.3 1.4-2 1.4-.9 0-1.5-.5-2.1-1.4z"
        fill="hsl(var(--chart-1))"
      />
      <path
        d="M18.9 9.4c-1.1-1.7-2.6-2.7-4.5-2.7-2 0-3.4.9-4.8 3l-1.2 1.8c-.7 1-1.2 1.4-2 1.4-.9 0-1.5-.5-2.1-1.5L2 12.8c1.1 1.9 2.6 3 4.6 3s3.4-1 4.8-3l1.2-1.8c.7-1 1.3-1.4 2-1.4.9 0 1.5.5 2.1 1.4z"
        fill="hsl(var(--chart-3))"
      />
    </svg>
  );
}

export function OpenClawIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="3" width="18" height="18" rx="5" fill="hsl(var(--foreground))" />
      <path d="M7 14.5c2.1 2.5 6.6 3.1 10 0" fill="none" stroke="hsl(var(--theme-success-bg))" strokeLinecap="round" strokeWidth="1.8" />
      <path d="M8 10.7 10.4 8l2.1 3.8L15.9 7" fill="none" stroke="hsl(var(--background))" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.7" />
      <circle cx="17" cy="13" r="1.4" fill="hsl(var(--background) / 0.75)" />
    </svg>
  );
}

export function CodexIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="4" width="18" height="16" rx="3" fill="hsl(var(--foreground))" />
      <path d="m8 9 3 3-3 3" fill="none" stroke="hsl(var(--background))" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.8" />
      <path d="M12.5 15h4" stroke="hsl(var(--theme-success-bg))" strokeLinecap="round" strokeWidth="1.8" />
      <rect x="4.5" y="5.5" width="15" height="13" rx="2" fill="none" stroke="hsl(var(--background) / 0.35)" />
    </svg>
  );
}
