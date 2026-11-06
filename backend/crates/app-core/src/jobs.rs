//! Postgres-backed job queue.
//!
//! - Workers claim with `FOR UPDATE SKIP LOCKED`, so many can poll safely.
//! - **Fairness**: among ready jobs, the one whose owner has the fewest jobs running wins
//!   (then oldest first), so one user's 2,000-file import can't starve everyone else.
//! - **Leases**: a claimed job is locked until `locked_until`; the worker extends it with
//!   heartbeats. If a worker dies, the lease expires and another worker reclaims the job.
//! - **Retries**: failures are retried with backoff up to `max_attempts`, then the job is
//!   marked `failed` (dead letter) and kept for inspection.
//! - Enqueueing sends `NOTIFY jobs` so idle workers wake immediately.

use std::time::Duration;

use serde_json::Value;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

pub const NOTIFY_CHANNEL: &str = "jobs";

pub mod kind {
    pub const PROCESS_VIDEO: &str = "process_video";
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub owner_id: Option<Uuid>,
    pub payload: Value,
    /// Including the current one.
    pub attempts: i32,
    pub max_attempts: i32,
}

impl Job {
    pub fn is_final_attempt(&self) -> bool {
        self.attempts >= self.max_attempts
    }
}

pub async fn enqueue(
    db: impl PgExecutor<'_>,
    kind: &str,
    owner_id: Option<Uuid>,
    payload: Value,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    // Data-modifying CTE so the insert and the notification are one statement; the
    // notification is delivered when the surrounding transaction commits.
    sqlx::query(
        "with inserted as (
             insert into jobs (id, kind, owner_id, payload) values ($1, $2, $3, $4) returning kind
         )
         select pg_notify($5, kind) from inserted",
    )
    .bind(id)
    .bind(kind)
    .bind(owner_id)
    .bind(payload)
    .bind(NOTIFY_CHANNEL)
    .execute(db)
    .await?;
    Ok(id)
}

/// Claims the next ready job of one of `kinds`, or `None` if there is nothing to do.
pub async fn claim(
    db: &PgPool,
    worker_id: &str,
    kinds: &[&str],
    lease: Duration,
) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query_as::<_, Job>(
        "with candidate as (
             select j.id from jobs j
             where j.kind = any($2)
               and (
                     (j.status = 'queued' and j.run_at <= now())
                  or (j.status = 'running' and j.locked_until < now() and j.attempts < j.max_attempts)
               )
             order by
               (select count(*) from jobs r
                 where r.status = 'running' and r.locked_until >= now()
                   and r.owner_id is not distinct from j.owner_id),
               j.run_at, j.id
             limit 1
             for update of j skip locked
         )
         update jobs set
             status = 'running',
             attempts = jobs.attempts + 1,
             locked_by = $1,
             locked_until = now() + make_interval(secs => $3),
             updated_at = now()
         from candidate
         where jobs.id = candidate.id
         returning jobs.id, jobs.kind, jobs.owner_id, jobs.payload, jobs.attempts, jobs.max_attempts",
    )
    .bind(worker_id)
    .bind(kinds)
    .bind(lease.as_secs_f64())
    .fetch_optional(db)
    .await
}

/// Extends the lease. Returns `false` if this worker no longer owns the job.
pub async fn heartbeat(
    db: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    lease: Duration,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "update jobs set locked_until = now() + make_interval(secs => $3), updated_at = now()
         where id = $1 and locked_by = $2 and status = 'running'",
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease.as_secs_f64())
    .execute(db)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

pub async fn complete(db: &PgPool, job_id: Uuid, worker_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update jobs set status = 'done', locked_by = null, locked_until = null, updated_at = now()
         where id = $1 and locked_by = $2",
    )
    .bind(job_id)
    .bind(worker_id)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum FailOutcome {
    /// Will run again after a backoff.
    Retrying,
    /// Out of attempts (or not retryable): marked `failed`.
    Dead,
}

/// Records a failure, retrying with backoff (30 s, 2 min, 8 min, …) unless out of attempts.
pub async fn fail(
    db: &PgPool,
    job: &Job,
    worker_id: &str,
    error: &str,
    retryable: bool,
) -> Result<FailOutcome, sqlx::Error> {
    let retry = retryable && !job.is_final_attempt();
    let backoff_secs = 30.0 * 4f64.powi(job.attempts.max(1) - 1);
    sqlx::query(
        "update jobs set
             status = case when $3 then 'queued' else 'failed' end,
             run_at = case when $3 then now() + make_interval(secs => $4) else run_at end,
             last_error = $5,
             locked_by = null, locked_until = null, updated_at = now()
         where id = $1 and locked_by = $2",
    )
    .bind(job.id)
    .bind(worker_id)
    .bind(retry)
    .bind(backoff_secs)
    .bind(error.chars().take(4000).collect::<String>())
    .execute(db)
    .await?;
    Ok(if retry {
        FailOutcome::Retrying
    } else {
        FailOutcome::Dead
    })
}

/// Jobs whose worker died during their final attempt. Marks them failed and returns
/// them so the caller can clean up (e.g. mark the media item as failed).
pub async fn reap_abandoned(db: &PgPool) -> Result<Vec<Job>, sqlx::Error> {
    sqlx::query_as::<_, Job>(
        "update jobs set status = 'failed', last_error = 'worker stopped responding during the final attempt',
                         locked_by = null, locked_until = null, updated_at = now()
         where status = 'running' and locked_until < now() and attempts >= max_attempts
         returning id, kind, owner_id, payload, attempts, max_attempts",
    )
    .fetch_all(db)
    .await
}

/// Deletes finished jobs older than `age`. Failed jobs are kept longer for debugging.
pub async fn prune(db: &PgPool, age: Duration) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "delete from jobs
         where (status = 'done' and updated_at < now() - make_interval(secs => $1))
            or (status = 'failed' and updated_at < now() - make_interval(secs => $1 * 4))",
    )
    .bind(age.as_secs_f64())
    .execute(db)
    .await?
    .rows_affected())
}

/// Hands a job back to the queue without counting the attempt, e.g. when a worker shuts
/// down mid-job during a deploy.
pub async fn release(db: &PgPool, job: &Job, worker_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update jobs set status = 'queued', attempts = greatest(attempts - 1, 0), run_at = now(),
                         locked_by = null, locked_until = null, updated_at = now()
         where id = $1 and locked_by = $2 and status = 'running'",
    )
    .bind(job.id)
    .bind(worker_id)
    .execute(db)
    .await?;
    Ok(())
}
