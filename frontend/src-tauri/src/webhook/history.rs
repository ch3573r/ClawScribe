use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;

use super::{
    delivery::Outcome,
    payload::{utc, Event},
};

const HISTORY_ERROR: &str = "Could not update webhook delivery history.";

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Delivery {
    id: String,
    event: String,
    started_at: String,
    finished_at: Option<String>,
    attempts: i64,
    outcome: String,
    http_status: Option<i64>,
    error_class: Option<String>,
}

pub(super) async fn claim(
    pool: &SqlitePool,
    id: &str,
    event: Event,
    meeting: Option<&str>,
    completion: Option<&str>,
) -> Result<bool, String> {
    sqlx::query(
        "INSERT INTO webhook_deliveries (id, event, meeting_id, completion_key, started_at)
         SELECT ?, ?, ?, ?, ?
         WHERE ? IS NULL OR EXISTS (SELECT 1 FROM active_meetings WHERE id = ?)
         ON CONFLICT (event, meeting_id, completion_key) DO NOTHING",
    )
    .bind(id)
    .bind(event.name())
    .bind(meeting)
    .bind(completion)
    .bind(utc(Utc::now()))
    .bind(meeting)
    .bind(meeting)
    .execute(pool)
    .await
    .map(|result| result.rows_affected() == 1)
    .map_err(|_| HISTORY_ERROR.into())
}

pub(super) async fn finish(
    pool: &SqlitePool,
    id: &str,
    result: Outcome,
) -> Result<Delivery, String> {
    let outcome = if result.failure.is_some() {
        "failed"
    } else {
        "succeeded"
    };
    sqlx::query_as(
        "UPDATE webhook_deliveries
         SET finished_at = ?, attempts = ?, outcome = ?, http_status = ?, error_class = ?
         WHERE id = ?
         RETURNING id, event, started_at, finished_at, attempts, outcome, http_status, error_class",
    )
    .bind(utc(Utc::now()))
    .bind(i64::from(result.attempts))
    .bind(outcome)
    .bind(result.status.map(i64::from))
    .bind(result.failure.map(|failure| failure.class()))
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|_| HISTORY_ERROR.into())
}

pub(super) async fn latest(pool: &SqlitePool) -> Result<Option<Delivery>, String> {
    sqlx::query_as(
        "SELECT id, event, started_at, finished_at, attempts, outcome, http_status, error_class
         FROM webhook_deliveries ORDER BY started_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| HISTORY_ERROR.into())
}

pub(super) async fn for_meeting(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Vec<Delivery>, String> {
    sqlx::query_as(
        "SELECT id, event, started_at, finished_at, attempts, outcome, http_status, error_class
         FROM webhook_deliveries WHERE meeting_id = ?
         ORDER BY started_at DESC, rowid DESC LIMIT 100",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|_| "Could not load webhook delivery history.".into())
}
