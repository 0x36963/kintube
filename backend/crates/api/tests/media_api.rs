//! HTTP contract for uploads, the library, media files and live events.
//! Needs Postgres (`DATABASE_URL`) and the dev S3 server (`make dev-up`).

mod common;

use std::time::Duration;

use app_core::{
    events::MediaEvent,
    media::{self, NewAsset, ProcessedVideo},
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use common::*;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const MIB: usize = 1024 * 1024;

/// Uploads `bytes` through the real protocol (presigned PUTs to S3). Returns the media id.
async fn upload(app: &TestApp, cookie: &str, filename: &str, bytes: usize) -> String {
    let res = app
        .send(
            Method::POST,
            "/api/uploads",
            Some(json!({ "filename": filename, "size": bytes, "mime_type": "video/mp4" })),
            Some(cookie),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.body);
    let id = res.body["media_id"].as_str().unwrap().to_owned();
    let part_size = res.body["part_size"].as_u64().unwrap() as usize;
    let count = res.body["part_count"].as_u64().unwrap() as i32;

    let numbers: Vec<i32> = (1..=count).collect();
    let res = app
        .send(
            Method::POST,
            &format!("/api/uploads/{id}/parts"),
            Some(json!({ "part_numbers": numbers })),
            Some(cookie),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    let http = reqwest::Client::new();
    for part in res.body["parts"].as_array().unwrap() {
        let n = part["part_number"].as_u64().unwrap() as usize;
        let len = (bytes - (n - 1) * part_size).min(part_size);
        let put = http
            .put(part["url"].as_str().unwrap())
            .body(vec![7u8; len])
            .send()
            .await
            .unwrap();
        assert!(put.status().is_success(), "part {n}: {}", put.status());
    }
    let res = app
        .send(
            Method::GET,
            &format!("/api/uploads/{id}"),
            None,
            Some(cookie),
        )
        .await;
    assert_eq!(
        res.body["uploaded_parts"].as_array().unwrap().len(),
        count as usize
    );

    let res = app
        .send(
            Method::POST,
            &format!("/api/uploads/{id}/complete"),
            None,
            Some(cookie),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(res.body["status"], "processing");
    id
}

/// Pretends a worker processed the video: stores a playlist, a segment and a poster.
async fn fake_processing(app: &TestApp, id: &str) {
    let media_id: Uuid = id.parse().unwrap();
    let storage = &app.state.storage;
    let prefix = format!("m/{media_id}/");
    storage
        .put_bytes(
            &format!("{prefix}hls/master.m3u8"),
            b"#EXTM3U\nv0/index.m3u8\n".to_vec(),
            "application/vnd.apple.mpegurl",
        )
        .await
        .unwrap();
    storage
        .put_bytes(
            &format!("{prefix}hls/v0/seg_0000.m4s"),
            (0..=255u8).collect(),
            "video/iso.segment",
        )
        .await
        .unwrap();
    storage
        .put_bytes(
            &format!("{prefix}poster/320.webp"),
            b"RIFFfake".to_vec(),
            "image/webp",
        )
        .await
        .unwrap();
    media::mark_ready(
        &app.state.db,
        media_id,
        &ProcessedVideo {
            width: 1280,
            height: 720,
            duration_ms: 7000,
            video_codec: "h264".into(),
            audio_codec: Some("aac".into()),
            hdr: false,
            taken_at: None,
            weighted_seconds: 7,
            assets: vec![
                NewAsset { kind: "hls", path: "hls/master.m3u8".into(), bytes: 300, meta: json!({ "renditions": [{ "label": "720p", "width": 1280, "height": 720 }] }) },
                NewAsset { kind: "poster", path: "poster/320.webp".into(), bytes: 8, meta: json!({ "sizes": [{ "width": 320, "height": 180, "path": "poster/320.webp" }] }) },
            ],
        },
    )
    .await
    .unwrap();
}

fn media_path(url: &Value) -> String {
    url.as_str()
        .unwrap()
        .strip_prefix(MEDIA_HOST)
        .expect("signed URL on the media host")
        .to_owned()
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn upload_process_and_play(db: PgPool) {
    let app = TestApp::with_storage(db).await;
    let cookie = app.sign_in("ada@example.com").await;
    let id = upload(&app, &cookie, "Beach day.mp4", 6 * MIB + 3).await;

    let list = app
        .send(Method::GET, "/api/media", None, Some(&cookie))
        .await;
    assert_eq!(list.body["items"][0]["id"], id.as_str());
    assert_eq!(list.body["items"][0]["title"], "Beach day");
    let detail = app
        .send(
            Method::GET,
            &format!("/api/media/{id}"),
            None,
            Some(&cookie),
        )
        .await;
    assert!(
        detail.body["playback"].is_null(),
        "not playable while processing"
    );

    fake_processing(&app, &id).await;
    let detail = app
        .send(
            Method::GET,
            &format!("/api/media/{id}"),
            None,
            Some(&cookie),
        )
        .await;
    assert_eq!(detail.body["media"]["status"], "ready");
    assert_eq!(detail.body["playback"]["renditions"][0]["label"], "720p");
    let poster = &detail.body["media"]["posters"][0];
    assert_eq!(
        (poster["width"].as_u64(), poster["height"].as_u64()),
        (Some(320), Some(180))
    );

    // The signed playlist URL works from any origin, with no cookie.
    let playlist = app
        .send_from(
            "203.0.113.9",
            None,
            Method::GET,
            &media_path(&detail.body["playback"]["hls_url"]),
            None,
            None,
        )
        .await;
    assert_eq!(playlist.status, StatusCode::OK);
    assert_eq!(
        playlist.headers[header::CONTENT_TYPE],
        "application/vnd.apple.mpegurl"
    );
    assert_eq!(playlist.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    assert_eq!(playlist.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert!(playlist.bytes.starts_with(b"#EXTM3U"));

    // Segments resolve relative to the playlist, inheriting the token; ranges work.
    let segment =
        media_path(&detail.body["playback"]["hls_url"]).replace("master.m3u8", "v0/seg_0000.m4s");
    let req = Request::get(&segment)
        .header(header::RANGE, "bytes=10-19")
        .body(Body::empty())
        .unwrap();
    let res = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 10-19/256");
    assert_eq!(
        &res.into_body().collect().await.unwrap().to_bytes()[..],
        &(10..=19u8).collect::<Vec<_>>()[..]
    );

    let usage = app
        .send(Method::GET, "/api/me/usage", None, Some(&cookie))
        .await;
    assert_eq!(usage.body["storage_used_bytes"], 6 * MIB + 3);
    assert_eq!(usage.body["storage_limit_bytes"], 21_474_836_480i64);
    assert_eq!(usage.body["processing_minutes_used"], 1);
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn media_urls_are_bound_to_their_item_and_unforgeable(db: PgPool) {
    let app = TestApp::with_storage(db).await;
    let cookie = app.sign_in("ada@example.com").await;
    let id = upload(&app, &cookie, "a.mp4", 1000).await;
    let other = upload(&app, &cookie, "b.mp4", 1000).await;
    fake_processing(&app, &id).await;
    fake_processing(&app, &other).await;
    let detail = app
        .send(
            Method::GET,
            &format!("/api/media/{id}"),
            None,
            Some(&cookie),
        )
        .await;
    let path = media_path(&detail.body["playback"]["hls_url"]); // /media/{token}/m/{id}/hls/master.m3u8
    let token = path.split('/').nth(2).unwrap();

    let cases = [
        (
            path.replace(&id, &other),
            StatusCode::FORBIDDEN,
            "token for another item",
        ),
        (
            path.replace(token, &format!("{}x", token)),
            StatusCode::FORBIDDEN,
            "tampered signature",
        ),
        (
            path.replace(token, "4102444800.AAAAAAAAAAAAAAAAAAAAAA"),
            StatusCode::FORBIDDEN,
            "forged token",
        ),
        (
            format!("/media/{token}/m/{id}/original"),
            StatusCode::NOT_FOUND,
            "originals aren't served",
        ),
        (
            format!("/media/{token}/m/{id}/hls/..%2Foriginal"),
            StatusCode::NOT_FOUND,
            "encoded traversal",
        ),
        (
            format!("/media/{token}/m/{id}/hls/missing.m3u8"),
            StatusCode::NOT_FOUND,
            "missing file",
        ),
    ];
    for (path, expected, why) in cases {
        let res = app
            .send_from("203.0.113.9", None, Method::GET, &path, None, None)
            .await;
        assert_eq!(res.status, expected, "{why}: {path}");
    }
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn library_is_private_to_its_owner(db: PgPool) {
    let app = TestApp::with_storage(db).await;
    let ada = app.sign_in("ada@example.com").await;
    let eve = app.sign_in("eve@example.com").await;
    let id = upload(&app, &ada, "private.mp4", 1000).await;

    for (method, path, body) in [
        (Method::GET, format!("/api/media/{id}"), None),
        (
            Method::PATCH,
            format!("/api/media/{id}"),
            Some(json!({ "title": "mine now" })),
        ),
        (Method::DELETE, format!("/api/media/{id}"), None),
        (Method::POST, format!("/api/media/{id}/restore"), None),
        (Method::DELETE, format!("/api/trash/{id}"), None),
        (Method::GET, format!("/api/uploads/{id}"), None),
        (Method::POST, format!("/api/uploads/{id}/complete"), None),
    ] {
        let res = app.send(method.clone(), &path, body, Some(&eve)).await;
        assert_eq!(res.status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    let eve_list = app.send(Method::GET, "/api/media", None, Some(&eve)).await;
    assert_eq!(eve_list.body["items"], json!([]));

    for (method, path) in [
        (Method::GET, "/api/media"),
        (Method::POST, "/api/uploads"),
        (Method::GET, "/api/events"),
        (Method::GET, "/api/me/usage"),
    ] {
        let res = app.send(method.clone(), path, Some(json!({})), None).await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn edit_trash_restore_and_purge(db: PgPool) {
    let app = TestApp::with_storage(db).await;
    let cookie = app.sign_in("ada@example.com").await;
    let id = upload(&app, &cookie, "clip.mp4", 1000).await;

    let res = app
        .send(
            Method::PATCH,
            &format!("/api/media/{id}"),
            Some(json!({ "title": "  Grandma's 90th  ", "description": "Cake!" })),
            Some(&cookie),
        )
        .await;
    assert_eq!(
        (res.body["title"].as_str(), res.body["description"].as_str()),
        (Some("Grandma's 90th"), Some("Cake!"))
    );
    for (body, code) in [
        (json!({ "title": "   " }), "invalid_title"),
        (
            json!({ "description": "x".repeat(5001) }),
            "invalid_description",
        ),
    ] {
        let res = app
            .send(
                Method::PATCH,
                &format!("/api/media/{id}"),
                Some(body),
                Some(&cookie),
            )
            .await;
        assert_eq!(res.body["error"]["code"], code);
    }

    let purge_early = app
        .send(
            Method::DELETE,
            &format!("/api/trash/{id}"),
            None,
            Some(&cookie),
        )
        .await;
    assert_eq!(purge_early.body["error"]["code"], "invalid_state");
    app.send(
        Method::DELETE,
        &format!("/api/media/{id}"),
        None,
        Some(&cookie),
    )
    .await;
    let trash = app
        .send(Method::GET, "/api/trash", None, Some(&cookie))
        .await;
    assert_eq!(
        (
            trash.body["items"][0]["id"].as_str(),
            trash.body["retention_days"].as_u64()
        ),
        (Some(id.as_str()), Some(30))
    );
    assert_eq!(
        app.send(Method::GET, "/api/media", None, Some(&cookie))
            .await
            .body["items"],
        json!([])
    );

    app.send(
        Method::POST,
        &format!("/api/media/{id}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(
        app.send(Method::GET, "/api/media", None, Some(&cookie))
            .await
            .body["items"][0]["id"],
        id.as_str()
    );

    app.send(
        Method::DELETE,
        &format!("/api/media/{id}"),
        None,
        Some(&cookie),
    )
    .await;
    let res = app
        .send(
            Method::DELETE,
            &format!("/api/trash/{id}"),
            None,
            Some(&cookie),
        )
        .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.send(Method::GET, "/api/me/usage", None, Some(&cookie))
            .await
            .body["storage_used_bytes"],
        0
    );
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn upload_validation(db: PgPool) {
    let app = TestApp::with_storage(db.clone()).await;
    let cookie = app.sign_in("ada@example.com").await;
    sqlx::query("update users set storage_limit_bytes = 5000")
        .execute(&db)
        .await
        .unwrap();

    for (body, status, code) in [
        (
            json!({ "filename": "x.jpg", "size": 10, "mime_type": "image/jpeg" }),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_type",
        ),
        (
            json!({ "filename": "x.mp4", "size": 0, "mime_type": "video/mp4" }),
            StatusCode::BAD_REQUEST,
            "empty_file",
        ),
        (
            json!({ "filename": "  ", "size": 10, "mime_type": "video/mp4" }),
            StatusCode::BAD_REQUEST,
            "invalid_filename",
        ),
        (
            json!({ "filename": "x.mp4", "size": 6000, "mime_type": "video/mp4" }),
            StatusCode::CONFLICT,
            "storage_quota_exceeded",
        ),
    ] {
        let res = app
            .send(
                Method::POST,
                "/api/uploads",
                Some(body.clone()),
                Some(&cookie),
            )
            .await;
        assert_eq!(
            (res.status, res.body["error"]["code"].as_str()),
            (status, Some(code)),
            "{body}"
        );
    }

    let res = app
        .send(
            Method::POST,
            "/api/uploads",
            Some(json!({ "filename": "x.mp4", "size": 3000, "mime_type": "video/mp4" })),
            Some(&cookie),
        )
        .await;
    let id = res.body["media_id"].as_str().unwrap();
    let early = app
        .send(
            Method::POST,
            &format!("/api/uploads/{id}/complete"),
            None,
            Some(&cookie),
        )
        .await;
    assert_eq!(early.body["error"]["code"], "upload_incomplete");
    let res = app
        .send(
            Method::POST,
            &format!("/api/uploads/{id}/parts"),
            Some(json!({ "part_numbers": [2] })),
            Some(&cookie),
        )
        .await;
    assert_eq!(
        res.body["error"]["code"], "invalid_state",
        "only one part exists"
    );
    let res = app
        .send(
            Method::DELETE,
            &format!("/api/uploads/{id}"),
            None,
            Some(&cookie),
        )
        .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.send(Method::GET, "/api/me/usage", None, Some(&cookie))
            .await
            .body["storage_reserved_bytes"],
        0
    );
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn library_pages_with_a_cursor(db: PgPool) {
    let app = TestApp::new(db.clone());
    let cookie = app.sign_in("ada@example.com").await;
    let me = app.send(Method::GET, "/api/me", None, Some(&cookie)).await;
    let owner: Uuid = me.body["id"].as_str().unwrap().parse().unwrap();
    for i in 0..5 {
        sqlx::query(
            "insert into media (id, owner_id, kind, status, title, original_filename, mime, bytes, created_at)
             values ($1, $2, 'video', 'ready', $3, 'f.mp4', 'video/mp4', 1, now() - make_interval(mins => $4))",
        )
        .bind(Uuid::now_v7())
        .bind(owner)
        .bind(format!("v{i}"))
        .bind(i)
        .execute(&db)
        .await
        .unwrap();
    }
    let mut titles = Vec::new();
    let mut url = "/api/media?limit=2".to_owned();
    loop {
        let page = app.send(Method::GET, &url, None, Some(&cookie)).await;
        titles.extend(
            page.body["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["title"].as_str().unwrap().to_owned()),
        );
        match page.body["next_cursor"].as_str() {
            Some(cursor) => url = format!("/api/media?limit=2&cursor={cursor}"),
            None => break,
        }
    }
    assert_eq!(
        titles,
        ["v0", "v1", "v2", "v3", "v4"],
        "newest first, no gaps or repeats"
    );
    let bad = app
        .send(Method::GET, "/api/media?cursor=nope", None, Some(&cookie))
        .await;
    assert_eq!(bad.body["error"]["code"], "bad_cursor");
}

#[sqlx::test(migrator = "app_core::db::MIGRATOR")]
async fn events_stream_only_carries_the_users_own_updates(db: PgPool) {
    let app = TestApp::new(db);
    let cookie = app.sign_in("ada@example.com").await;
    let me = app.send(Method::GET, "/api/me", None, Some(&cookie)).await;
    let owner: Uuid = me.body["id"].as_str().unwrap().parse().unwrap();

    let req = Request::get("/api/events")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let res = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.headers()[header::CONTENT_TYPE], "text/event-stream");
    let mut body = res.into_body();

    let (mine, theirs) = (Uuid::now_v7(), Uuid::now_v7());
    let event = |owner_id, media_id| MediaEvent {
        owner_id,
        media_id,
        status: "processing".into(),
        progress: 0.5,
        error: None,
    };
    app.state
        .events
        .send(event(Uuid::now_v7(), theirs))
        .unwrap();
    app.state.events.send(event(owner, mine)).unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
    assert!(text.starts_with("event: media\ndata: "), "{text}");
    assert!(
        text.contains(&mine.to_string()) && !text.contains(&theirs.to_string()),
        "{text}"
    );
    assert!(!text.contains(&owner.to_string()), "owner id isn't echoed");
}
