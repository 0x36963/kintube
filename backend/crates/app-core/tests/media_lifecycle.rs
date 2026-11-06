//! Upload → processing → trash → purge, with real Postgres and the dev S3 server.
//! Focus: quota accounting is exact at every step.

use std::time::Duration;

use app_core::{
    config::S3Config,
    media::{self, MediaError, NewUpload},
    storage::{Storage, original_key},
};
use sqlx::PgPool;
use uuid::Uuid;

const MIB: i64 = 1024 * 1024;

fn storage() -> Storage {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
    let endpoint = env("S3_ENDPOINT", "http://localhost:9000");
    Storage::new(&S3Config {
        public_endpoint: endpoint.clone(),
        endpoint,
        region: env("S3_REGION", "us-east-1"),
        bucket: env("S3_TEST_BUCKET", "kintube-test"),
        access_key_id: env("S3_ACCESS_KEY_ID", "kintube"),
        secret_access_key: env("S3_SECRET_ACCESS_KEY", "kintube-dev-secret"),
        force_path_style: true,
    })
}

async fn setup(db: &PgPool, limit: i64) -> (Storage, Uuid) {
    let storage = storage();
    storage
        .setup_bucket(&["http://localhost:5173".into()])
        .await
        .unwrap();
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into users (id, email, display_name, storage_limit_bytes) values ($1, $2, 'u', $3)",
    )
    .bind(id)
    .bind(format!("{id}@example.com"))
    .bind(limit)
    .execute(db)
    .await
    .unwrap();
    (storage, id)
}

async fn quota(db: &PgPool, user: Uuid) -> (i64, i64) {
    let u = media::usage(db, user).await.unwrap();
    (u.storage_used_bytes, u.storage_reserved_bytes)
}

fn upload(owner_id: Uuid, bytes: i64) -> NewUpload<'static> {
    NewUpload {
        owner_id,
        filename: "Beach day.mov",
        mime: "video/quicktime",
        bytes,
        part_size: 5 * MIB,
    }
}

