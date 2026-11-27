import type { ElementType } from "react";
import { SETTINGS_TABS } from "./settings-tabs";
import {
  Info,
  Settings2,
  ShieldCheck,
  Palette,
  Mic,
  Keyboard,
  AudioLines,
  Sparkles,
  BrainCircuit,
  CalendarClock,
  Bot,
  Activity,
  Webhook,
} from "lucide-react";
import { ConfluenceIcon, Microsoft365Icon } from "@/components/IntegrationIcons";

type SectionItem = {
  value: (typeof SETTINGS_TABS)[number];
  label: string;
  icon: ElementType;
  desc: string;
};

// Grouped so the nav reads as a structure, not a flat row of tabs. `value`s are
// stable (the sidebar deep-links via ?tab= and the open-settings-tab event).
export const SETTINGS_GROUPS: { label: string; items: SectionItem[] }[] = [
  {
    label: "Workspace",
    items: [
      { value: "general", label: "General", icon: Settings2, desc: "Windows sign-in startup, close button and system tray, updates, notifications, recording folder, library storage, backup and restore" },
      { value: "privacy", label: "Privacy", icon: ShieldCheck, desc: "What stays on this PC and what leaves it" },
      { value: "appearance", label: "Appearance", icon: Palette, desc: "Theme, accent colour and interface font" },
      { value: "recording", label: "Recording", icon: Mic, desc: "Capture, audio devices and call detection" },
      { value: "shortcuts", label: "Shortcuts", icon: Keyboard, desc: "Choose keys for recording controls and saved meeting playback" },
    ],
  },
  {
    label: "Intelligence",
    items: [
      { value: "transcription", label: "Transcription", icon: AudioLines, desc: "Speech-to-text engine, model and options" },
      { value: "summary", label: "Summary", icon: Sparkles, desc: "Summary model, language, live assistance and templates" },
      { value: "memory", label: "Meeting memory", icon: BrainCircuit, desc: "Search across your meetings, on this PC" },
    ],
  },
  {
    label: "Connections",
    items: [
      { value: "ms365", label: "Microsoft 365", icon: Microsoft365Icon, desc: "Where exports go in OneNote, OneDrive, Planner and To Do" },
      { value: "calendars", label: "Calendars", icon: CalendarClock, desc: "Meeting titles and invitees for your recordings" },
      { value: "confluence", label: "Confluence", icon: ConfluenceIcon, desc: "Publish meeting notes as a Confluence page" },
      { value: "webhook", label: "Webhook", icon: Webhook, desc: "Send saved meeting data to your automation endpoint" },
      { value: "assistant", label: "Assistant access", icon: Bot, desc: "Let an MCP-compatible assistant read your saved meetings" },
    ],
  },
  {
    label: "Advanced",
    items: [
      { value: "diagnostics", label: "Diagnostics", icon: Activity, desc: "Build, engine and connection health for troubleshooting" },
      { value: "about", label: "About ClawScribe", icon: Info, desc: "Version, licensing and bundled software" },
    ],
  },
];

export const SETTINGS_ITEMS = SETTINGS_GROUPS.flatMap((g) => g.items);

export function filterSettingsGroups(query: string) {
  const terms = query.trim().toLowerCase().split(/\s+/);
  return SETTINGS_GROUPS.map(group => ({
    ...group,
    items: group.items.filter(item => {
      const matchingText = `${group.label} ${item.label} ${item.desc}`.toLowerCase();
      return terms.every(term => matchingText.includes(term));
    }),
  })).filter(group => group.items.length > 0);
}
