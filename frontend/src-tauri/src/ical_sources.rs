//! Local iCalendar imports and read-only subscriptions, independent of Microsoft sign-in.
mod feed;
mod store;
#[cfg(test)]
mod tests;

use std::path::Path;
use store::CalendarSource;
pub(crate) use store::CalendarStore;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

async fn with_store<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&CalendarStore, &Path) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(move || {
        let directory = app
            .path()
            .app_data_dir()
            .map_err(|_| store::STORAGE)?
            .join("ical-sources");
        work(&app.state::<CalendarStore>(), &directory)
    })
    .await
    .map_err(|_| store::STORAGE)?
}

#[tauri::command]
pub(crate) async fn list_ical_sources(app: AppHandle) -> Result<Vec<CalendarSource>, String> {
    with_store(app, |store, dir| store.list(dir)).await
}

#[tauri::command]
pub(crate) async fn add_ical_file(
    app: AppHandle,
    name: String,
    content: String,
) -> Result<CalendarSource, String> {
    with_store(app, move |store, dir| store.add_file(dir, &name, content)).await
}

#[tauri::command]
pub(crate) async fn add_ical_subscription(
    app: AppHandle,
    name: String,
    url: String,
) -> Result<CalendarSource, String> {
    with_store(app, move |store, dir| {
        store.add_subscription(dir, &name, &url)
    })
    .await
}

#[tauri::command]
pub(crate) async fn read_ical_source(
    app: AppHandle,
    id: String,
    force: Option<bool>,
) -> Result<String, String> {
    let id = Uuid::parse_str(&id).map_err(|_| store::MISSING)?;
    with_store(app, move |store, dir| {
        store.read(dir, id, force.unwrap_or(false))
    })
    .await
}

#[tauri::command]
pub(crate) async fn remove_ical_source(app: AppHandle, id: String) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(|_| store::MISSING)?;
    with_store(app, move |store, dir| store.remove(dir, id)).await
}