/// Uploads every part via presigned URLs, like the browser does.
async fn upload_parts(
    db: &PgPool,
    storage: &Storage,
    owner: Uuid,
    media: &media::Media,
    skip: &[i32],
) {
    let numbers: Vec<i32> = (1..=media.part_count() as i32)
        .filter(|n| !skip.contains(n))
        .collect();
    let urls = media::presign_parts(db, storage, owner, media.id, &numbers)
        .await
        .unwrap();
    let part_size = media.part_size.unwrap();
    let http = reqwest::Client::new();
    for (n, url) in urls {
        let len = (media.bytes - (n as i64 - 1) * part_size).min(part_size);
        let res = http
            .put(url)
            .body(vec![n as u8; len as usize])
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success());
    }
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn upload_reserves_then_commits_quota_and_queues_processing(db: PgPool) {
    let (storage, owner) = setup(&db, 100 * MIB).await;
    let bytes = 12 * MIB + 7;
    let m = media::begin_upload(&db, &storage, upload(owner, bytes))
        .await
        .unwrap();
    assert_eq!(
        (m.status.as_str(), m.title.as_str(), m.part_count()),
        ("uploading", "Beach day", 3)
    );
    assert_eq!(quota(&db, owner).await, (0, bytes));

    // Resume support: the server reports exactly which parts are stored.
    upload_parts(&db, &storage, owner, &m, &[2]).await;
    let (_, parts) = media::upload_progress(&db, &storage, owner, m.id)
        .await
        .unwrap();
    assert_eq!(
        parts.iter().map(|p| p.number).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert!(matches!(
        media::complete_upload(&db, &storage, owner, m.id).await,
        Err(MediaError::UploadIncomplete)
    ));

    upload_parts(&db, &storage, owner, &m, &[1, 3]).await;
    let done = media::complete_upload(&db, &storage, owner, m.id)
        .await
        .unwrap();
    assert_eq!(done.status, "processing");
    assert_eq!(quota(&db, owner).await, (bytes, 0));
    let again = media::complete_upload(&db, &storage, owner, m.id)
        .await
        .unwrap();
    assert_eq!(again.status, "processing", "complete is idempotent");
    assert_eq!(
        quota(&db, owner).await,
        (bytes, 0),
        "and doesn't double-count"
    );

    let object = storage
        .get_object(&original_key(m.id), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(object.content_length, Some(bytes));
    let jobs: Vec<(String, serde_json::Value)> = sqlx::query_as("select kind, payload from jobs")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].0, "process_video");
    assert_eq!(jobs[0].1["media_id"], m.id.to_string());
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn uploads_beyond_quota_are_refused(db: PgPool) {
    let (storage, owner) = setup(&db, 20 * MIB).await;
    media::begin_upload(&db, &storage, upload(owner, 15 * MIB))
        .await
        .unwrap();
    // Reserved bytes count against the quota too.
    let err = media::begin_upload(&db, &storage, upload(owner, 6 * MIB))
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::StorageQuotaExceeded), "{err:?}");
    media::begin_upload(&db, &storage, upload(owner, 5 * MIB))
        .await
        .expect("exactly fills the quota");
    assert_eq!(quota(&db, owner).await, (0, 20 * MIB));
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn rejected_files_reserve_nothing(db: PgPool) {
    let (storage, owner) = setup(&db, 100 * MIB).await;
    for (filename, mime, bytes) in [
        ("a.jpg", "image/jpeg", MIB),
        ("a.mp4", "video/mp4", 0),
        ("a.mp4", "video/mp4", media::MAX_FILE_BYTES + 1),
    ] {
        let res = media::begin_upload(
            &db,
            &storage,
            NewUpload {
                owner_id: owner,
                filename,
                mime,
                bytes,
                part_size: 5 * MIB,
            },
        )
        .await;
        assert!(res.is_err(), "{filename} {bytes}");
    }
    assert_eq!(quota(&db, owner).await, (0, 0));
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn aborting_releases_the_reservation(db: PgPool) {
    let (storage, owner) = setup(&db, 100 * MIB).await;
    let m = media::begin_upload(&db, &storage, upload(owner, 6 * MIB))
        .await
        .unwrap();
    upload_parts(&db, &storage, owner, &m, &[2]).await;
    media::abort_upload(&db, &storage, owner, m.id)
        .await
        .unwrap();
    assert_eq!(quota(&db, owner).await, (0, 0));
    assert!(matches!(
        media::get_owned(&db, owner, m.id).await,
        Err(MediaError::NotFound)
    ));
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn other_users_cannot_touch_an_upload(db: PgPool) {
    let (storage, owner) = setup(&db, 100 * MIB).await;
    let (_, stranger) = setup(&db, 100 * MIB).await;
    let m = media::begin_upload(&db, &storage, upload(owner, MIB))
        .await
        .unwrap();

    assert!(matches!(
        media::presign_parts(&db, &storage, stranger, m.id, &[1]).await,
        Err(MediaError::NotFound)
    ));
    assert!(matches!(
        media::complete_upload(&db, &storage, stranger, m.id).await,
        Err(MediaError::NotFound)
    ));
    assert!(matches!(
        media::abort_upload(&db, &storage, stranger, m.id).await,
        Err(MediaError::NotFound)
    ));
    assert!(matches!(
        media::trash(&db, stranger, m.id).await,
        Err(MediaError::NotFound)
    ));
    assert_eq!(quota(&db, owner).await, (0, MIB), "nothing changed");
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn trash_restore_and_purge(db: PgPool) {
    let (storage, owner) = setup(&db, 100 * MIB).await;
    let m = media::begin_upload(&db, &storage, upload(owner, MIB))
        .await
        .unwrap();
    upload_parts(&db, &storage, owner, &m, &[]).await;
    media::complete_upload(&db, &storage, owner, m.id)
        .await
        .unwrap();

    assert!(
        matches!(
            media::purge(&db, &storage, owner, m.id).await,
            Err(MediaError::InvalidState(_))
        ),
        "must be trashed first"
    );
    media::trash(&db, owner, m.id).await.unwrap();
    assert!(
        media::list_library(&db, owner, None, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(media::list_trash(&db, owner).await.unwrap().len(), 1);
    media::restore(&db, owner, m.id).await.unwrap();
    assert_eq!(
        media::list_library(&db, owner, None, 50)
            .await
            .unwrap()
            .len(),
        1
    );

    media::trash(&db, owner, m.id).await.unwrap();
    media::purge(&db, &storage, owner, m.id).await.unwrap();
    assert_eq!(quota(&db, owner).await, (0, 0), "purging frees the space");
    assert!(
        storage
            .get_object(&original_key(m.id), None)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn sweeps_clean_up_stale_uploads_and_old_trash(db: PgPool) {
    let (storage, owner) = setup(&db, 100 * MIB).await;
    let stale = media::begin_upload(&db, &storage, upload(owner, MIB))
        .await
        .unwrap();
    let fresh = media::begin_upload(&db, &storage, upload(owner, MIB))
        .await
        .unwrap();
    let old = media::begin_upload(&db, &storage, upload(owner, 2 * MIB))
        .await
        .unwrap();
    upload_parts(&db, &storage, owner, &old, &[]).await;
    media::complete_upload(&db, &storage, owner, old.id)
        .await
        .unwrap();
    media::trash(&db, owner, old.id).await.unwrap();

    let day = Duration::from_secs(86_400).as_secs_f64();
    sqlx::query("update media set created_at = now() - make_interval(secs => $2) where id = $1")
        .bind(stale.id)
        .bind(8.0 * day)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("update media set trashed_at = now() - make_interval(secs => $2) where id = $1")
        .bind(old.id)
        .bind(31.0 * day)
        .execute(&db)
        .await
        .unwrap();

    assert_eq!(media::abort_stale_uploads(&db, &storage).await.unwrap(), 1);
    assert_eq!(media::purge_expired_trash(&db, &storage).await.unwrap(), 1);
    assert_eq!(
        quota(&db, owner).await,
        (0, MIB),
        "only the fresh upload remains reserved"
    );
    assert!(media::get_owned(&db, owner, fresh.id).await.is_ok());
}
