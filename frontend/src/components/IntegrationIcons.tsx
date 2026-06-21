type IntegrationIconProps = {
  className?: string;
};

export function Microsoft365Icon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="2" y="2" width="9" height="9" rx="1" fill="#F25022" />
      <rect x="13" y="2" width="9" height="9" rx="1" fill="#7FBA00" />
      <rect x="2" y="13" width="9" height="9" rx="1" fill="#00A4EF" />
      <rect x="13" y="13" width="9" height="9" rx="1" fill="#FFB900" />
    </svg>
  );
}

export function OneNoteIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="4" width="13" height="16" rx="2" fill="#7719AA" />
      <path d="M16 6.5h3.5A1.5 1.5 0 0 1 21 8v8a1.5 1.5 0 0 1-1.5 1.5H16z" fill="#A437DB" />
      <path d="M7 8h2.1l3 5.1V8H14v8h-2.1l-3-5.1V16H7z" fill="#fff" />
      <path d="M17.5 9h2M17.5 12h2M17.5 15h2" stroke="#fff" strokeLinecap="round" strokeWidth="1.25" />
    </svg>
  );
}

export function PlannerIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="3" width="18" height="18" rx="4" fill="#107C41" />
      <rect x="7" y="7" width="4.5" height="4" rx="1" fill="#7FBA00" />
      <rect x="13" y="7" width="4" height="9.5" rx="1" fill="#33C481" />
      <rect x="7" y="13" width="4.5" height="4" rx="1" fill="#DFF6DD" />
      <path d="m7.8 14.9 1.1 1.1 2-2.3" fill="none" stroke="#107C41" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.25" />
    </svg>
  );
}

export function TeamsIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <circle cx="17.5" cy="7" r="3" fill="#7B83EB" />
      <circle cx="18.5" cy="14.5" r="4.5" fill="#5059C9" />
      <rect x="3" y="6" width="12" height="12" rx="2.5" fill="#6264A7" />
      <path d="M6.3 9h6.2v1.6h-2.2V15H8.5v-4.4H6.3z" fill="#fff" />
    </svg>
  );
}

export function OutlookCalendarIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="4" width="18" height="17" rx="3" fill="#0078D4" />
      <rect x="6" y="8" width="12" height="10" rx="1.5" fill="#fff" />
      <path d="M6 10h12" stroke="#0078D4" strokeWidth="1.25" />
      <path d="M9 3v3M15 3v3" stroke="#50E6FF" strokeLinecap="round" strokeWidth="1.5" />
      <rect x="8" y="12" width="2.4" height="2.4" rx=".5" fill="#0078D4" />
      <rect x="11" y="12" width="2.4" height="2.4" rx=".5" fill="#0078D4" opacity=".65" />
      <rect x="14" y="12" width="2.4" height="2.4" rx=".5" fill="#0078D4" opacity=".35" />
    </svg>
  );
}

export function ConfluenceIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <path
        d="M5.1 14.6c1.1 1.7 2.6 2.7 4.5 2.7 2 0 3.4-.9 4.8-3l1.2-1.8c.7-1 1.2-1.4 2-1.4.9 0 1.5.5 2.1 1.5l2.3-1.4c-1.1-1.9-2.6-3-4.6-3s-3.4 1-4.8 3l-1.2 1.8c-.7 1-1.3 1.4-2 1.4-.9 0-1.5-.5-2.1-1.4z"
        fill="#0052CC"
      />
      <path
        d="M18.9 9.4c-1.1-1.7-2.6-2.7-4.5-2.7-2 0-3.4.9-4.8 3l-1.2 1.8c-.7 1-1.2 1.4-2 1.4-.9 0-1.5-.5-2.1-1.5L2 12.8c1.1 1.9 2.6 3 4.6 3s3.4-1 4.8-3l1.2-1.8c.7-1 1.3-1.4 2-1.4.9 0 1.5.5 2.1 1.4z"
        fill="#2684FF"
      />
    </svg>
  );
}

export function OpenClawIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="3" width="18" height="18" rx="5" fill="#0F766E" />
      <path d="M7 14.5c2.1 2.5 6.6 3.1 10 0" fill="none" stroke="#5EEAD4" strokeLinecap="round" strokeWidth="1.8" />
      <path d="M8 10.7 10.4 8l2.1 3.8L15.9 7" fill="none" stroke="#ECFEFF" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.7" />
      <circle cx="17" cy="13" r="1.4" fill="#99F6E4" />
    </svg>
  );
}

export function CodexIcon({ className = "h-5 w-5" }: IntegrationIconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      <rect x="3" y="4" width="18" height="16" rx="3" fill="#111827" />
      <path d="m8 9 3 3-3 3" fill="none" stroke="#38BDF8" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.8" />
      <path d="M12.5 15h4" stroke="#A7F3D0" strokeLinecap="round" strokeWidth="1.8" />
      <rect x="4.5" y="5.5" width="15" height="13" rx="2" fill="none" stroke="#334155" />
    </svg>
  );
}
