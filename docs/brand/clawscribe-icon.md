# ClawScribe Icon

The ClawScribe icon is generated from the current product mark supplied by Alex.
The uploaded source had an outer white border/background; generation flood-fills
only edge-connected white pixels, removes that border, crops to the visible
rounded-square mark, and exports transparent-corner assets.

## Generated Assets

Regenerate from the uploaded/source mark with:

```bash
python3 /tmp/regenerate_clawscribe_icons.py
```

Covered surfaces:

- Tauri app/window/default/tray icon: `frontend/src-tauri/icons/icon.png`
- Windows installer/uninstaller icon: `frontend/src-tauri/icons/app_icon.ico`
- Windows `.ico`: `frontend/src-tauri/icons/icon.ico`, `app_icon.ico`
- macOS `.icns`: `frontend/src-tauri/icons/icon.icns`, `app_icon.icns`
- Next/web favicon: `frontend/src/app/favicon.ico`
- About/public icon: `frontend/public/icon_128x128.png`, `icon_32x32@2x.png`
- Sidebar collapsed logo: `frontend/public/logo-collapsed.png`
- Public brand sizes: `frontend/public/brand/clawscribe-icon-*`
- Microsoft Store/Windows logo aliases under `frontend/src-tauri/icons/`

The expanded sidebar currently renders the text label `ClawScribe`; the
collapsed sidebar and About dialog use the regenerated product icon.

Dependencies used: `ffmpeg`/`ffprobe`, Python 3 stdlib, and NumPy.
