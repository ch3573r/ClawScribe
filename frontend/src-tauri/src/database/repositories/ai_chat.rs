use crate::database::models::{AiChatMessage, Transcript};
use chrono::Utc;
use sqlx::{Error as SqlxError, SqlitePool};
use uuid::Uuid;

/// Persistence for the transcript-grounded meeting chat.
pub struct AiChatRepository;

impl AiChatRepository {
    /// All chat turns for a meeting, oldest first.
    pub async fn list(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<AiChatMessage>, SqlxError> {
        sqlx::query_as::<_, AiChatMessage>(
            "SELECT id, meeting_id, role, content, created_at \
             FROM ai_chat_messages WHERE meeting_id = ? ORDER BY created_at ASC, id ASC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
    }

    /// Append one turn and return the stored row.
    pub async fn insert(
        pool: &SqlitePool,
        meeting_id: &str,
        role: &str,
        content: &str,
    ) -> Result<AiChatMessage, SqlxError> {
        let msg = AiChatMessage {
            id: format!("chat-{}", Uuid::new_v4()),
            meeting_id: meeting_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: Utc::now().to_rfc3339(),
        };
        sqlx::query(
            "INSERT INTO ai_chat_messages (id, meeting_id, role, content, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&msg.id)
        .bind(&msg.meeting_id)
        .bind(&msg.role)
        .bind(&msg.content)
        .bind(&msg.created_at)
        .execute(pool)
        .await?;
        Ok(msg)
    }

    /// Drop a meeting's entire chat history.
    pub async fn clear(pool: &SqlitePool, meeting_id: &str) -> Result<u64, SqlxError> {
        let res = sqlx::query("DELETE FROM ai_chat_messages WHERE meeting_id = ?")
            .bind(meeting_id)
            .execute(pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// The meeting's transcript segments, oldest first, for grounding the chat.
    /// Speaker labels are included so the model knows who said what — the same
    /// who-said-what context the summary path currently drops.
    pub async fn transcripts(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<Transcript>, SqlxError> {
        sqlx::query_as::<_, Transcript>(
            "SELECT * FROM transcripts WHERE meeting_id = ? ORDER BY timestamp ASC, id ASC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
    }
}
