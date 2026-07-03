//! GitHub App JWT minting (RS256).
//!
//! Behaviour parity with Coolify's `generateGithubToken(..., 'jwt')`
//! (bootstrap/helpers/github.php:30-43): the token is issued by the App id,
//! back-dated one minute to tolerate clock skew, and expires eight minutes out.
//! GitHub caps App JWT lifetime at ten minutes, so eight is safely inside it.
//!
//! The signing key must be an RSA private key in PEM (PKCS#1 `BEGIN RSA PRIVATE
//! KEY` or PKCS#8 `BEGIN PRIVATE KEY`) — the format GitHub hands out. OpenSSH
//! keys (`BEGIN OPENSSH PRIVATE KEY`) are rejected up front: `jsonwebtoken`
//! cannot load them and a clear error beats an opaque one downstream.

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;

use crate::error::CoreError;

/// Registered claims for a GitHub App JWT. `iss` is the numeric App id; GitHub
/// accepts it as an integer.
#[derive(Debug, Serialize)]
struct Claims {
    iss: i64,
    iat: i64,
    exp: i64,
}

/// Sign a GitHub App JWT (RS256) for `app_id` using the RSA `pem`, relative to
/// `now`. Fails if the PEM is OpenSSH-formatted or not a valid RSA key.
pub fn app_jwt(app_id: i64, pem: &str, now: DateTime<Utc>) -> Result<String, CoreError> {
    if pem.contains("OPENSSH PRIVATE KEY") {
        return Err(CoreError::Jwt(
            "expected an RSA PEM private key, got an OpenSSH key".into(),
        ));
    }
    let key = EncodingKey::from_rsa_pem(pem.as_bytes())
        .map_err(|e| CoreError::Jwt(format!("invalid RSA private key: {e}")))?;
    let claims = Claims {
        iss: app_id,
        iat: (now - Duration::seconds(60)).timestamp(),
        exp: (now + Duration::minutes(8)).timestamp(),
    };
    encode(&Header::new(Algorithm::RS256), &claims, &key)
        .map_err(|e| CoreError::Jwt(format!("failed to sign jwt: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    use chrono::TimeZone;

    // A 2048-bit RSA private key (PKCS#1) generated solely for these unit tests;
    // it is not used anywhere outside this module.
    const TEST_RSA_PEM: &str = include_str!("../test-data/github_app_test_key.pem");

    fn decode_part(part: &str) -> serde_json::Value {
        let bytes = B64URL.decode(part).expect("valid base64url");
        serde_json::from_slice(&bytes).expect("valid json")
    }

    #[test]
    fn rejects_openssh_keys() {
        let openssh = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----";
        let err = app_jwt(42, openssh, Utc::now()).unwrap_err();
        assert!(matches!(err, CoreError::Jwt(_)));
    }

    #[test]
    fn rejects_garbage_pem() {
        assert!(app_jwt(1, "not a pem", Utc::now()).is_err());
    }

    #[test]
    fn claims_are_golden_rs256() {
        let now = Utc.with_ymd_and_hms(2026, 7, 3, 12, 0, 0).unwrap();
        let token = app_jwt(123456, TEST_RSA_PEM, now).unwrap();

        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "jwt has header.payload.signature");

        let header = decode_part(parts[0]);
        assert_eq!(header["alg"], "RS256");
        assert_eq!(header["typ"], "JWT");

        let claims = decode_part(parts[1]);
        assert_eq!(claims["iss"], 123456);
        // iat = now - 60s ; exp = now + 8min (parity with github.php:41-42).
        assert_eq!(claims["iat"].as_i64().unwrap(), now.timestamp() - 60);
        assert_eq!(claims["exp"].as_i64().unwrap(), now.timestamp() + 8 * 60);
        assert!(!parts[2].is_empty(), "signature present");
    }
}
