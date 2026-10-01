//! Zone entry tokens (HUB.md 3.1): ed25519 over the bitcode-encoded payload, 60 s, single
//! use, zone-bound. The hub signs; zones verify offline and remember accepted nonces until
//! they expire.

use std::collections::HashMap;
use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::protocol::{
    CLOCK_SKEW_SECS, SessionToken, TOKEN_VALID_SECS, TokenPayload, ZoneId, now_secs,
};

/// The hub's signing identity, persisted so a restart does not invalidate tickets.
pub struct HubKey {
    key: SigningKey,
}

impl HubKey {
    /// Load the 32-byte seed from `path`, or create and write a fresh one.
    pub fn load_or_create(path: &Path) -> anyhow::Result<HubKey> {
        if path.is_file() {
            let bytes = std::fs::read(path)?;
            let seed: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("{} is not a 32-byte key", path.display()))?;
            return Ok(HubKey {
                key: SigningKey::from_bytes(&seed),
            });
        }
        let key = HubKey::generate();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, key.key.to_bytes())?;
        Ok(key)
    }

    pub fn generate() -> HubKey {
        let mut seed = [0u8; 32];
        rand::fill(&mut seed);
        HubKey {
            key: SigningKey::from_bytes(&seed),
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    pub fn issue(&self, account: i64, character: i64, zone: &str) -> SessionToken {
        let mut nonce = [0u8; 16];
        rand::fill(&mut nonce);
        let now = now_secs();
        let payload = TokenPayload {
            account,
            character,
            zone: zone.to_string(),
            issued_at: now,
            expires_at: now + TOKEN_VALID_SECS,
            nonce,
        };
        let signature = self.key.sign(&bitcode::encode(&payload)).to_bytes();
        SessionToken { payload, signature }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TokenError {
    BadSignature,
    WrongZone,
    Expired,
    Replayed,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::BadSignature => write!(f, "bad token signature"),
            TokenError::WrongZone => write!(f, "token is for another zone"),
            TokenError::Expired => write!(f, "token expired"),
            TokenError::Replayed => write!(f, "token already used"),
        }
    }
}

/// Zone-side verifier with the single-use nonce memory.
pub struct TokenVerifier {
    public: VerifyingKey,
    zone: ZoneId,
    /// nonce → expiry; swept on every check.
    seen: HashMap<[u8; 16], u64>,
}

impl TokenVerifier {
    pub fn new(public_key: [u8; 32], zone: &str) -> anyhow::Result<TokenVerifier> {
        Ok(TokenVerifier {
            public: VerifyingKey::from_bytes(&public_key)?,
            zone: zone.to_string(),
            seen: HashMap::new(),
        })
    }

    pub fn set_public_key(&mut self, public_key: [u8; 32]) -> anyhow::Result<()> {
        self.public = VerifyingKey::from_bytes(&public_key)?;
        Ok(())
    }

    /// Verify and consume a token at `now` (unix seconds).
    pub fn accept(&mut self, token: &SessionToken, now: u64) -> Result<TokenPayload, TokenError> {
        let sig = Signature::from_bytes(&token.signature);
        self.public
            .verify(&bitcode::encode(&token.payload), &sig)
            .map_err(|_| TokenError::BadSignature)?;
        let p = &token.payload;
        if p.zone != self.zone {
            return Err(TokenError::WrongZone);
        }
        if now > p.expires_at + CLOCK_SKEW_SECS || p.issued_at > now + CLOCK_SKEW_SECS {
            return Err(TokenError::Expired);
        }
        self.seen.retain(|_, exp| *exp + CLOCK_SKEW_SECS >= now);
        if self.seen.contains_key(&p.nonce) {
            return Err(TokenError::Replayed);
        }
        self.seen.insert(p.nonce, p.expires_at);
        Ok(p.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_zone_bound_single_use_and_expire() {
        let hub = HubKey::generate();
        let mut arena = TokenVerifier::new(hub.public_key(), "arena").unwrap();
        let mut town = TokenVerifier::new(hub.public_key(), "town").unwrap();
        let t = hub.issue(1, 7, "arena");
        let now = now_secs();
        assert_eq!(town.accept(&t, now), Err(TokenError::WrongZone));
        let p = arena.accept(&t, now).unwrap();
        assert_eq!((p.account, p.character), (1, 7));
        assert_eq!(arena.accept(&t, now), Err(TokenError::Replayed));
        let t2 = hub.issue(1, 7, "arena");
        assert_eq!(
            arena.accept(&t2, now + TOKEN_VALID_SECS + CLOCK_SKEW_SECS + 1),
            Err(TokenError::Expired)
        );
        assert!(arena.accept(&t2, now + TOKEN_VALID_SECS).is_ok());
        // Another hub's signature is refused.
        let other = HubKey::generate();
        let forged = other.issue(1, 7, "arena");
        assert_eq!(arena.accept(&forged, now), Err(TokenError::BadSignature));
        // A tampered payload is refused.
        let mut tampered = hub.issue(1, 7, "arena");
        tampered.payload.character = 8;
        assert_eq!(arena.accept(&tampered, now), Err(TokenError::BadSignature));
    }

    #[test]
    fn keys_persist() {
        let dir = std::env::temp_dir().join(format!("gm-hub-key-{}", std::process::id()));
        let path = dir.join("hub.key");
        let a = HubKey::load_or_create(&path).unwrap();
        let b = HubKey::load_or_create(&path).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
