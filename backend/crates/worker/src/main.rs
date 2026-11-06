//! Background worker: processes uploaded videos and runs periodic maintenance.
//!
//! Runs separately from the API (PLAN.md §2) and can be scaled independently. Untrusted
//! files are parsed by ffmpeg here, so production workers run sandboxed (PLAN.md §6).

mod process_video;

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use app_core::{config::S3Config, db, jobs, media, storage::Storage};
use sqlx::{PgPool, postgres::PgListener};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

/// How long a claimed job stays locked without a heartbeat.
const LEASE: Duration = Duration::from_secs(120);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);
/// Poll interval when no notification arrives (notifications can be missed on reconnect).
const IDLE_POLL: Duration = Duration::from_secs(5);
const MAINTENANCE_EVERY: Duration = Duration::from_secs(600);

pub struct Ctx {
    pub db: PgPool,
    pub storage: Storage,
    pub worker_id: String,
    pub tmp_root: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,worker=debug,app_core=debug"));
    if std::env::var("LOG_FORMAT").as_deref() == Ok("json") {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let database_url = std::env::var("DATABASE_URL").context("missing DATABASE_URL")?;
    let concurrency: usize = std::env::var("WORKER_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let tmp_root = std::env::var("WORKER_TMP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("kintube-worker"));
    tokio::fs::create_dir_all(&tmp_root).await?;

    let ctx = Arc::new(Ctx {
        db: db::connect(&database_url)
            .await
            .context("connecting to database")?,
        storage: Storage::new(&S3Config::from_env()?),
        worker_id: format!("{}-{}", hostname(), std::process::id()),
        tmp_root,
    });
    tracing::info!(worker_id = %ctx.worker_id, concurrency, "worker started");

    let wake = Arc::new(Notify::new());
    let shutdown = CancellationToken::new();
    tokio::spawn(listen_for_jobs(database_url, wake.clone()));
    tokio::spawn(maintenance(ctx.clone(), shutdown.clone()));

    let mut loops = Vec::new();
    for slot in 0..concurrency {
        loops.push(tokio::spawn(job_loop(
            ctx.clone(),
            wake.clone(),
            shutdown.clone(),
            slot,
        )));
    }

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down: releasing running jobs back to the queue");
    shutdown.cancel();
    for handle in loops {
        let _ = handle.await;
    }
    Ok(())
}

async fn job_loop(ctx: Arc<Ctx>, wake: Arc<Notify>, shutdown: CancellationToken, slot: usize) {
    let worker_id = format!("{}#{slot}", ctx.worker_id);
    while !shutdown.is_cancelled() {
        match jobs::claim(&ctx.db, &worker_id, &[jobs::kind::PROCESS_VIDEO], LEASE).await {
            Ok(Some(job)) => run_job(&ctx, &worker_id, job, &shutdown).await,
            Ok(None) => {
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(IDLE_POLL) => {}
                    _ = shutdown.cancelled() => {}
                }
            }
            Err(err) => {
                tracing::error!(error = %err, "claiming jobs failed");
                tokio::time::sleep(IDLE_POLL).await;
            }
        }
    }
}

