# Notification checks

These Tauri commands remain registered in the desktop app. They test app-side
notification handling; confirm actual delivery in the installed Windows app.

```javascript
await invoke('initialize_notification_manager_manual');
await invoke('is_notification_system_ready');
await invoke('get_notification_settings');
await invoke('show_test_notification');
```

For an explicit consent-enabled test, `test_notification_with_auto_consent`
initializes the manager if needed, enables stored app consent, calls the app's
permission helper, and sends a test notification. This changes the app's consent
setting; it does not establish that the operating system will display a toast.

The underlying `request_permission` helper assumes native OS permission is
granted, and `get_system_dnd_status` returns `false` rather
than querying system Do Not Disturb. Check notification permissions and Do Not
Disturb in Windows when diagnosing missing notifications. A successful command
alone is not proof of visible delivery.

Implementation: `src/notifications/commands.rs`, `src/notifications/manager.rs`,
`src/notifications/system.rs`, and command registration in `src/lib.rs`.
