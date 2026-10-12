//! MusicKit developer token: an ES256 JWT signed with the `.p8` private key
//! from the user's Apple Developer account (Certificates, IDs & Profiles →
//! Keys → MusicKit).
//!
//! Header `{alg: ES256, kid: <Key ID>}`, claims `{iss: <Team ID>, iat, exp}`.
//! Apple caps `exp - iat` at 15777000 s (6 months).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;

use crate::Error;

/// Longest lifetime Apple accepts for a developer token.
pub const MAX_TTL_SECS: u64 = 15_777_000;

pub struct DeveloperKey {
    pub team_id: String,
    pub key_id: String,
    key: SigningKey,
}

impl DeveloperKey {
    /// `pem` is the content of `AuthKey_<KEYID>.p8` (PKCS#8, `BEGIN PRIVATE KEY`).
    pub fn from_pem(team_id: &str, key_id: &str, pem: &str) -> Result<Self, Error> {
        let key = SigningKey::from_pkcs8_pem(pem.trim()).map_err(|e| Error::Key(e.to_string()))?;
        Ok(Self { team_id: team_id.trim().to_owned(), key_id: key_id.trim().to_owned(), key })
    }

    /// Signs a token valid from `now` (Unix seconds) for `ttl` seconds.
    pub fn sign(&self, now: u64, ttl: u64) -> String {
        let header = serde_json::json!({ "alg": "ES256", "kid": self.key_id });
        let claims = serde_json::json!({
            "iss": self.team_id,
            "iat": now,
            "exp": now + ttl.min(MAX_TTL_SECS),
        });
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        // JWS wants the raw 64-byte r||s form, not DER.
        let sig: Signature = self.key.sign(signing_input.as_bytes());
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig.to_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::VerifyingKey;
    use p256::pkcs8::{EncodePrivateKey, LineEnding};

    fn test_key() -> SigningKey {
        SigningKey::from_slice(&[7u8; 32]).unwrap()
    }

    #[test]
    fn signs_a_verifiable_es256_jwt() {
        let pem = test_key().to_pkcs8_pem(LineEnding::LF).unwrap();
        let key = DeveloperKey::from_pem("TEAM123456", "KEY1234567", &pem).unwrap();
        let jwt = key.sign(1_000, 3_600);

        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "KEY1234567");
        assert_eq!(claims["iss"], "TEAM123456");
        assert_eq!(claims["iat"], 1_000);
        assert_eq!(claims["exp"], 4_600);

        let sig_bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        let sig = Signature::from_slice(&sig_bytes).unwrap();
        VerifyingKey::from(&test_key())
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .expect("signature verifies");
    }

    #[test]
    fn caps_ttl_at_six_months() {
        let pem = test_key().to_pkcs8_pem(LineEnding::LF).unwrap();
        let key = DeveloperKey::from_pem("T", "K", &pem).unwrap();
        let jwt = key.sign(0, u64::MAX / 2);
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(jwt.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["exp"], MAX_TTL_SECS);
    }

    #[test]
    fn rejects_garbage_key() {
        assert!(DeveloperKey::from_pem("T", "K", "not a key").is_err());
    }
}
