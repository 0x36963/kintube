use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Returns the account for an already-verified, normalized email, creating it on first sign-in.
pub async fn find_or_create_by_email(
    db: impl PgExecutor<'_>,
    email: &str,
) -> Result<User, sqlx::Error> {
    // `do update` (instead of `do nothing`) so `returning` yields the row in both cases.
    sqlx::query_as::<_, User>(
        "insert into users (id, email, display_name) values ($1, $2, $3)
         on conflict (email) do update set email = excluded.email
         returning id, email, display_name, role, created_at, deleted_at",
    )
    .bind(Uuid::now_v7())
    .bind(email)
    .bind(default_display_name(email))
    .fetch_one(db)
    .await
}

fn default_display_name(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    local.chars().take(50).collect()
}
