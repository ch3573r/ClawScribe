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

    /// Fetch only the bounded history needed by the provider.
    pub async fn recent(pool: &SqlitePool, meeting_id: &str, limit: i64) -> Result<Vec<AiChatMessage>, SqlxError> {
        let mut messages = sqlx::query_as::<_, AiChatMessage>(
            "SELECT id, meeting_id, role, content, created_at FROM ai_chat_messages \
             WHERE meeting_id = ? ORDER BY created_at DESC, id DESC LIMIT ?"
        ).bind(meeting_id).bind(limit.clamp(1, 100)).fetch_all(pool).await?;
        messages.reverse();
        Ok(messages)
    }

    pub async fn get(pool: &SqlitePool, id: &str, meeting_id: &str) -> Result<Option<AiChatMessage>, SqlxError> {
        sqlx::query_as::<_, AiChatMessage>(
            "SELECT id, meeting_id, role, content, created_at FROM ai_chat_messages WHERE id = ? AND meeting_id = ?"
        ).bind(id).bind(meeting_id).fetch_optional(pool).await
    }

    pub async fn insert_question(pool: &SqlitePool, id: &str, meeting_id: &str, content: &str) -> Result<(), SqlxError> {
        sqlx::query("INSERT INTO ai_chat_messages (id, meeting_id, role, content, created_at) \
            VALUES (?, ?, 'user', ?, ?) ON CONFLICT(id) DO NOTHING")
            .bind(id).bind(meeting_id).bind(content).bind(Utc::now().to_rfc3339()).execute(pool).await?;
        let existing = Self::get(pool, id, meeting_id).await?.ok_or(SqlxError::RowNotFound)?;
        if existing.role != "user" || existing.content != content {
            return Err(SqlxError::Protocol("Chat request ID is already used for a different question".into()));
        }
        Ok(())
    }

    /// Never recreate a reply after its question/meeting has been removed.
    pub async fn insert_reply(pool: &SqlitePool, id: &str, question_id: &str, meeting_id: &str, content: &str) -> Result<AiChatMessage, SqlxError> {
        sqlx::query("INSERT INTO ai_chat_messages (id, meeting_id, role, content, created_at) \
            SELECT ?, ?, 'assistant', ?, ? WHERE EXISTS \
            (SELECT 1 FROM ai_chat_messages WHERE id = ? AND meeting_id = ? AND role = 'user') \
            ON CONFLICT(id) DO NOTHING")
            .bind(id).bind(meeting_id).bind(content).bind(Utc::now().to_rfc3339())
            .bind(question_id).bind(meeting_id).execute(pool).await?;
        Self::get(pool, id, meeting_id).await?.ok_or(SqlxError::RowNotFound)
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

#[cfg(test)]
mod tests {
    use super::*;
    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE ai_chat_messages (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL)").execute(&pool).await.unwrap();
        pool
    }
    #[tokio::test]
    async fn retry_is_idempotent_for_question_and_reply() {
        let pool = pool().await;
        AiChatRepository::insert_question(&pool, "chat-one", "meeting", "Question").await.unwrap();
        AiChatRepository::insert_question(&pool, "chat-one", "meeting", "Question").await.unwrap();
        let first = AiChatRepository::insert_reply(&pool, "reply-chat-one", "chat-one", "meeting", "Answer").await.unwrap();
        let retry = AiChatRepository::insert_reply(&pool, "reply-chat-one", "chat-one", "meeting", "Different provider retry").await.unwrap();
        assert_eq!(first.content, retry.content);
        assert_eq!(AiChatRepository::list(&pool, "meeting").await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn request_identity_cannot_cross_meetings_or_questions() {
        let pool = pool().await;
        AiChatRepository::insert_question(&pool, "chat-one", "meeting-a", "Question").await.unwrap();
        assert!(AiChatRepository::insert_question(&pool, "chat-one", "meeting-b", "Question").await.is_err());
        assert!(AiChatRepository::insert_question(&pool, "chat-one", "meeting-a", "Different question").await.is_err());
        assert!(AiChatRepository::insert_reply(&pool, "reply-chat-one", "chat-one", "meeting-b", "Answer").await.is_err());
        assert!(AiChatRepository::list(&pool, "meeting-b").await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn a_cleared_question_cannot_be_resurrected_by_a_late_reply() {
        let pool = pool().await;
        AiChatRepository::insert_question(&pool, "chat-one", "meeting", "Question").await.unwrap();
        AiChatRepository::clear(&pool, "meeting").await.unwrap();
        assert!(AiChatRepository::insert_reply(&pool, "reply-chat-one", "chat-one", "meeting", "Late answer").await.is_err());
        assert!(AiChatRepository::list(&pool, "meeting").await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn recent_history_is_bounded_and_chronological() {
        let pool = pool().await;
        for i in 0..120 {
            sqlx::query("INSERT INTO ai_chat_messages VALUES (?, 'meeting', 'user', ?, ?)")
                .bind(format!("chat-{i:03}")).bind(format!("Question {i}"))
                .bind(format!("2026-01-01T00:{:02}:{:02}Z", i / 60, i % 60))
                .execute(&pool).await.unwrap();
        }
        let recent = AiChatRepository::recent(&pool, "meeting", 20).await.unwrap();
        assert_eq!(recent.len(), 20);
        assert_eq!(recent[0].id, "chat-100");
        assert_eq!(recent[19].id, "chat-119");
        assert_eq!(AiChatRepository::recent(&pool, "meeting", i64::MAX).await.unwrap().len(), 100);
    }
}
