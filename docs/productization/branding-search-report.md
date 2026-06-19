# ClawScribe Branding Search Report

Date: 2026-06-15
Branch: `feat/clawscribe-productization-auth-theme-exports`
Worker: Branding/version/license subagent E

## Product Branding Applied

Updated product-visible metadata and UI to ClawScribe:

- `frontend/package.json`: version `0.5.0-alpha.2`
- `frontend/src-tauri/Cargo.toml`: version `0.5.0-alpha.2`
- `frontend/src-tauri/tauri.conf.json`: version `0.5.0-alpha.2`
- `frontend/src/components/About.tsx`: ClawScribe title, `0.5.0-alpha.2` fallback, upstream base line, and license-friendly fork disclaimer
- `frontend/src/components/Logo.tsx`, `Info.tsx`, `Sidebar/index.tsx`: ClawScribe visible name and version footer
- `frontend/src/components/onboarding/**`: ClawScribe onboarding copy
- `frontend/src/components/PermissionWarning.tsx`, `PreferenceSettings.tsx`, `TranscriptView.tsx`, `VirtualizedTranscriptView.tsx`: ClawScribe display strings
- `frontend/src/app/metadata.ts`, `frontend/src/app/metadata.tsx`: ClawScribe app metadata
- `frontend/src-tauri/src/tray.rs`: ClawScribe tray tooltip
- `frontend/src-tauri/src/notifications/commands.rs`, `notifications/types.rs`: ClawScribe notification titles
- `README.md`, `NOTICE.md`, `UPSTREAM.md`, `CHANGELOG.md`: productized docs and upstream attribution

About dialog acceptance target:

- Shows `ClawScribe`
- Shows product version `0.5.0-alpha.2` before runtime version loads
- Shows `Based on Meetily Community Edition 0.4.0`
- Includes a fork/license attribution disclaimer

## Intentional Remaining Meetily References

### Upstream Attribution / Legal

Keep these references:

- `LICENSE.md`: original MIT license copyright for Zackriya Solutions
- `NOTICE.md`, `UPSTREAM.md`, `README.md`, `CHANGELOG.md`: fork attribution and upstream base version
- `frontend/src-tauri/Cargo.toml`: upstream repository URL remains as provenance until a ClawScribe public repository URL exists
- `frontend/src/components/About.tsx`: upstream base line and copyright attribution

### Historical Productization Docs

Keep these as baseline/history, not live product metadata:

- `docs/productization/phase0-inventory.md`
- `docs/productization/coordinator-plan.md`
- `docs/clawscribe-product-direction.md`

### Previous Meetily Install Migration

Keep these visible Meetily references because they tell users what old data is being imported:

- `frontend/src/components/DatabaseImport/LegacyDatabaseImport.tsx`
- `frontend/src/components/DatabaseImport/HomebrewDatabaseDetector.tsx`
- `frontend/src/contexts/OnboardingContext.tsx`

### Compatibility Names

Keep these until a coordinated migration exists:

- `frontend/src-tauri/src/audio/recording_preferences.rs`: `meetily-recordings` default folders
- `frontend/src-tauri/src/openclaw.rs`: `openclaw.meetily-submission*.v1`, `meetily-json-v1`, and fallback meeting IDs
- `docs/openclaw-handoff.md`, `docs/windows-release.md`: `MEETILY_OPENCLAW_*` deployment variables
- `frontend/src-tauri/src/summary/summary_engine/sidecar.rs`: `MEETILY_LLAMA_HELPER`
- `frontend/src/lib/analytics.ts`: `meetily_user_id` analytics/session key
- `frontend/src/services/indexedDBService.ts`: `MeetilyRecoveryDB`
- `frontend/src-tauri/src/notifications/settings.rs`: `meetily` settings path
- `frontend/src-tauri/src/audio/decoder.rs`: `.meetily_decode_` temp prefix
- `frontend/src-tauri/src/audio/capture/core_audio.rs`: `meetily-audio-tap`

### Existing Model / Template Cache Paths

Keep these for now to avoid redownloading or orphaning user assets:

- `frontend/src-tauri/src/whisper_engine/whisper_engine.rs`
- `frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs`
- `frontend/src-tauri/src/summary/summary_engine/model_manager.rs`
- `frontend/src-tauri/src/summary/templates/loader.rs`
- `frontend/src-tauri/src/summary/templates/mod.rs`
- `frontend/src-tauri/templates/README.md`

Changing these should be paired with old-path discovery, migration/copy behavior, and tests.

### Upstream / Developer Documentation

Remaining docs such as `docs/BUILDING.md`, `docs/GPU_ACCELERATION.md`, `docs/building_in_linux.md`, `docs/architecture.md`, `frontend/README.md`, and `frontend/API.md` still contain upstream Meetily wording. These are not the primary product README, but should be refreshed in a later docs pass if they become customer-facing.

### Archived / Legacy Code

Legacy copied source files have since been removed from the tracked tree; avoid
reintroducing copied implementation files for historical branding context.

## Recording Folder Migration Recommendation

Do not rename the default `meetily-recordings` folder in this patch.

Recommended staged plan:

1. Add ClawScribe-branded default path selection for new installs only.
2. Detect existing `meetily-recordings` folders and keep them readable/importable.
3. Add UI copy that distinguishes the current recording folder from previous Meetily compatibility folders.
4. Keep OpenClaw ingest compatible with both old and new recording folder names.
5. Only then change the default folder name after Windows runtime validation.
