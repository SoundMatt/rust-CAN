// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! HMAC-SHA256 implementation of [`MessageAuthenticator`].
//!
//! Enabled by the `hmac-auth` feature flag.
//!
//! # Security properties
//!
//! - **Key length:** HMAC-SHA256 accepts any key length; recommend ≥ 32 bytes
//!   (256 bits) from a cryptographically secure random source or an HSM.
//! - **Tag length:** 32 bytes (256 bits). Provides 128-bit security against
//!   collision and forgery under the random oracle model.
//! - **Timing:** Tag comparison uses `verify_slice()` from the `hmac` crate,
//!   which performs a constant-time comparison to prevent timing side-channels.
//! - **Standard:** FIPS 198-1 / RFC 2104; satisfies IEC 62443 SL-2 and
//!   ISO/SAE 21434 CAL-3 integrity requirements (REQ-SEC-006).
//!
//! # Usage
//!
//! ```rust,no_run
//! # #[cfg(feature = "hmac-auth")]
//! # {
//! use rust_can::safety::{Config, MessageAuthenticator, Protector, Receiver};
//! use rust_can::safety::hmac_auth::HmacSha256Auth;
//!
//! let key = b"my-32-byte-secret-key-here!!!!!!";
//! let auth = HmacSha256Auth;
//!
//! let payload = b"brake-command";
//! let tag = auth.sign(key, payload);
//! assert!(auth.verify(key, payload, &tag));
//! # }
//! ```

use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::MessageAuthenticator;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256 message authenticator.
///
/// See module-level documentation for security properties and usage.
//fusa:req REQ-SEC-006
pub struct HmacSha256Auth;

impl MessageAuthenticator for HmacSha256Auth {
    fn sign(&self, key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    fn verify(&self, key: &[u8], data: &[u8], tag: &[u8]) -> bool {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
        mac.update(data);
        // constant-time comparison — prevents timing side-channels
        mac.verify_slice(tag).is_ok()
    }

    fn tag_len(&self) -> usize {
        32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    //fusa:test REQ-SEC-006
    #[test]
    fn hmac_sign_verify_roundtrip() {
        let auth = HmacSha256Auth;
        let key = b"test-key-32-bytes-padding-padpad";
        let data = b"safety-critical-payload";
        let tag = auth.sign(key, data);
        assert_eq!(tag.len(), 32);
        assert!(auth.verify(key, data, &tag));
    }

    //fusa:test REQ-SEC-006
    #[test]
    fn hmac_wrong_key_rejected() {
        let auth = HmacSha256Auth;
        let key = b"correct-key-32-bytes-padding-pad";
        let bad_key = b"wrong-key-32-bytes-padding-paddd";
        let data = b"payload";
        let tag = auth.sign(key, data);
        assert!(!auth.verify(bad_key, data, &tag));
    }

    //fusa:test REQ-SEC-006
    #[test]
    fn hmac_tampered_data_rejected() {
        let auth = HmacSha256Auth;
        let key = b"key-32-bytes-padding-paddingpadd";
        let data = b"original";
        let tag = auth.sign(key, data);
        assert!(!auth.verify(key, b"tampered", &tag));
    }

    //fusa:test REQ-SEC-006
    #[test]
    fn hmac_tag_len() {
        assert_eq!(HmacSha256Auth.tag_len(), 32);
    }
}
