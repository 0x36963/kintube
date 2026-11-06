//! Job queue behaviour against a real Postgres (`#[sqlx::test]` gives each test a fresh DB).

use std::time::Duration;

use app_core::jobs::{self, FailOutcome};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

const LEASE: Duration = Duration::from_secs(60);
const KINDS: &[&str] = &["test"];

async fn user(db: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into users (id, email, display_name) values ($1, $2, 'u')")
        .bind(id)
        .bind(format!("{id}@example.com"))
        .execute(db)
        .await
        .unwrap();
    id
}

async fn enqueue(db: &PgPool, owner: Uuid, n: i32) -> Uuid {
    jobs::enqueue(db, "test", Some(owner), json!({ "n": n }))
        .await
        .unwrap()
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn claims_round_robin_across_users(db: PgPool) {
    let (alice, bob) = (user(&db).await, user(&db).await);
    // Alice queues a big batch first; Bob queues one job afterwards.
    for n in 0..5 {
        enqueue(&db, alice, n).await;
    }
    enqueue(&db, bob, 100).await;

    let first = jobs::claim(&db, "w1", KINDS, LEASE).await.unwrap().unwrap();
    assert_eq!(first.owner_id, Some(alice), "oldest job first");
    // Alice now has a job running, so Bob's job jumps ahead of her backlog.
    let second = jobs::claim(&db, "w2", KINDS, LEASE).await.unwrap().unwrap();
    assert_eq!(second.owner_id, Some(bob));
    let third = jobs::claim(&db, "w3", KINDS, LEASE).await.unwrap().unwrap();
    assert_eq!(third.owner_id, Some(alice));
    assert_eq!(third.payload["n"], 1, "a user's own jobs stay in order");
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn a_job_is_claimed_once(db: PgPool) {
    let owner = user(&db).await;
    enqueue(&db, owner, 0).await;

    let claims = futures::future::join_all((0..8).map(|i| {
        let db = db.clone();
        async move {
            jobs::claim(&db, &format!("w{i}"), KINDS, LEASE)
                .await
                .unwrap()
        }
    }))
    .await;
    assert_eq!(claims.iter().flatten().count(), 1);
    assert!(
        jobs::claim(&db, "late", &["other-kind"], LEASE)
            .await
            .unwrap()
            .is_none(),
        "kinds filter"
    );
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn expired_leases_are_reclaimed_and_heartbeats_prevent_it(db: PgPool) {
    let owner = user(&db).await;
    enqueue(&db, owner, 0).await;

    let job = jobs::claim(&db, "dead-worker", KINDS, Duration::from_millis(1))
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let reclaimed = jobs::claim(&db, "w2", KINDS, LEASE)
        .await
        .unwrap()
        .expect("lease expired");
    assert_eq!(reclaimed.id, job.id);
    assert_eq!(
        reclaimed.attempts, 2,
        "the crashed run counts as an attempt"
    );

    assert!(
        !jobs::heartbeat(&db, job.id, "dead-worker", LEASE)
            .await
            .unwrap(),
        "old owner lost it"
    );
    assert!(jobs::heartbeat(&db, job.id, "w2", LEASE).await.unwrap());
    assert!(
        jobs::claim(&db, "w3", KINDS, LEASE)
            .await
            .unwrap()
            .is_none(),
        "held by w2"
    );
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn failures_retry_with_backoff_then_die(db: PgPool) {
    let owner = user(&db).await;
    let id = enqueue(&db, owner, 0).await;

    let job = jobs::claim(&db, "w", KINDS, LEASE).await.unwrap().unwrap();
    assert_eq!(
        jobs::fail(&db, &job, "w", "boom", true).await.unwrap(),
        FailOutcome::Retrying
    );
    assert!(
        jobs::claim(&db, "w", KINDS, LEASE).await.unwrap().is_none(),
        "backing off"
    );

    // Skip the backoff, and fail the remaining attempts.
    for expected in [FailOutcome::Retrying, FailOutcome::Dead] {
        sqlx::query("update jobs set run_at = now() where id = $1")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
        let job = jobs::claim(&db, "w", KINDS, LEASE).await.unwrap().unwrap();
        assert_eq!(
            jobs::fail(&db, &job, "w", "boom", true).await.unwrap(),
            expected
        );
    }
    let (status, attempts, error): (String, i32, String) =
        sqlx::query_as("select status, attempts, last_error from jobs where id = $1")
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), attempts, error.as_str()),
        ("failed", 3, "boom")
    );
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn non_retryable_failures_die_immediately(db: PgPool) {
    let owner = user(&db).await;
    enqueue(&db, owner, 0).await;
    let job = jobs::claim(&db, "w", KINDS, LEASE).await.unwrap().unwrap();
    assert_eq!(
        jobs::fail(&db, &job, "w", "not a video", false)
            .await
            .unwrap(),
        FailOutcome::Dead
    );
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn workers_dying_on_the_final_attempt_are_reaped(db: PgPool) {
    let owner = user(&db).await;
    let id = enqueue(&db, owner, 0).await;
    sqlx::query("update jobs set max_attempts = 1 where id = $1")
        .bind(id)
        .execute(&db)
        .await
        .unwrap();

    jobs::claim(&db, "dead-worker", KINDS, Duration::from_millis(1))
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        jobs::claim(&db, "w2", KINDS, LEASE)
            .await
            .unwrap()
            .is_none(),
        "no attempts left"
    );

    let reaped = jobs::reap_abandoned(&db).await.unwrap();
    assert_eq!(reaped.iter().map(|j| j.id).collect::<Vec<_>>(), vec![id]);
    assert!(jobs::reap_abandoned(&db).await.unwrap().is_empty());
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn completed_jobs_are_pruned(db: PgPool) {
    let owner = user(&db).await;
    let id = enqueue(&db, owner, 0).await;
    let job = jobs::claim(&db, "w", KINDS, LEASE).await.unwrap().unwrap();
    jobs::complete(&db, job.id, "w").await.unwrap();
    assert_eq!(
        jobs::prune(&db, Duration::from_secs(3600)).await.unwrap(),
        0,
        "too recent"
    );
    sqlx::query("update jobs set updated_at = now() - interval '2 hours' where id = $1")
        .bind(id)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        jobs::prune(&db, Duration::from_secs(3600)).await.unwrap(),
        1
    );
}
