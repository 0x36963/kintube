//! S3-compatible object storage (Cloudflare R2 in production, RustFS in development).
//!
//! Object layout: everything for one media item lives under `m/{media_id}/`
//! (`original`, `hls/…`, `poster/…`, `sprites/…`), so the media gateway can map a
//! URL path straight to a key and purging an item is a prefix delete.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use aws_sdk_s3::{
    Client,
    config::{
        BehaviorVersion, Credentials, Region, RequestChecksumCalculation,
        ResponseChecksumValidation,
    },
    presigning::PresigningConfig,
    primitives::ByteStream,
    types::{
        AbortIncompleteMultipartUpload, BucketLifecycleConfiguration, CompletedMultipartUpload,
        CompletedPart, CorsConfiguration, CorsRule, Delete, ExpirationStatus, LifecycleRule,
        LifecycleRuleFilter, ObjectIdentifier,
    },
};
use uuid::Uuid;

use crate::config::S3Config;

pub fn media_prefix(media_id: Uuid) -> String {
    format!("m/{media_id}/")
}

pub fn original_key(media_id: Uuid) -> String {
    format!("m/{media_id}/original")
}

#[derive(Clone)]
pub struct Storage {
    client: Client,
    /// Same credentials, but signs URLs for the browser-facing endpoint.
    presign_client: Client,
    bucket: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedPart {
    pub number: i32,
    pub size: i64,
    pub etag: String,
}

/// A (possibly partial) object body plus the headers needed to serve it.
pub struct ObjectBody {
    pub body: ByteStream,
    pub content_length: Option<i64>,
    pub content_range: Option<String>,
    pub content_type: Option<String>,
    pub etag: Option<String>,
}

impl Storage {
    pub fn new(config: &S3Config) -> Self {
        let client_for = |endpoint: &str| {
            let conf = aws_sdk_s3::config::Builder::new()
                .behavior_version(BehaviorVersion::latest())
                .endpoint_url(endpoint)
                .region(Region::new(config.region.clone()))
                .credentials_provider(Credentials::new(
                    &config.access_key_id,
                    &config.secret_access_key,
                    None,
                    None,
                    "kintube-config",
                ))
                .force_path_style(config.force_path_style)
                // Newer SDKs add CRC checksums to uploads by default. Presigned part URLs
                // would then require browsers to send matching checksum headers, and some
                // S3-compatible stores reject them, so only use checksums where S3 requires them.
                .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
                .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
                .build();
            Client::from_conf(conf)
        };
        Self {
            client: client_for(&config.endpoint),
            presign_client: client_for(&config.public_endpoint),
            bucket: config.bucket.clone(),
        }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Creates the bucket if needed and applies CORS (browsers PUT upload parts directly)
    /// and a lifecycle rule that aborts abandoned multipart uploads. Idempotent.
    pub async fn setup_bucket(&self, upload_origins: &[String]) -> Result<()> {
        if self
            .client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .is_err()
        {
            self.client
                .create_bucket()
                .bucket(&self.bucket)
                .send()
                .await
                .with_context(|| format!("creating bucket {}", self.bucket))?;
            tracing::info!(bucket = %self.bucket, "created bucket");
        }

        let rule = CorsRule::builder()
            .set_allowed_origins(Some(upload_origins.to_vec()))
            .allowed_methods("PUT")
            .allowed_headers("*")
            .max_age_seconds(3600)
            .build()?;
        self.client
            .put_bucket_cors()
            .bucket(&self.bucket)
            .cors_configuration(CorsConfiguration::builder().cors_rules(rule).build()?)
            .send()
            .await
            .context("setting bucket CORS")?;

        // Best effort: our own sweep also cleans up stale uploads, and not every
        // S3-compatible server implements lifecycle rules.
        let lifecycle = LifecycleRule::builder()
            .id("abort-stale-multipart-uploads")
            .status(ExpirationStatus::Enabled)
            .filter(LifecycleRuleFilter::builder().prefix("").build())
            .abort_incomplete_multipart_upload(
                AbortIncompleteMultipartUpload::builder()
                    .days_after_initiation(7)
                    .build(),
            )
            .build()?;
        if let Err(err) = self
            .client
            .put_bucket_lifecycle_configuration()
            .bucket(&self.bucket)
            .lifecycle_configuration(
                BucketLifecycleConfiguration::builder()
                    .rules(lifecycle)
                    .build()?,
            )
            .send()
            .await
        {
            tracing::warn!(error = %aws_sdk_s3::error::DisplayErrorContext(&err), "could not set bucket lifecycle rule");
        }
        Ok(())
    }

    pub async fn create_multipart_upload(&self, key: &str, content_type: &str) -> Result<String> {
        let out = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .send()
            .await
            .context("creating multipart upload")?;
        out.upload_id.context("storage returned no upload id")
    }

    /// A URL the browser can `PUT` one part's bytes to.
    pub async fn presign_upload_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: i32,
        expires_in: Duration,
    ) -> Result<String> {
        let presigned = self
            .presign_client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part_number)
            .presigned(PresigningConfig::expires_in(expires_in)?)
            .await
            .context("presigning upload part")?;
        Ok(presigned.uri().to_owned())
    }

