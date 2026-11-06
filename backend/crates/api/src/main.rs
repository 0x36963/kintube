use std::net::SocketAddr;

use anyhow::{Context, Result, bail};
use app_core::{Config, Mailer, db};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: api [serve | migrate | storage-setup | openapi]";

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let command = std::env::args().nth(1).unwrap_or_else(|| "serve".into());

    // Needs no config or database, and must keep stdout clean for redirection.
    if command == "openapi" {
        println!("{}", api::openapi().to_pretty_json()?);
        return Ok(());
    }

    init_tracing();
    let config = Config::from_env()?;
    match command.as_str() {
        "serve" => serve(config).await,
        "migrate" => {
            let db = db::connect(&config.database_url)
                .await
                .context("connecting to database")?;
            db::MIGRATOR.run(&db).await.context("running migrations")?;
            tracing::info!("migrations applied");
            Ok(())
        }
        "storage-setup" => {
            // Bucket, CORS for browser uploads, and the abandoned-upload lifecycle rule.
            let storage = app_core::storage::Storage::new(&config.s3);
            storage
                .setup_bucket(std::slice::from_ref(&config.app_origin))
                .await?;
            tracing::info!(bucket = storage.bucket(), "storage configured");
            Ok(())
        }
        other => bail!("unknown command {other:?}\n{USAGE}"),
    }
}

async fn serve(config: Config) -> Result<()> {
    let db = db::connect(&config.database_url)
        .await
        .context("connecting to database")?;
    let mailer = Mailer::smtp(&config.smtp_url, &config.email_from)?;
    let bind_addr = config.bind_addr;

    let state = api::AppState::new(db, mailer, config);
    state.spawn_background();

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!("listening on http://{bind_addr}");
    axum::serve(
        listener,
        api::app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    })
    .await?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,api=debug,app_core=debug"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if std::env::var("LOG_FORMAT").as_deref() == Ok("json") {
        builder.json().init();
    } else {
        builder.init();
    }
}
