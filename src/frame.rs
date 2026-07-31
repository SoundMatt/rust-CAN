// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! CAN frame types and validation per RELAY spec §15.1.

use serde::{Deserialize, Serialize};

use crate::error::Error;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum data length for a classic CAN frame.
//fusa:req REQ-CAN-013
pub const CAN_MAX_DATA_LEN: usize = 8;

/// Maximum data length for a CAN FD frame.
//fusa:req REQ-CAN-013
pub const CAN_FD_MAX_DATA_LEN: usize = 64;

/// Canonical CAN FD data lengths per the ISO 11898-1 DLC table.
///
/// DLC 0–8 map 1:1 to 0–8 data bytes; DLC 9–15 map to the fixed lengths
/// 12, 16, 20, 24, 32, 48, 64. Any other byte count (e.g. 9, 10, 11, 13,
/// 20-31 excluded values, ...) has no DLC encoding: a conformant CAN FD
/// controller/driver (including Linux SocketCAN) silently rounds such a
/// length up to the next canonical value and zero-pads on the wire, so a
/// peer receives a different length than the sender specified.
//fusa:req REQ-CAN-013
pub const CAN_FD_CANONICAL_DATA_LENS: [usize; 16] =
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];

/// Returns true if `len` is a canonical CAN FD data length (i.e. directly
/// representable by a CAN FD DLC value per ISO 11898-1).
pub fn is_canonical_fd_data_len(len: usize) -> bool {
    CAN_FD_CANONICAL_DATA_LENS.contains(&len)
}

/// Minimum data length for a CAN XL frame.
//fusa:req REQ-CAN-013
pub const CAN_XL_MIN_DATA_LEN: usize = 1;

/// Maximum data length for a CAN XL frame.
//fusa:req REQ-CAN-013
pub const CAN_XL_MAX_DATA_LEN: usize = 2048;

/// Maximum value for a standard (11-bit) CAN ID.
//fusa:req REQ-CAN-009
pub const CAN_MAX_STD_ID: u32 = 0x7FF;

/// Maximum value for an extended (29-bit) CAN ID.
//fusa:req REQ-CAN-010
pub const CAN_MAX_EXT_ID: u32 = 0x1FFF_FFFF;

/// Maximum value for a CAN XL Priority ID (11-bit).
pub const CAN_XL_MAX_PRIO_ID: u32 = 0x7FF;

// ---------------------------------------------------------------------------
// Serde helpers
// ---------------------------------------------------------------------------