    /// All parts uploaded so far, in part-number order.
    pub async fn list_parts(&self, key: &str, upload_id: &str) -> Result<Vec<UploadedPart>> {
        let mut parts = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let out = self
                .client
                .list_parts()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(upload_id)
                .set_part_number_marker(marker.take())
                .send()
                .await
                .context("listing uploaded parts")?;
            for p in out.parts() {
                if let (Some(number), Some(etag)) = (p.part_number, p.e_tag()) {
                    parts.push(UploadedPart {
                        number,
                        size: p.size.unwrap_or(0),
                        etag: etag.to_owned(),
                    });
                }
            }
            match (out.is_truncated, out.next_part_number_marker) {
                (Some(true), Some(next)) => marker = Some(next),
                _ => break,
            }
        }
        parts.sort_by_key(|p| p.number);
        Ok(parts)
    }

    pub async fn complete_multipart_upload(
        &self,
        key: &str,
        upload_id: &str,
        parts: &[UploadedPart],
    ) -> Result<()> {
        let completed = CompletedMultipartUpload::builder()
            .set_parts(Some(
                parts
                    .iter()
                    .map(|p| {
                        CompletedPart::builder()
                            .part_number(p.number)
                            .e_tag(&p.etag)
                            .build()
                    })
                    .collect(),
            ))
            .build();
        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(completed)
            .send()
            .await
            .context("completing multipart upload")?;
        Ok(())
    }

    /// Aborts an upload. An upload that no longer exists counts as aborted.
    pub async fn abort_multipart_upload(&self, key: &str, upload_id: &str) -> Result<()> {
        match self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(err)
                if err
                    .as_service_error()
                    .is_some_and(|e| e.is_no_such_upload()) =>
            {
                Ok(())
            }
            Err(err) => Err(err).context("aborting multipart upload"),
        }
    }

    /// `None` if the object doesn't exist. `range` is an HTTP `Range` header value.
    pub async fn get_object(&self, key: &str, range: Option<&str>) -> Result<Option<ObjectBody>> {
        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .set_range(range.map(str::to_owned))
            .send()
            .await
        {
            Ok(out) => Ok(Some(ObjectBody {
                content_length: out.content_length,
                content_range: out.content_range,
                content_type: out.content_type,
                etag: out.e_tag,
                body: out.body,
            })),
            Err(err) if err.as_service_error().is_some_and(|e| e.is_no_such_key()) => Ok(None),
            Err(err) => Err(err).context("reading object"),
        }
    }

    pub async fn download_to_file(&self, key: &str, path: &Path) -> Result<u64> {
        let object = self
            .get_object(key, None)
            .await?
            .with_context(|| format!("object {key} not found"))?;
        let mut reader = object.body.into_async_read();
        let mut file = tokio::fs::File::create(path).await?;
        let bytes = tokio::io::copy(&mut reader, &mut file).await?;
        file.sync_all().await?;
        Ok(bytes)
    }

    pub async fn put_file(&self, key: &str, path: &Path, content_type: &str) -> Result<()> {
        let body = ByteStream::from_path(path)
            .await
            .with_context(|| format!("reading {}", path.display()))?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(body)
            .send()
            .await
            .with_context(|| format!("uploading {key}"))?;
        Ok(())
    }

    pub async fn put_bytes(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .with_context(|| format!("uploading {key}"))?;
        Ok(())
    }

    /// Deletes every object under `prefix`. Returns how many were deleted.
    pub async fn delete_prefix(&self, prefix: &str) -> Result<usize> {
        let mut deleted = 0;
        loop {
            // Always list from the start: we delete what we list, so the next page
            // is whatever remains.
            let out = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .max_keys(1000)
                .send()
                .await
                .context("listing objects")?;
            let keys: Vec<ObjectIdentifier> = out
                .contents()
                .iter()
                .filter_map(|o| o.key())
                .map(|k| ObjectIdentifier::builder().key(k).build())
                .collect::<Result<_, _>>()?;
            if keys.is_empty() {
                return Ok(deleted);
            }
            deleted += keys.len();
            self.client
                .delete_objects()
                .bucket(&self.bucket)
                .delete(
                    Delete::builder()
                        .set_objects(Some(keys))
                        .quiet(true)
                        .build()?,
                )
                .send()
                .await
                .context("deleting objects")?;
        }
    }
}

/// Content type for a stored media file, by extension.
pub fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "m3u8" => "application/vnd.apple.mpegurl",
        "m4s" => "video/iso.segment",
        "mp4" => "video/mp4",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "vtt" => "text/vtt; charset=utf-8",
        _ => "application/octet-stream",
    }
}
