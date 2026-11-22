//! Opt-in delivery of committed meeting events to a user-configured receiver.
mod delivery;
mod history;
mod meetings;
mod payload;
mod settings;
#[cfg(test)]
mod tests;

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::watch;
use uuid::Uuid;

use self::{
    delivery::{Failure, Outcome},
    history::Delivery,
    payload::Event,
};
use crate::state::AppState;

const UPDATED_EVENT: &str = "webhook-updated";
const CONFIG_CHANGED_EVENT: &str = "webhook-settings-changed";
const SETTINGS_UNAVAILABLE: &str = "Webhook settings are unavailable.";
const DISABLED: &str = "Enable and configure the webhook in Settings first.";

struct DeliveryJob {
    event: Event,
    meeting: Option<String>,
    completion: Option<String>,
    generated_markdown: Option<String>,
}

impl DeliveryJob {
    fn automatic(&self) -> bool {
        self.completion.is_some()
    }
}

pub(crate) fn recording_saved<R: Runtime>(app: &AppHandle<R>, meeting: &str) {
    enqueue(
        app,
        DeliveryJob {
            event: Event::RecordingSaved,
            meeting: Some(meeting.into()),
            completion: Some("saved".into()),
            generated_markdown: None,
        },
    );
}

pub(crate) fn summary_ready<R: Runtime>(
    app: &AppHandle<R>,
    meeting: &str,
    generation: Uuid,
    mut result: serde_json::Value,
) {
    let generated_markdown = match result.get_mut("markdown") {
        Some(serde_json::Value::String(markdown)) => Some(std::mem::take(markdown)),
        _ => None,
    };
    enqueue(
        app,
        DeliveryJob {
            event: Event::SummaryReady,
            meeting: Some(meeting.into()),
            completion: Some(generation.to_string()),
            generated_markdown,
        },
    );
}

fn enqueue<R: Runtime>(app: &AppHandle<R>, send: DeliveryJob) {
    let changed = settings::CHANGED.subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if execute(&app, state.db_manager.pool(), send, None, changed)
            .await
            .is_err()
        {
            log::warn!("Could not prepare or record an automatic webhook delivery");
        }
    });
}

async fn execute<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    send: DeliveryJob,
    revision: Option<Uuid>,
    mut changed: watch::Receiver<()>,
) -> Result<Option<Delivery>, String> {
    let read_app = app.clone();
    let automatic = send.automatic();
    let event = send.event;
    let route =
        tokio::task::spawn_blocking(move || settings::route(&read_app, event, automatic, revision))
            .await
            .map_err(|_| SETTINGS_UNAVAILABLE)??;
    if changed.has_changed().unwrap_or(true) {
        return Ok(None);
    }
    let Some(route) = route else { return Ok(None) };
    let id = Uuid::new_v4().to_string();
    if !history::claim(
        pool,
        &id,
        event,
        send.meeting.as_deref(),
        send.completion.as_deref(),
    )
    .await?
    {
        return Ok(None);
    }
    let _ = app.emit(UPDATED_EVENT, ());
    let outcome = match prepare(pool, send, route, id.clone()).await {
        Ok(request) => delivery::deliver(&delivery::Http, &request, &mut changed).await,
        Err(failure) => Outcome::rejected(failure),
    };
    let delivery = history::finish(pool, &id, outcome).await?;
    let _ = app.emit(UPDATED_EVENT, ());
    Ok(Some(delivery))
}

async fn prepare(
    pool: &SqlitePool,
    send: DeliveryJob,
    route: settings::Route,
    id: String,
) -> Result<delivery::Request, Failure> {
    let credential = route
        .credentials
        .ok()
        .flatten()
        .ok_or(Failure::CredentialsUnavailable)?;
    let switches = route.switches;
    let url = delivery::destination(&credential.url, switches.allow_unencrypted)
        .map_err(|_| Failure::Destination)?;
    let content = match send.meeting {
        Some(meeting) => meetings::load(pool, meeting, send.generated_markdown, switches).await?,
        None => payload::sample(),
    };
    tokio::task::spawn_blocking(move || {
        let now = Utc::now();
        let body = payload::build(
            send.event,
            &id,
            now,
            content,
            switches.include_summary,
            switches.include_transcript,
        )
        .ok_or(Failure::PayloadTooLarge)?;
        let timestamp = now.timestamp().to_string();
        let signature = credential
            .signing_secret
            .as_deref()
            .map(|key| delivery::signature(key.as_bytes(), &timestamp, &body));
        Ok(delivery::Request {
            url,
            allow_unencrypted: switches.allow_unencrypted,
            event: send.event,
            id,
            timestamp,
            signature,
            body: body.into(),
        })
    })
    .await
    .map_err(|_| Failure::PayloadTooLarge)?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsView {
    #[serde(flatten)]
    config: settings::Status,
    last_delivery: Option<Delivery>,
}

#[tauri::command]
pub(crate) async fn get_webhook_settings<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<SettingsView, String> {
    let config = tokio::task::spawn_blocking(move || settings::read(&app))
        .await
        .map_err(|_| SETTINGS_UNAVAILABLE)??;
    Ok(SettingsView {
        config,
        last_delivery: history::latest(state.db_manager.pool()).await?,
    })
}

#[tauri::command]
pub(crate) async fn save_webhook_settings<R: Runtime>(
    app: AppHandle<R>,
    changes: settings::Changes,
) -> Result<settings::Status, String> {
    let edit_app = app.clone();
    let config = tokio::task::spawn_blocking(move || settings::save(&edit_app, changes))
        .await
        .map_err(|_| SETTINGS_UNAVAILABLE)??;
    let _ = app.emit(CONFIG_CHANGED_EVENT, ());
    Ok(config)
}

#[tauri::command]
pub(crate) async fn remove_webhook_settings<R: Runtime>(
    app: AppHandle<R>,
    revision: Option<Uuid>,
) -> Result<settings::Status, String> {
    let edit_app = app.clone();
    let config = tokio::task::spawn_blocking(move || settings::remove(&edit_app, revision))
        .await
        .map_err(|_| SETTINGS_UNAVAILABLE)??;
    let _ = app.emit(CONFIG_CHANGED_EVENT, ());
    Ok(config)
}

#[tauri::command]
pub(crate) async fn send_webhook_test<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    revision: Option<Uuid>,
) -> Result<Delivery, String> {
    let send = DeliveryJob {
        event: Event::Test,
        meeting: None,
        completion: None,
        generated_markdown: None,
    };
    execute(
        &app,
        state.db_manager.pool(),
        send,
        revision,
        settings::CHANGED.subscribe(),
    )
    .await?
    .ok_or_else(|| DISABLED.into())
}

#[tauri::command]
pub(crate) async fn send_meeting_webhook<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    meeting_id: String,
    revision: Option<Uuid>,
) -> Result<Delivery, String> {
    let send = DeliveryJob {
        event: Event::SummaryReady,
        meeting: Some(meeting_id),
        completion: None,
        generated_markdown: None,
    };
    execute(
        &app,
        state.db_manager.pool(),
        send,
        revision,
        settings::CHANGED.subscribe(),
    )
    .await?
    .ok_or_else(|| "The meeting or enabled webhook is no longer available.".into())
}
