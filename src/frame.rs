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

    /// Frame payload.
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
//fusa:req REQ-CAN-004, REQ-CAN-009, REQ-CAN-010, REQ-CAN-011, REQ-CAN-012,
//fusa:req REQ-CAN-013, REQ-CAN-014
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

    //fusa:req REQ-CAN-011: BRS requires FD.
    if f.brs && !f.fd {
        return Err(Error::invalid_frame("BRS requires FD=true"));
    }

    //fusa:req REQ-CAN-012: RTR must be false when FD is true.
    if f.rtr && f.fd {
        return Err(Error::invalid_frame("RTR must be false when FD=true"));
    }

    // ESI must be false unless FD or XL.
    if f.esi && !f.fd && !f.xl {
        return Err(Error::invalid_frame("ESI requires FD or XL"));
    }

    //fusa:req REQ-CAN-013: Data length constraints.
    if f.fd {
        if f.data.len() > CAN_FD_MAX_DATA_LEN {
            return Err(Error::invalid_frame(format!(
                "CAN FD data length {} exceeds 64",
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
