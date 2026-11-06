//! Signed, expiring path tokens for private media.
//!
//! A media URL looks like `{MEDIA_BASE_URL}/{token}/m/{media_id}/hls/master.m3u8`.
//! HLS playlists use relative URLs, so every segment request inherits the token. The
//! token authorizes everything under one media item until it expires; it is checked by
//! the Cloudflare media gateway in production and by the API's `/media` route in dev.
//!
//! Format: `{expires_unix}.{sig}` where
//! `sig = base64url_nopad(HMAC-SHA256(secret, "v1:{media_id}:{expires_unix}")[..16])`.
//! Shared test vectors: `fixtures/media-token-vectors.json` (also used by the gateway).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use uuid::Uuid;

/// Tokens stay valid at least this long after being issued…
const MIN_LIFETIME_SECS: i64 = 6 * 3600;
/// …and expiry is rounded up to this, so URLs are stable (and cacheable) for a while.
const BUCKET_SECS: i64 = 3600;

#[derive(Clone)]
pub struct MediaTokens {
    secret: Vec<u8>,
}

impl MediaTokens {
    pub fn new(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            secret: secret.into(),
        }
    }

    /// A token for `media_id`, valid for 6–7 hours from `now`.
    pub fn issue(&self, media_id: Uuid, now_unix: i64) -> String {
        let expires =
            (now_unix + MIN_LIFETIME_SECS).div_euclid(BUCKET_SECS) * BUCKET_SECS + BUCKET_SECS;
        self.sign(media_id, expires)
    }

    pub fn sign(&self, media_id: Uuid, expires_unix: i64) -> String {
        format!(
            "{expires_unix}.{}",
            URL_SAFE_NO_PAD.encode(self.mac(media_id, expires_unix))
        )
    }

    pub fn verify(&self, token: &str, media_id: Uuid, now_unix: i64) -> bool {
        let Some((expires, sig)) = token.split_once('.') else {
            return false;
        };
        let Ok(expires) = expires.parse::<i64>() else {
            return false;
        };
        let Ok(sig) = URL_SAFE_NO_PAD.decode(sig) else {
            return false;
        };
        expires > now_unix && constant_time_eq(&sig, &self.mac(media_id, expires))
    }

    fn mac(&self, media_id: Uuid, expires_unix: i64) -> [u8; 16] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC accepts any key length");
        mac.update(format!("v1:{media_id}:{expires_unix}").as_bytes());
        let full = mac.finalize().into_bytes();
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        out
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Vectors {
        secret: String,
        cases: Vec<Case>,
    }
    #[derive(serde::Deserialize)]
    struct Case {
        media_id: Uuid,
        expires: i64,
        token: String,
    }

    #[test]
    fn matches_shared_test_vectors() {
        let raw = include_str!("../../../../fixtures/media-token-vectors.json");
        let vectors: Vectors = serde_json::from_str(raw).unwrap();
        let tokens = MediaTokens::new(vectors.secret.as_bytes());
        for case in vectors.cases {
            assert_eq!(tokens.sign(case.media_id, case.expires), case.token);
            assert!(tokens.verify(&case.token, case.media_id, case.expires - 1));
        }
    }

    #[test]
    fn rejects_expired_tampered_and_foreign_tokens() {
        let tokens = MediaTokens::new(b"0123456789abcdef0123456789abcdef".to_vec());
        let id = Uuid::now_v7();
        let now = 1_800_000_000;
        let token = tokens.issue(id, now);

        assert!(tokens.verify(&token, id, now));
        assert!(!tokens.verify(&token, id, now + 8 * 3600), "expired");
        assert!(!tokens.verify(&token, Uuid::now_v7(), now), "other media");
        let (exp, sig) = token.split_once('.').unwrap();
        assert!(
            !tokens.verify(
                &format!("{}.{sig}", exp.parse::<i64>().unwrap() + 3600),
                id,
                now
            ),
            "extended expiry"
        );
        assert!(
            !tokens.verify(&format!("{exp}.AAAAAAAAAAAAAAAAAAAAAA"), id, now),
            "forged sig"
        );
        assert!(!tokens.verify("garbage", id, now));
        let other_key = MediaTokens::new(b"another-secret-another-secret-xx".to_vec());
        assert!(!other_key.verify(&token, id, now), "different secret");
    }

    #[test]
    fn issued_tokens_are_stable_within_the_hour() {
        let tokens = MediaTokens::new(b"0123456789abcdef0123456789abcdef".to_vec());
        let id = Uuid::now_v7();
        let start: i64 = 1_800_000_000; // on an hour boundary
        assert_eq!(start % 3600, 0);
        assert_eq!(tokens.issue(id, start + 1), tokens.issue(id, start + 3599));
        assert_ne!(tokens.issue(id, start + 1), tokens.issue(id, start + 3601));
    }
}