/// Outcome of a job handler.
pub enum JobError {
    /// Won't succeed on retry; `.0` is shown to the owner.
    Permanent(String),
    /// Might succeed on retry (network, storage, resources…).
    Retryable(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for JobError {
    fn from(err: E) -> Self {
        Self::Retryable(err.into())
    }
}

/// Shown when a video fails for reasons that aren't the file's fault, after all retries.
const GENERIC_FAILURE: &str = "Processing failed. Try uploading the video again.";

async fn run_job(ctx: &Ctx, worker_id: &str, job: jobs::Job, shutdown: &CancellationToken) {
    let span = tracing::info_span!("job", id = %job.id, kind = %job.kind, attempt = job.attempts);
    let _enter = span.enter();
    tracing::info!("started");
    let started = std::time::Instant::now();

    let handler = async {
        match job.kind.as_str() {
            jobs::kind::PROCESS_VIDEO => process_video::run(ctx, &job).await,
            other => Err(JobError::Permanent(format!("unknown job kind {other}"))),
        }
    };
    // The handler is dropped (killing ffmpeg) if we lose the lease or shut down.
    let heartbeat = async {
        loop {
            tokio::time::sleep(HEARTBEAT_EVERY).await;
            match jobs::heartbeat(&ctx.db, job.id, worker_id, LEASE).await {
                Ok(true) => {}
                Ok(false) => return "lost the job's lease",
                Err(err) => tracing::warn!(error = %err, "heartbeat failed"),
            }
        }
    };

    let result = tokio::select! {
        result = handler => Some(result),
        reason = heartbeat => {
            tracing::warn!(reason, "abandoning job");
            None
        }
        _ = shutdown.cancelled() => {
            if let Err(err) = jobs::release(&ctx.db, &job, worker_id).await {
                tracing::error!(error = %err, "could not release job");
            }
            None
        }
    };

    match result {
        None => {}
        Some(Ok(())) => {
            tracing::info!(elapsed_ms = started.elapsed().as_millis() as u64, "done");
            if let Err(err) = jobs::complete(&ctx.db, job.id, worker_id).await {
                tracing::error!(error = %err, "could not mark job complete");
            }
        }
        Some(Err(JobError::Permanent(reason))) => {
            tracing::info!(reason, "failed permanently");
            let _ = jobs::fail(&ctx.db, &job, worker_id, &reason, false).await;
            on_dead(ctx, &job, &reason).await;
        }
        Some(Err(JobError::Retryable(err))) => {
            tracing::error!(error = ?err, "failed");
            match jobs::fail(&ctx.db, &job, worker_id, &format!("{err:#}"), true).await {
                Ok(jobs::FailOutcome::Dead) => on_dead(ctx, &job, GENERIC_FAILURE).await,
                Ok(jobs::FailOutcome::Retrying) => {}
                Err(db_err) => tracing::error!(error = %db_err, "could not record failure"),
            }
        }
    }
}

/// A job is out of attempts: tell the owner.
async fn on_dead(ctx: &Ctx, job: &jobs::Job, reason: &str) {
    if job.kind == jobs::kind::PROCESS_VIDEO
        && let Some(media_id) = process_video::media_id(job)
        && let Err(err) = media::mark_failed(&ctx.db, media_id, reason).await
    {
        tracing::error!(error = %err, "could not mark media failed");
    }
}

async fn listen_for_jobs(database_url: String, wake: Arc<Notify>) {
    loop {
        let result = async {
            let mut listener = PgListener::connect(&database_url).await?;
            listener.listen(jobs::NOTIFY_CHANNEL).await?;
            loop {
                listener.recv().await?;
                wake.notify_waiters();
            }
            #[allow(unreachable_code)]
            Ok::<(), sqlx::Error>(())
        }
        .await;
        if let Err(err) = result {
            tracing::warn!(error = %err, "job listener disconnected; reconnecting");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn maintenance(ctx: Arc<Ctx>, shutdown: CancellationToken) {
    let mut tick = tokio::time::interval(MAINTENANCE_EVERY);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.cancelled() => return,
        }
        match jobs::reap_abandoned(&ctx.db).await {
            Ok(reaped) => {
                for job in reaped {
                    tracing::warn!(job_id = %job.id, "reaped job abandoned on its final attempt");
                    on_dead(&ctx, &job, GENERIC_FAILURE).await;
                }
            }
            Err(err) => tracing::error!(error = %err, "reaping jobs failed"),
        }
        match media::abort_stale_uploads(&ctx.db, &ctx.storage).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "aborted stale uploads"),
            Err(err) => tracing::error!(error = %err, "aborting stale uploads failed"),
        }
        match media::purge_expired_trash(&ctx.db, &ctx.storage).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "purged expired trash"),
            Err(err) => tracing::error!(error = %err, "purging trash failed"),
        }
        if let Err(err) = jobs::prune(&ctx.db, Duration::from_secs(7 * 24 * 3600)).await {
            tracing::error!(error = %err, "pruning jobs failed");
        }
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "worker".into())
}
