# ClawScribe Theme Notes

ClawScribe supports dark theme, light theme, a system-theme option, a
user-selectable accent color, and an interface font picker. Theme documentation
should describe those product controls rather than a fixed one-brand palette.

## Current Design Principles

- Support dark and light modes equally.
- Treat the user's selected accent color as the primary action/focus color.
- Keep neutral surfaces quiet so transcript, summary, and export content stays
  readable.
- Keep control radii restrained; the UI should not drift back into overly round
  pill-heavy styling.
- Use semantic tokens in new UI instead of hardcoded Tailwind color families.
- Do not make screenshots or docs depend on one specific accent color.

## Core Tokens

Use semantic utilities and CSS variables where possible:

| Purpose | Preferred token/utility |
| --- | --- |
| App canvas | `bg-background` |
| Main panels | `bg-card` |
| Popovers/menus | `bg-popover` |
| Primary text | `text-foreground` |
| Secondary text | `text-muted-foreground` |
| Borders | `border-border`, `border-input` |
| Primary action | `bg-primary`, `text-primary`, `text-primary-foreground` |
| Accent/focus | `bg-accent`, `ring-ring` |
| Sidebar | sidebar CSS variables |

Hardcoded colors are acceptable for semantic status only: destructive, warning,
success, recording, disabled, and overlay transparency.

## Accent Color

The accent color is user-configurable. New components should inherit the
existing `primary`, `accent`, and focus-ring variables rather than choosing a
new blue/green/purple. When a component needs a tint, derive it with opacity or
the existing semantic surface variables.

Do not document a single mandatory brand accent as the product palette. The logo
can carry the brand color; the UI must adapt.

## Typography

The interface font can be changed by the user. Components should not hard-code a
font family unless they are rendering code, monospace diagnostics, or exported
content that intentionally differs from the app shell.

Font changes must not resize fixed-format controls enough to break layout. Use
responsive constraints for toolbars, sidebars, transcript rows, cards, and
buttons.

## Custom Title Bar

The app uses its own title bar/window controls. Title-bar styling should follow
the app theme and feel native to ClawScribe, not like a default Windows title
bar pasted above the app.

Title-bar requirements:

- works in dark and light modes
- uses the app icon and product name
- preserves drag behavior
- keeps minimize, maximize/restore, and close controls predictable
- avoids clashing with the user's accent color

## Screenshot Policy

The old `docs/brand/screenshots/` images were removed because they showed an
obsolete light-only UI. Future screenshots should be regenerated from the
current Tauri app and should include:

- main meeting-notes view in dark mode
- settings/integrations view in dark mode
- at least one light-mode view
- a non-default accent-color example when documenting theming

Do not commit screenshots of local meetings, real attendee names, real tenant
URLs, logs, tokens, or private file paths.

## Brand Assets

Current retained assets:

- `docs/brand/clawscribe-logo.png`
- `docs/brand/clawscribe-readme-hero.png`
- `docs/brand/clawscribe-icon.md`

Installer, executable, taskbar, About, README, and release assets should use the
same icon family unless a platform-specific size/format is required.