fn is_false(b: &bool) -> bool {
    !b
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// A CAN, CAN FD, or CAN XL frame per RELAY spec §15.1.
//fusa:req REQ-CAN-001
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Frame {
    /// Arbitration ID. Standard IDs are 11 bits (0–0x7FF); extended are 29
    /// bits (0–0x1FFFFFFF). CAN XL uses an 11-bit Priority ID.
    pub id: u32,

    /// Extended format (29-bit ID).
    #[serde(default, skip_serializing_if = "is_false")]
    pub ext: bool,

    /// Remote Transmission Request. Must be false for FD and XL frames.
    ///
    /// Per ISO 11898-1, an RTR frame carries no payload bytes on the wire,
    /// but its DLC field still encodes the length of the data frame being
    /// solicited. Transports in this crate derive the outgoing DLC from
    /// `data.len()` even when `rtr` is set, so a caller requesting an
    /// N-byte reply MUST set `data` to a `Vec` of length N (the byte
    /// *values* are ignored and never placed on the wire) — see
    /// [`Frame::remote_request`] for a convenience constructor. Leaving
    /// `data` empty on an RTR frame requests a 0-byte reply.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rtr: bool,

    /// CAN FD format (payload up to 64 bytes).
    #[serde(default, skip_serializing_if = "is_false")]
    pub fd: bool,

    /// Bit Rate Switch (CAN FD only). Must be false when fd=false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub brs: bool,

    /// Error State Indicator (CAN FD and CAN XL). Must be false unless fd or xl.
    #[serde(default, skip_serializing_if = "is_false")]
    pub esi: bool,

    /// CAN XL format (payload 1–2048 bytes). Mutually exclusive with fd.
    #[serde(default, skip_serializing_if = "is_false")]
    pub xl: bool,

    /// SDU Type (CAN XL only).
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub sdt: u8,

    /// Virtual CAN network ID (CAN XL only).
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub vcid: u8,

    /// Acceptance Field (CAN XL only).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub af: u32,

    /// Simple Extended Content flag (CAN XL only).
    #[serde(default, skip_serializing_if = "is_false")]
    pub sec: bool,

    /// Frame payload — base64-encoded in JSON (RELAY spec §15.1).
    #[serde(with = "crate::base64_serde")]
    pub data: Vec<u8>,
}

impl Frame {
    /// Returns the maximum data length for this frame's format.
    ///
    /// - CAN XL: 2048 bytes
    /// - CAN FD: 64 bytes
    /// - Classic CAN: 8 bytes
    pub fn max_data_len(&self) -> usize {
        if self.xl {
            CAN_XL_MAX_DATA_LEN
        } else if self.fd {
            CAN_FD_MAX_DATA_LEN
        } else {
            CAN_MAX_DATA_LEN
        }
    }

    /// Builds a classic CAN Remote Transmission Request (RTR) frame that
    /// solicits a `requested_len`-byte reply.
    ///
    /// Per ISO 11898-1, an RTR frame carries no payload on the wire, but
    /// its DLC field still conveys the requested data length. This crate's
    /// transports derive the outgoing DLC from `data.len()`, so this
    /// constructor fills `data` with `requested_len` placeholder zero
    /// bytes — their *values* are never transmitted, only the length.
    /// `requested_len` MUST be ≤ [`CAN_MAX_DATA_LEN`] (8); RTR is not valid
    /// on CAN FD or CAN XL frames.
    pub fn remote_request(id: u32, ext: bool, requested_len: usize) -> Frame {
        Frame {
            id,
            ext,
            rtr: true,
            data: vec![0u8; requested_len.min(CAN_MAX_DATA_LEN)],
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

/// A content filter for CAN frames per RELAY spec §15.1.
///
/// A frame passes the filter when `(frame.id & mask) == (id & mask)`.
/// A zero-value `Filter{}` passes all frames.
//fusa:req REQ-CAN-002
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Filter {
    pub id: u32,
    pub mask: u32,
}

impl Filter {
    /// Returns true if `fr` passes this filter.
    pub fn matches(&self, fr: &Frame) -> bool {
        (fr.id & self.mask) == (self.id & self.mask)
    }
}

// ---------------------------------------------------------------------------
// LoanedFrame
// ---------------------------------------------------------------------------

/// A frame with an optional release callback for zero-copy buffer pooling.
pub struct LoanedFrame {
    pub frame: Frame,
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl LoanedFrame {
    /// Create a loaned frame with a release callback.
    pub fn new(frame: Frame, release: impl FnOnce() + Send + 'static) -> Self {
        Self {
            frame,
            release: Some(Box::new(release)),
        }
    }

    /// Create a loaned frame with no release callback.
    pub fn simple(frame: Frame) -> Self {
        Self {
            frame,
            release: None,
        }
    }

    /// Consume the frame and invoke the release callback (if any).
    pub fn return_loan(mut self) {
        if let Some(f) = self.release.take() {
            f();
        }
    }
}

impl std::fmt::Debug for LoanedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoanedFrame")
            .field("frame", &self.frame)
            .field("release", &self.release.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// validate_frame
// ---------------------------------------------------------------------------

/// Validates a CAN frame against RELAY spec §15.1 constraints.
///
/// Returns `Error::InvalidFrame` for any structural violation.
//fusa:req REQ-CAN-004
//fusa:req REQ-SEC-001
//fusa:req REQ-CAN-009
//fusa:req REQ-CAN-010
//fusa:req REQ-CAN-011
//fusa:req REQ-CAN-012
//fusa:req REQ-CAN-013
//fusa:req REQ-CAN-014
pub fn validate_frame(f: &Frame) -> Result<(), Error> {
    // XL and FD are mutually exclusive.
    if f.xl && f.fd {
        return Err(Error::invalid_frame("XL and FD are mutually exclusive"));
    }

    if f.xl {
        // CAN XL constraints.
        if f.ext {
            return Err(Error::invalid_frame("CAN XL frame must not set Ext"));
        }
        if f.rtr {
            return Err(Error::invalid_frame("CAN XL frame must not set RTR"));
        }
        if f.brs {
            return Err(Error::invalid_frame("CAN XL frame must not set BRS"));
        }
        if f.id > CAN_XL_MAX_PRIO_ID {
            return Err(Error::invalid_frame(format!(
                "CAN XL Priority ID 0x{:X} exceeds 0x7FF",
                f.id
            )));
        }
        if f.data.is_empty() {
            return Err(Error::invalid_frame(
                "CAN XL frame must carry at least 1 byte",
            ));
        }
        if f.data.len() > CAN_XL_MAX_DATA_LEN {
            return Err(Error::invalid_frame(format!(
                "CAN XL data length {} exceeds 2048",
                f.data.len()
            )));
        }
        return Ok(());
    }

    // Standard and extended ID range checks.
    if f.ext {
        //fusa:req REQ-CAN-010
        if f.id > CAN_MAX_EXT_ID {
            return Err(Error::invalid_frame(format!(
                "extended ID 0x{:X} exceeds 0x1FFFFFFF",
                f.id
            )));
        }
    } else {
        //fusa:req REQ-CAN-009
        if f.id > CAN_MAX_STD_ID {
            return Err(Error::invalid_frame(format!(
                "standard ID 0x{:X} exceeds 0x7FF",
                f.id
            )));
        }
    }

    //fusa:req REQ-CAN-011
    if f.brs && !f.fd {
        return Err(Error::invalid_frame("BRS requires FD=true"));
    }

    //fusa:req REQ-CAN-012
    if f.rtr && f.fd {
        return Err(Error::invalid_frame("RTR must be false when FD=true"));
    }

    // ESI must be false unless FD or XL.
    if f.esi && !f.fd && !f.xl {
        return Err(Error::invalid_frame("ESI requires FD or XL"));
    }

    //fusa:req REQ-CAN-013
    if f.fd {
        if f.data.len() > CAN_FD_MAX_DATA_LEN {
            return Err(Error::invalid_frame(format!(
                "CAN FD data length {} exceeds 64",
                f.data.len()
            )));
        }
        // ISO 11898-1: CAN FD DLC values only encode the lengths in
        // CAN_FD_CANONICAL_DATA_LENS. A non-canonical length (e.g. 20's
        // neighbor 21) is not representable on the wire; conformant
        // hardware/kernel stacks (including Linux SocketCAN) silently
        // round it up and zero-pad, so the receiver sees a different
        // length than the sender specified. Reject it here rather than
        // let it reach the transport layer.
        if !is_canonical_fd_data_len(f.data.len()) {
            return Err(Error::invalid_frame(format!(
                "CAN FD data length {} is not a canonical DLC length \
                 (must be one of 0-8, 12, 16, 20, 24, 32, 48, 64)",
                f.data.len()
            )));
        }
    } else if f.data.len() > CAN_MAX_DATA_LEN {
        return Err(Error::invalid_frame(format!(
            "classic CAN data length {} exceeds 8",
            f.data.len()
        )));
    }

    Ok(())
}

/// Returns the maximum data length for the given frame type.
///
/// Returns 64 for FD frames, 8 for classic frames.
pub fn max_data_len(fd: bool) -> usize {
    if fd {
        CAN_FD_MAX_DATA_LEN
    } else {
        CAN_MAX_DATA_LEN
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_standard_frame() {
        let f = Frame {
            id: 0x100,
            data: vec![1, 2, 3, 4],
            ..Default::default()
        };
        assert!(validate_frame(&f).is_ok());
    }

    #[test]
    fn valid_extended_frame() {
        let f = Frame {
            id: 0x1234_5678,
            ext: true,
            data: vec![0xFF; 8],
            ..Default::default()
        };
        assert!(validate_frame(&f).is_ok());
    }

    #[test]
    fn valid_fd_frame() {
        let f = Frame {
            id: 0x100,
            fd: true,
            brs: true,
            data: vec![0u8; 64],
            ..Default::default()
        };
        assert!(validate_frame(&f).is_ok());
    }

    /// ISO 11898-1: every canonical CAN FD DLC length (0-8, 12, 16, 20, 24,
    /// 32, 48, 64) must validate successfully.
    //fusa:test REQ-CAN-013
    #[test]
    fn fd_frame_all_canonical_lengths_accepted() {
        for &len in CAN_FD_CANONICAL_DATA_LENS.iter() {
            let f = Frame {
                id: 0x100,
                fd: true,
                data: vec![0u8; len],
                ..Default::default()
            };
            assert!(
                validate_frame(&f).is_ok(),
                "canonical FD length {len} should validate"
            );
        }
    }

    /// Non-canonical CAN FD lengths (e.g. 20's neighbors 9-11, 13-15, 17-19,
    /// 21-23, ...) have no DLC encoding in ISO 11898-1. A conformant
    /// SocketCAN driver silently rounds these up and zero-pads on the wire,
    /// so the receiver sees a different length than the sender specified —
    /// this must be rejected by `validate_frame` rather than reach the
    /// transport layer. Regression test for the missing canonicality check.
    //fusa:test REQ-CAN-013
    #[test]
    fn fd_frame_non_canonical_lengths_rejected() {
        for len in [
            9usize, 10, 11, 13, 14, 15, 17, 18, 19, 21, 25, 33, 47, 49, 63,
        ] {
            let f = Frame {
                id: 0x100,
                fd: true,
                data: vec![0u8; len],
                ..Default::default()
            };
            assert!(
                matches!(validate_frame(&f), Err(Error::InvalidFrame { .. })),
                "non-canonical FD length {len} should be rejected"
            );
        }
    }

    #[test]
    fn valid_xl_frame() {
        let f = Frame {
            id: 0x7FF,
            xl: true,
            data: vec![0u8; 2048],
            ..Default::default()
        };
        assert!(validate_frame(&f).is_ok());
    }

    #[test]
    fn standard_id_too_large() {
        let f = Frame {
            id: 0x800,
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[test]
    fn extended_id_too_large() {
        let f = Frame {
            id: 0x2000_0000,
            ext: true,
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[test]
    fn brs_without_fd_rejected() {
        let f = Frame {
            id: 0x100,
            brs: true,
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[test]
    fn rtr_with_fd_rejected() {
        let f = Frame {
            id: 0x100,
            fd: true,
            rtr: true,
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    /// `Frame::remote_request` must produce an RTR frame whose `data.len()`
    /// (the value transports use to derive the outgoing DLC) equals the
    /// requested length — this is the signal that a classic RTR send must
    /// not drop. Regression test for the "RTR frame drops the requested
    /// DLC" finding: prior to documenting/constructing this correctly, the
    /// natural (but wrong) way to build an RTR frame left `data` empty,
    /// forcing DLC=0 regardless of the length the caller intended to
    /// solicit.
    //fusa:test REQ-CAN-012
    #[test]
    fn remote_request_preserves_requested_dlc() {
        for len in 0..=8usize {
            let f = Frame::remote_request(0x123, false, len);
            assert!(f.rtr);
            assert!(!f.fd);
            assert_eq!(
                f.data.len(),
                len,
                "remote_request({len}) must preserve the requested DLC in data.len()"
            );
            assert!(validate_frame(&f).is_ok());
        }
    }

    /// A caller-supplied `requested_len` above the classic CAN max (8) must
    /// be clamped, not overflow into an invalid frame.
    #[test]
    fn remote_request_clamps_oversized_length() {
        let f = Frame::remote_request(0x123, false, 20);
        assert_eq!(f.data.len(), CAN_MAX_DATA_LEN);
        assert!(validate_frame(&f).is_ok());
    }

    #[test]
    fn data_too_long_classic() {
        let f = Frame {
            id: 0x100,
            data: vec![0u8; 9],
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[test]
    fn data_too_long_fd() {
        let f = Frame {
            id: 0x100,
            fd: true,
            data: vec![0u8; 65],
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[test]
    fn xl_and_fd_rejected() {
        let f = Frame {
            id: 0x100,
            xl: true,
            fd: true,
            data: vec![0u8; 8],
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[test]
    fn xl_ext_rejected() {
        let f = Frame {
            id: 0x100,
            xl: true,
            ext: true,
            data: vec![0u8; 8],
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[test]
    fn xl_priority_id_too_large() {
        let f = Frame {
            id: 0x800,
            xl: true,
            data: vec![0u8; 8],
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    //fusa:test REQ-CAN-013
    #[test]
    fn xl_data_too_large() {
        let f = Frame {
            id: 0x100,
            xl: true,
            data: vec![0u8; 2049],
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[test]
    fn esi_without_fd_rejected() {
        let f = Frame {
            id: 0x100,
            esi: true,
            ..Default::default()
        };
        assert!(matches!(
            validate_frame(&f),
            Err(Error::InvalidFrame { .. })
        ));
    }

    //fusa:test REQ-CAN-002
    #[test]
    fn filter_matches() {
        let f = Frame {
            id: 0x100,
            ..Default::default()
        };
        let pass = Filter {
            id: 0x100,
            mask: 0x7FF,
        };
        let miss = Filter {
            id: 0x200,
            mask: 0x7FF,
        };
        let all = Filter { id: 0, mask: 0 };

        assert!(pass.matches(&f));
        assert!(!miss.matches(&f));
        assert!(all.matches(&f));
    }

    #[test]
    fn frame_max_data_len() {
        let classic = Frame::default();
        assert_eq!(classic.max_data_len(), 8);

        let fd = Frame {
            fd: true,
            ..Default::default()
        };
        assert_eq!(fd.max_data_len(), 64);

        let xl = Frame {
            xl: true,
            ..Default::default()
        };
        assert_eq!(xl.max_data_len(), 2048);
    }

    #[test]
    fn loaned_frame_release_called() {
        use std::sync::{Arc, Mutex};
        let released = Arc::new(Mutex::new(false));
        let r = released.clone();
        let lf = LoanedFrame::new(Frame::default(), move || {
            *r.lock().unwrap() = true;
        });
        lf.return_loan();
        assert!(*released.lock().unwrap());
    }

    #[test]
    fn max_data_len_fn() {
        assert_eq!(max_data_len(false), 8);
        assert_eq!(max_data_len(true), 64);
    }
}
