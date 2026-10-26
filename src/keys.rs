// SPDX-License-Identifier: AGPL-3.0-only
//! Catalog signing keys. The two pinned Ed25519 public keys (base64); a remote
//! catalog must verify against one of them. These are development keys that
//! will be replaced with the release keys in the packaging handoff.
pub const CATALOG_PUBLIC_KEYS: [&str; 2] = [
    "Bs0jQMth9DtZ/ct5dLmChtnPHsRnAb4izpXmECjW31s=",
    "Bs0jQMth9DtZ/ct5dLmChtnPHsRnAb4izpXmECjW31s=",
];
