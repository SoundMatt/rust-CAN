// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Unified error type for rust-CAN.
//!
//! Wraps the four mandatory RELAY sentinels (§5.1) and protocol-specific
//! CAN errors (§5.4).

use thiserror::Error;

/// The unified error type for all rust-CAN operations.
///
/// Protocol-specific errors wrap the RELAY sentinel they correspond to so
/// that callers can use `matches!(err, Error::Closed)` style checks.
//fusa:req REQ-CAN-005
#[derive(Debug, Error)]
pub enum Error {
    /// Operation on a closed bus or subscription.
    #[error("relay: closed")]
    Closed,

    /// Operation before the bus is connected.
    #[error("relay: not connected")]
    NotConnected,

    /// Operation timed out.
    #[error("relay: timeout")]
    Timeout,

    /// Payload exceeds the protocol maximum.
    #[error("relay: payload too large")]
    PayloadTooLarge,

    /// CAN frame failed structural validation per §15.1.
    //fusa:req REQ-CAN-004
    #[error("can: invalid frame: {reason}")]
    InvalidFrame { reason: String },

    /// Generic CAN error with a message string.
    #[error("can: {0}")]
    Other(String),

    /// Underlying I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<crate::relay::Error> for Error {
    fn from(e: crate::relay::Error) -> Self {
        match e {
            crate::relay::Error::Closed => Error::Closed,
            crate::relay::Error::NotConnected => Error::NotConnected,
            crate::relay::Error::Timeout => Error::Timeout,
            crate::relay::Error::PayloadTooLarge => Error::PayloadTooLarge,
        }
    }
}

impl Error {
    /// Return the RELAY sentinel this error maps to, if any.
    pub fn kind(&self) -> Option<crate::relay::Error> {
        match self {
            Error::Closed => Some(crate::relay::Error::Closed),
            Error::NotConnected => Some(crate::relay::Error::NotConnected),
            Error::Timeout => Some(crate::relay::Error::Timeout),
            Error::PayloadTooLarge => Some(crate::relay::Error::PayloadTooLarge),
            _ => None,
        }
    }

    /// Convenience constructor for an `InvalidFrame` error.
    pub fn invalid_frame(reason: impl Into<String>) -> Self {
        Error::InvalidFrame {
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_error_conversion() {
        let e: Error = crate::relay::Error::Closed.into();
        assert!(matches!(e, Error::Closed));
        assert_eq!(e.kind(), Some(crate::relay::Error::Closed));
    }

    #[test]
    fn invalid_frame_kind_is_none() {
        let e = Error::invalid_frame("bad id");
        assert!(e.kind().is_none());
    }

    #[test]
    fn error_display() {
        let e = Error::Closed;
        assert_eq!(e.to_string(), "relay: closed");

        let e2 = Error::InvalidFrame {
            reason: "test".into(),
        };
        assert_eq!(e2.to_string(), "can: invalid frame: test");
    }
}
