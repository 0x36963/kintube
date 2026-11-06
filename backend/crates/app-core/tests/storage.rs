//! Compatibility test against the dev S3 server (RustFS): exercises exactly the
//! operations the upload flow, workers and media route rely on.
//! Needs `make dev-up`. Uses the `S3_*` env vars if set, else the dev defaults.

use std::time::Duration;

use app_core::{
    config::S3Config,
    storage::{Storage, media_prefix, original_key},
};
use uuid::Uuid;

fn s3_config() -> S3Config {
    let env = |k: &str, default: &str| std::env::var(k).unwrap_or_else(|_| default.to_owned());
    let endpoint = env("S3_ENDPOINT", "http://localhost:9000");
    S3Config {
        public_endpoint: endpoint.clone(),
        endpoint,
        region: env("S3_REGION", "us-east-1"),
        bucket: env("S3_TEST_BUCKET", "kintube-test"),
        access_key_id: env("S3_ACCESS_KEY_ID", "kintube"),
        secret_access_key: env("S3_SECRET_ACCESS_KEY", "kintube-dev-secret"),
        force_path_style: true,
    }
}

const ORIGIN: &str = "http://localhost:5173";

#[tokio::test]
async fn multipart_upload_round_trip_via_presigned_urls() {
    let storage = Storage::new(&s3_config());
    storage.setup_bucket(&[ORIGIN.to_owned()]).await.unwrap();
    storage
        .setup_bucket(&[ORIGIN.to_owned()])
        .await
        .expect("setup is idempotent");

    let media_id = Uuid::now_v7();
    let key = original_key(media_id);
    let upload_id = storage
        .create_multipart_upload(&key, "video/mp4")
        .await
        .unwrap();

    // Two parts: 5 MiB (S3's minimum for non-final parts) + a short final part.
    let part1: Vec<u8> = (0..5 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let part2 = b"the end".to_vec();
    let http = reqwest::Client::new();

    // The browser's CORS preflight for a part upload must be allowed.
    let url1 = storage
        .presign_upload_part(&key, &upload_id, 1, Duration::from_secs(600))
        .await
        .unwrap();
    let preflight = http
        .request(reqwest::Method::OPTIONS, &url1)
        .header("Origin", ORIGIN)
        .header("Access-Control-Request-Method", "PUT")
        .header("Access-Control-Request-Headers", "content-type")
        .send()
        .await
        .unwrap();
    assert!(
        preflight.status().is_success(),
        "preflight status {}",
        preflight.status()
    );
    let allowed = preflight
        .headers()
        .get("access-control-allow-origin")
        .map(|v| v.to_str().unwrap().to_owned());
    assert!(
        matches!(allowed.as_deref(), Some(ORIGIN) | Some("*")),
        "allow-origin: {allowed:?}"
    );

    // Upload part 2 first: parts may arrive in any order.
    for (number, body) in [(2, part2.clone()), (1, part1.clone())] {
        let url = storage
            .presign_upload_part(&key, &upload_id, number, Duration::from_secs(600))
            .await
            .unwrap();
        let res = http
            .put(&url)
            .header("Origin", ORIGIN)
            .body(body)
            .send()
            .await
            .unwrap();
        assert!(
            res.status().is_success(),
            "part {number}: {} {}",
            res.status(),
            res.text().await.unwrap()
        );
    }

    // The server learns ETags from ListParts, so the browser never has to read them.
    let parts = storage.list_parts(&key, &upload_id).await.unwrap();
    assert_eq!(
        parts.iter().map(|p| (p.number, p.size)).collect::<Vec<_>>(),
        vec![(1, part1.len() as i64), (2, part2.len() as i64)]
    );
    storage
        .complete_multipart_upload(&key, &upload_id, &parts)
        .await
        .unwrap();

    // Ranged read straddling the part boundary.
    let start = part1.len() - 3;
    let object = storage
        .get_object(&key, Some(&format!("bytes={start}-{}", start + 5)))
        .await
        .unwrap()
        .expect("object exists");
    assert_eq!(object.content_length, Some(6));
    assert!(
        object
            .content_range
            .as_deref()
            .is_some_and(|r| r.starts_with(&format!("bytes {start}-")))
    );
    let bytes = object.body.collect().await.unwrap().into_bytes();
    let mut expected = part1[start..].to_vec();
    expected.extend_from_slice(&part2[..3]);
    assert_eq!(&bytes[..], &expected[..]);

    // Prefix delete removes everything for the media item.
    storage
        .put_bytes(
            &format!("{}hls/master.m3u8", media_prefix(media_id)),
            b"#EXTM3U".to_vec(),
            "application/vnd.apple.mpegurl",
        )
        .await
        .unwrap();
    assert_eq!(
        storage
            .delete_prefix(&media_prefix(media_id))
            .await
            .unwrap(),
        2
    );
    assert!(storage.get_object(&key, None).await.unwrap().is_none());
}

#[tokio::test]
async fn aborted_uploads_are_gone_and_abort_is_idempotent() {
    let storage = Storage::new(&s3_config());
    storage.setup_bucket(&[ORIGIN.to_owned()]).await.unwrap();
    let key = original_key(Uuid::now_v7());
    let upload_id = storage
        .create_multipart_upload(&key, "video/mp4")
        .await
        .unwrap();

    storage
        .abort_multipart_upload(&key, &upload_id)
        .await
        .unwrap();
    storage
        .abort_multipart_upload(&key, &upload_id)
        .await
        .expect("aborting twice is fine");
    assert!(
        storage.list_parts(&key, &upload_id).await.is_err(),
        "upload no longer exists"
    );
}
