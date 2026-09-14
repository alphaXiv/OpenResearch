//! Shared bearer-token comparison for loopback surfaces that gate on a
//! pre-shared secret (the `orx up --remote` session bearer, `orx serve
//! --token`). Tokens are always compared as SHA-256 digests, never as raw
//! bytes: this keeps the token itself out of process memory diffs/core dumps
//! at the comparison site and makes every call site constant-time by
//! construction rather than by discipline.

use sha2::{Digest as _, Sha256};

/// SHA-256 digest of a token/secret, as a fixed-size array so callers can
/// store and compare digests without re-hashing.
pub(crate) fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

/// Constant-time equality for two digests (or any equal-length byte
/// sequences). Returns `false` immediately on a length mismatch — safe here
/// because both operands are always fixed-size digests, so the length itself
/// leaks nothing timing-sensitive.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_value_digests_equal() {
        assert_eq!(digest("secret"), digest("secret"));
    }

    #[test]
    fn different_values_digest_differently() {
        assert_ne!(digest("secret"), digest("different"));
    }

    #[test]
    fn constant_time_eq_matches_equal_digests() {
        let a = digest("token-123");
        let b = digest("token-123");
        assert!(constant_time_eq(&a, &b));
    }

    #[test]
    fn constant_time_eq_rejects_mismatched_digests() {
        let a = digest("token-123");
        let b = digest("token-456");
        assert!(!constant_time_eq(&a, &b));
    }

    #[test]
    fn constant_time_eq_rejects_length_mismatch() {
        assert!(!constant_time_eq(b"short", b"a-longer-slice"));
    }
}
