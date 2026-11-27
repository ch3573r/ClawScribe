//! Reference lifecycle tests that need the desktop database repositories.
//! They live outside `documents/`, which is also compiled as a standalone crate.
use super::documents::store::{
    self,
    tests::{assert_deleted_state, attach, pool, seed_dependent_turn},
};
use crate::database::repositories::meeting::MeetingsRepository;

const FIRST: &str = "00000000-0000-4000-8000-000000000001";
const SECOND: &str = "00000000-0000-4000-8000-000000000002";

#[tokio::test]
async fn trash_keeps_document_attached_even_if_a_stale_view_detaches_it() {
    let pool = pool().await;
    let attached = attach(&pool, "aster", FIRST).await;
    let document = attached.attachment.id;
    assert!(MeetingsRepository::move_to_trash(&pool, "aster", false)
        .await
        .unwrap());
    assert_eq!(
        store::detach(&pool, "aster", &document).await.unwrap(),
        None
    );
    assert!(MeetingsRepository::restore_from_trash(&pool, "aster")
        .await
        .unwrap());
    assert_eq!(
        store::get(&pool, "aster", &document).await.unwrap().id,
        document
    );
}

#[tokio::test]
async fn meeting_delete_reports_original_and_redacts_dependent_turns() {
    let pool = pool().await;
    let attached = attach(&pool, "aster", FIRST).await;
    seed_dependent_turn(&pool, &attached.attachment.id).await;
    let (deleted, originals) = MeetingsRepository::delete_meeting_with_documents(&pool, "aster")
        .await
        .unwrap();
    assert!(deleted);
    assert_eq!(originals, vec![format!("{}.docx", attached.attachment.id)]);
    assert_deleted_state(&pool, &attached.attachment.id).await;
}

#[tokio::test]
async fn shared_document_survives_meeting_delete() {
    let pool = pool().await;
    let first = attach(&pool, "aster", FIRST).await;
    attach(&pool, "birch", SECOND).await;
    let (deleted, originals) = MeetingsRepository::delete_meeting_with_documents(&pool, "aster")
        .await
        .unwrap();
    assert!(deleted);
    assert!(originals.is_empty());
    assert_eq!(
        store::list(&pool, "birch").await.unwrap()[0].id,
        first.attachment.id
    );
    assert_eq!(
        store::blocks(&pool, "birch", &first.attachment.id)
            .await
            .unwrap()
            .len(),
        1537
    );
    assert_eq!(
        store::detach(&pool, "aster", &first.attachment.id)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn failed_meeting_delete_rolls_back_document_deletion() {
    let pool = pool().await;
    let first = attach(&pool, "aster", FIRST).await;
    sqlx::query("CREATE TRIGGER reject_meeting_delete BEFORE DELETE ON meetings BEGIN SELECT RAISE(ABORT,'synthetic failure'); END")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        MeetingsRepository::delete_meeting_with_documents(&pool, "aster")
            .await
            .is_err()
    );
    assert_eq!(
        store::list(&pool, "aster").await.unwrap()[0].id,
        first.attachment.id
    );
    assert_eq!(
        store::blocks(&pool, "aster", &first.attachment.id)
            .await
            .unwrap()
            .len(),
        1537
    );
}
