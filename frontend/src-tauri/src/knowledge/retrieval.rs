//! Scoped keyword and local semantic retrieval.
use super::types::*;
use sqlx::SqlitePool;

pub const CANDIDATES: usize = 64;
pub const VECTOR_PAGE: usize = 512;
pub const RESULT_LIMIT: usize = 12;
pub const RRF_CONSTANT: f64 = 60.;

#[derive(Debug, Clone)]
pub struct FrozenScope {
    pub scope: KnowledgeScope,
    pub meeting_ids: Vec<String>,
}
pub async fn freeze_scope(
    _pool: &SqlitePool,
    scope: &KnowledgeScope,
) -> Result<FrozenScope, KnowledgeError> {
    Ok(FrozenScope {
        scope: scope.clone(),
        meeting_ids: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn fixture() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        for (id, date) in [
            ("one", "2026-01-01"),
            ("two", "2026-09-01"),
            ("three", "2026-10-01"),
        ] {
            sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?, 'Public fixture', ?, ?)").bind(id).bind(date).bind(date).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO meeting_tags(meeting_id,tag) VALUES ('one','ÄPFEL'),('two','Äpfel'),('two','Beta')").execute(&pool).await.unwrap();
        pool
    }
    #[tokio::test]
    async fn tag_scope_applies_before_ranking() {
        let pool = fixture().await;
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                tags: vec!["äpfel".into(), "beta".into()],
                tag_mode: TagMatch::All,
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["two"]
        );
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                tags: vec!["äpfel".into()],
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["one", "two"]
        );
    }
    #[tokio::test]
    async fn empty_scope_never_broadens_and_untagged_has_precedence() {
        let pool = fixture().await;
        assert!(freeze_scope(
            &pool,
            &KnowledgeScope::Library {
                filter: MeetingFilter::default()
            }
        )
        .await
        .is_err());
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                tags: vec!["äpfel".into()],
                untagged: true,
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["three"]
        );
    }
    #[tokio::test]
    async fn selected_ids_and_dates_intersect() {
        let pool = fixture().await;
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                meeting_ids: vec!["one".into(), "two".into()],
                from: Some("2026-06-01".into()),
                to: Some("2026-09-30".into()),
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["two"]
        );
        assert!(freeze_scope(
            &pool,
            &KnowledgeScope::Live {
                session_id: "live".into()
            }
        )
        .await
        .is_err());
    }
}
