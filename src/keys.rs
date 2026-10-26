// SPDX-License-Identifier: AGPL-3.0-only
//! Catalog signing keys: the two pinned Ed25519 public keys (base64) a remote
//! catalog must verify against. Slot 0 is the release-CI key
//! (GitHub secret SHRUG_CATALOG_KEY); slot 1 is the offline backup, so one
//! can be rotated without stranding installs. The private halves live in CI
//! and offline storage, never in this repository.
pub const CATALOG_PUBLIC_KEYS: [&str; 2] = [
    "WOt3K9p5b1g8W39CuF0YOlNGSgTCpIGIRqHe54m86BY=",
    "S5/+KJLosw93mJZSZCBvSXw9dcQuOGUiug2BAz2dkNU=",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_keys_are_valid_and_differ() {
        use base64::Engine as _;
        let decoded: Vec<Vec<u8>> = CATALOG_PUBLIC_KEYS
            .iter()
            .map(|k| base64::engine::general_purpose::STANDARD.decode(k).unwrap())
            .collect();
        for k in &decoded {
            assert_eq!(k.len(), 32);
            ed25519_dalek::VerifyingKey::from_bytes(&k.clone().try_into().unwrap()).unwrap();
        }
        assert_ne!(decoded[0], decoded[1]);
    }
}
