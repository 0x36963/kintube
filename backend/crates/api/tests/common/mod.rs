//! Shared harness for API integration tests.
#![allow(dead_code)] // each test binary uses a different subset

use api::AppState;
use app_core::{
    Config, Mailer,
    config::{MediaConfig, S3Config},
    mailer::Email,
};
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// Signed media URLs start with this; strip the host to request them from the router.
pub const MEDIA_HOST: &str = "http://media.test";

pub fn test_config() -> Config {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
    let endpoint = env("S3_ENDPOINT", "http://localhost:9000");
    Config {
        app_name: "testtube".into(),
        app_origin: ORIGIN.into(),
        database_url: String::new(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        smtp_url: String::new(),
        email_from: "noreply@app.test".into(),
        cookie_secure: true,
        client_ip_header: Some("x-test-ip".into()),
        auth_rate_limit_per_minute: 20,
        s3: S3Config {
            public_endpoint: endpoint.clone(),
            endpoint,
            region: env("S3_REGION", "us-east-1"),
            bucket: env("S3_TEST_BUCKET", "kintube-test"),
            access_key_id: env("S3_ACCESS_KEY_ID", "kintube"),
            secret_access_key: env("S3_SECRET_ACCESS_KEY", "kintube-dev-secret"),
            force_path_style: true,
        },
        media: MediaConfig {
            base_url: format!("{MEDIA_HOST}/media"),
            token_secret: b"test-media-token-secret-0123456789".to_vec(),
            upload_part_size: 5 * 1024 * 1024,
            serve_from_api: true,
        },
    }
}

pub const ORIGIN: &str = "https://app.test";

pub struct TestApp {
    pub router: Router,
    pub mailer: Mailer,
    pub state: AppState,
}

pub struct Res {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
    /// Raw body, for non-JSON responses.
    pub bytes: axum::body::Bytes,
}

impl TestApp {
    pub fn new(db: PgPool) -> Self {
        let mailer = Mailer::memory();
        let state = AppState::new(db, mailer.clone(), test_config());
        Self {
            router: api::app(state.clone()),
            mailer,
            state,
        }
    }

    /// Also makes sure the test bucket exists (needs the dev S3 server).
    pub async fn with_storage(db: PgPool) -> Self {
        let app = Self::new(db);
        app.state
            .storage
            .setup_bucket(&[ORIGIN.to_owned()])
            .await
            .expect("dev S3 server is running");
        app
    }

    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        cookie: Option<&str>,
    ) -> Res {
        self.send_from("203.0.113.1", Some(ORIGIN), method, path, body, cookie)
            .await
    }

    pub async fn send_from(
        &self,
        ip: &str,
        origin: Option<&str>,
        method: Method,
        path: &str,
        body: Option<Value>,
        cookie: Option<&str>,
    ) -> Res {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("x-test-ip", ip);
        if let Some(origin) = origin {
            req = req.header(header::ORIGIN, origin);
        }
        if let Some(cookie) = cookie {
            req = req.header(header::COOKIE, cookie);
        }
        let req = match body {
            Some(json) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();

        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Res {
            status,
            headers,
            body,
            bytes,
        }
    }

    pub async fn start_login(&self, email: &str) -> (String, Email) {
        let res = self
            .send(
                Method::POST,
                "/api/auth/email/start",
                Some(json!({ "email": email })),
                None,
            )
            .await;
        assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
        let login_id = res.body["login_id"].as_str().unwrap().to_owned();
        (
            login_id,
            self.mailer.sent().pop().expect("an email was sent"),
        )
    }

    /// Full code flow; returns the `Cookie` header value for the new session.
    pub async fn sign_in(&self, email: &str) -> String {
        let (login_id, mail) = self.start_login(email).await;
        let res = self
            .send(
                Method::POST,
                "/api/auth/email/verify-code",
                Some(json!({ "login_id": login_id, "code": code_from(&mail) })),
                None,
            )
            .await;
        assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
        session_cookie(&res.headers)
    }
}

pub fn code_from(mail: &Email) -> String {
    mail.subject[..6].to_owned()
}

pub fn token_from(mail: &Email) -> String {
    let start = mail.text.find("#token=").expect("link in email") + "#token=".len();
    mail.text[start..]
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

/// `name=value` from the response's Set-Cookie, ready to send back as a Cookie header.
pub fn session_cookie(headers: &HeaderMap) -> String {
    let set_cookie = headers
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .unwrap();
    for attr in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
        assert!(
            set_cookie.contains(attr),
            "cookie missing {attr}: {set_cookie}"
        );
    }
    let pair = set_cookie.split(';').next().unwrap().to_owned();
    assert!(pair.starts_with("__Host-sid="), "{pair}");
    pair
}
