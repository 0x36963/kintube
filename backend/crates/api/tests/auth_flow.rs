//! End-to-end tests of sign-in, sessions and request guards against a real Postgres.
//! Each test gets a fresh, migrated database from `#[sqlx::test]` (needs `DATABASE_URL`).

mod common;

use axum::http::{Method, StatusCode, header};
use common::*;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn code_sign_in_creates_account_and_session(db: PgPool) {
    let app = TestApp::new(db);
    let (_, mail) = app.start_login("  Ada@Example.com ").await;
    assert_eq!(mail.to, "ada@example.com");
    assert!(mail.text.contains(&format!("{ORIGIN}/auth/verify#token=")));

    let cookie = app.sign_in("ada@example.com").await;
    let me = app.send(Method::GET, "/api/me", None, Some(&cookie)).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["email"], "ada@example.com");
    assert_eq!(me.body["display_name"], "ada");
    assert_eq!(me.body["role"], "user");
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn same_email_signs_into_same_account(db: PgPool) {
    let app = TestApp::new(db);
    let first = app.sign_in("ada@example.com").await;
    let second = app.sign_in("ADA@example.com").await;
    let a = app.send(Method::GET, "/api/me", None, Some(&first)).await;
    let b = app.send(Method::GET, "/api/me", None, Some(&second)).await;
    assert_eq!(a.body["id"], b.body["id"]);
    assert_ne!(first, second, "each sign-in gets its own session");
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn magic_link_works_once(db: PgPool) {
    let app = TestApp::new(db);
    let (login_id, mail) = app.start_login("ada@example.com").await;
    let body = json!({ "token": token_from(&mail) });

    let first = app
        .send(
            Method::POST,
            "/api/auth/email/verify-link",
            Some(body.clone()),
            None,
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    session_cookie(&first.headers);

    let replay = app
        .send(
            Method::POST,
            "/api/auth/email/verify-link",
            Some(body),
            None,
        )
        .await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert_eq!(replay.body["error"]["code"], "login_invalid");

    // Using the link also consumes the code from the same email.
    let code = json!({ "login_id": login_id, "code": code_from(&mail) });
    let res = app
        .send(
            Method::POST,
            "/api/auth/email/verify-code",
            Some(code),
            None,
        )
        .await;
    assert_eq!(res.body["error"]["code"], "login_invalid");
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn code_guesses_are_limited(db: PgPool) {
    let app = TestApp::new(db);
    let (login_id, mail) = app.start_login("ada@example.com").await;
    let real = code_from(&mail);
    let wrong = if real == "000000" { "111111" } else { "000000" };

    for _ in 0..app_core::auth::MAX_CODE_ATTEMPTS {
        let res = app
            .send(
                Method::POST,
                "/api/auth/email/verify-code",
                Some(json!({ "login_id": login_id, "code": wrong })),
                None,
            )
            .await;
        assert_eq!(res.body["error"]["code"], "incorrect_code");
    }
    // Attempts exhausted: even the right code is refused now.
    let res = app
        .send(
            Method::POST,
            "/api/auth/email/verify-code",
            Some(json!({ "login_id": login_id, "code": real })),
            None,
        )
        .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.body["error"]["code"], "login_invalid");
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn sign_in_emails_are_rate_limited_per_address(db: PgPool) {
    let app = TestApp::new(db);
    for _ in 0..app_core::auth::MAX_LOGIN_EMAILS_PER_HOUR {
        app.start_login("ada@example.com").await;
    }
    let res = app
        .send(
            Method::POST,
            "/api/auth/email/start",
            Some(json!({ "email": "ada@example.com" })),
            None,
        )
        .await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        app.mailer.sent().len(),
        app_core::auth::MAX_LOGIN_EMAILS_PER_HOUR as usize
    );
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn auth_endpoints_are_rate_limited_per_ip(db: PgPool) {
    let app = TestApp::new(db);
    let mut limited = false;
    for i in 0..30 {
        let body = json!({ "email": format!("user{i}@example.com") });
        let res = app
            .send_from(
                "198.51.100.7",
                Some(ORIGIN),
                Method::POST,
                "/api/auth/email/start",
                Some(body),
                None,
            )
            .await;
        if res.status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited, "per-IP limit never triggered");

    // Other clients are unaffected.
    let res = app
        .send(
            Method::POST,
            "/api/auth/email/start",
            Some(json!({ "email": "ada@example.com" })),
            None,
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn state_changes_require_app_origin(db: PgPool) {
    let app = TestApp::new(db);
    let body = Some(json!({ "email": "ada@example.com" }));
    for origin in [None, Some("https://evil.test"), Some("http://app.test")] {
        let res = app
            .send_from(
                "203.0.113.1",
                origin,
                Method::POST,
                "/api/auth/email/start",
                body.clone(),
                None,
            )
            .await;
        assert_eq!(res.status, StatusCode::FORBIDDEN, "origin {origin:?}");
        assert_eq!(res.body["error"]["code"], "bad_origin");
    }
    assert!(app.mailer.sent().is_empty());

    // Safe methods don't need an Origin.
    let res = app
        .send_from("203.0.113.1", None, Method::GET, "/api/health", None, None)
        .await;
    assert_eq!(res.status, StatusCode::OK);
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn me_requires_a_valid_session(db: PgPool) {
    let app = TestApp::new(db);
    for cookie in [None, Some("__Host-sid=not-a-real-session")] {
        let res = app.send(Method::GET, "/api/me", None, cookie).await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED);
        assert_eq!(res.body["error"]["code"], "unauthenticated");
    }
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn logout_ends_the_session(db: PgPool) {
    let app = TestApp::new(db);
    let cookie = app.sign_in("ada@example.com").await;

    let res = app
        .send(Method::POST, "/api/auth/logout", None, Some(&cookie))
        .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    let cleared = res
        .headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");

    let res = app.send(Method::GET, "/api/me", None, Some(&cookie)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn disabled_accounts_cannot_sign_in(db: PgPool) {
    let app = TestApp::new(db.clone());
    let cookie = app.sign_in("ada@example.com").await;
    sqlx::query("update users set deleted_at = now() where email = 'ada@example.com'")
        .execute(&db)
        .await
        .unwrap();

    let res = app.send(Method::GET, "/api/me", None, Some(&cookie)).await;
    assert_eq!(
        res.status,
        StatusCode::UNAUTHORIZED,
        "existing sessions stop working"
    );

    let (login_id, mail) = app.start_login("ada@example.com").await;
    let res = app
        .send(
            Method::POST,
            "/api/auth/email/verify-code",
            Some(json!({ "login_id": login_id, "code": code_from(&mail) })),
            None,
        )
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(res.body["error"]["code"], "account_disabled");
}
