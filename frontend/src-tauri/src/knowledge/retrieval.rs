//! Scoped keyword and local semantic retrieval.
use super::types::*;
use sqlx::{Row, SqlitePool};

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
    pool: &SqlitePool,
    scope: &KnowledgeScope,
) -> Result<FrozenScope, KnowledgeError> {
    let filter = match scope {
        KnowledgeScope::Meeting { meeting_id } => MeetingFilter {
            meeting_ids: vec![meeting_id.clone()],
            ..Default::default()
        },
        KnowledgeScope::Library { filter } => filter.clone(),
        KnowledgeScope::Live { .. } => return Err(KnowledgeError::InvalidInput),
    };
    if !filter.all_meetings
        && filter.meeting_ids.is_empty()
        && filter.tags.is_empty()
        && !filter.untagged
        && filter.from.is_none()
        && filter.to.is_none()
    {
        return Err(KnowledgeError::InvalidInput);
    }
    if filter.meeting_ids.iter().any(|id| id.trim().is_empty())
        || filter.tags.iter().any(|tag| tag.trim().is_empty())
        || filter.from.as_ref().is_some_and(|v| v.len() < 10)
        || filter.to.as_ref().is_some_and(|v| v.len() < 10)
    {
        return Err(KnowledgeError::InvalidInput);
    }
    let mut ids = std::collections::BTreeMap::<String, (String, Vec<String>)>::new();
    let rows=sqlx::query("SELECT m.id,m.created_at,t.tag FROM meetings m LEFT JOIN meeting_tags t ON t.meeting_id=m.id ORDER BY m.id").fetch_all(pool).await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let date: String = row.try_get("created_at")?;
        let entry = ids.entry(id).or_insert((date, Vec::new()));
        if let Some(tag) = row.try_get::<Option<String>, _>("tag")? {
            entry.1.push(tag.to_lowercase());
        }
    }
    let meeting_ids = ids
        .into_iter()
        .filter(|(id, (date, tags))| {
            (filter.meeting_ids.is_empty() || filter.meeting_ids.contains(id))
                && filter
                    .from
                    .as_ref()
                    .is_none_or(|from| date.get(..10).unwrap_or(date) >= from.as_str())
                && filter
                    .to
                    .as_ref()
                    .is_none_or(|to| date.get(..10).unwrap_or(date) <= to.as_str())
                && if filter.untagged {
                    tags.is_empty()
                } else if filter.tags.is_empty() {
                    true
                } else {
                    match filter.tag_mode {
                        TagMatch::Any => filter
                            .tags
                            .iter()
                            .any(|tag| tags.contains(&tag.to_lowercase())),
                        TagMatch::All => filter
                            .tags
                            .iter()
                            .all(|tag| tags.contains(&tag.to_lowercase())),
                    }
                }
        })
        .map(|(id, _)| id)
        .collect();
    Ok(FrozenScope {
        scope: scope.clone(),
        meeting_ids,
    })
}

pub async fn recheck_scope(pool: &SqlitePool, frozen: &FrozenScope) -> Result<(), KnowledgeError> {
    let current = freeze_scope(pool, &frozen.scope).await?;
    if frozen
        .meeting_ids
        .iter()
        .any(|id| !current.meeting_ids.contains(id))
    {
        return Err(KnowledgeError::Superseded);
    }
    Ok(())
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
