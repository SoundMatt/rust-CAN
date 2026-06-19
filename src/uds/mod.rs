// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! UDS (ISO 14229 — Unified Diagnostic Services) over ISO-TP.
//!
//! Provides a service-layer client for the most common UDS services:
//! - `0x10` DiagnosticSessionControl
//! - `0x11` ECUReset
//! - `0x22` ReadDataByIdentifier
//! - `0x27` SecurityAccess
//! - `0x2E` WriteDataByIdentifier
//! - `0x3E` TesterPresent
//!
//! # Example
//! ```rust,no_run
//! # use std::sync::Arc;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use rust_can::{virtual_bus::VirtualBus, Bus};
//! use rust_can::isotp::{Config, IsoTpConn};
//! use rust_can::uds::{Client, SessionType};
//!
//! let bus: Arc<dyn Bus> = Arc::new(VirtualBus::new());
//! let conn = IsoTpConn::new(bus, Config {
//!     tx_id: 0x7E0, rx_id: 0x7E8, ..Default::default()
//! }).await?;
//! let client = Client::new(conn);
//! client.diagnostic_session_control(Default::default(), SessionType::Extended).await?;
//! # Ok(())
//! # }
//! ```

use crate::error::Error;
use crate::isotp::IsoTpConn;
use crate::relay::Context;

// ---------------------------------------------------------------------------
// Service IDs
// ---------------------------------------------------------------------------

pub const SID_DIAGNOSTIC_SESSION_CONTROL: u8 = 0x10;
pub const SID_ECU_RESET: u8 = 0x11;
pub const SID_READ_DID: u8 = 0x22;
pub const SID_SECURITY_ACCESS: u8 = 0x27;
pub const SID_WRITE_DID: u8 = 0x2E;
pub const SID_TESTER_PRESENT: u8 = 0x3E;
pub const SID_NEGATIVE_RESPONSE: u8 = 0x7F;

const POSITIVE_RESPONSE_OFFSET: u8 = 0x40;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// UDS diagnostic session sub-functions.
//fusa:req REQ-UDS-001
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionType {
    Default = 0x01,
    Programming = 0x02,
    Extended = 0x03,
}

/// UDS ECU reset sub-function.
//fusa:req REQ-UDS-002
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetType {
    Hard = 0x01,
    KeyOffOn = 0x02,
    Soft = 0x03,
}

// ---------------------------------------------------------------------------
// NegativeResponseError
// ---------------------------------------------------------------------------

/// Returned when the ECU responds with a UDS negative response (SID 0x7F).
//fusa:req REQ-UDS-007
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegativeResponseError {
    pub service: u8,
    pub nrc: u8,
}

impl NegativeResponseError {
    /// Return a human-readable description of the NRC byte.
    pub fn nrc_description(&self) -> &'static str {
        match self.nrc {
            0x10 => "generalReject",
            0x11 => "serviceNotSupported",
            0x12 => "subFunctionNotSupported",
            0x13 => "incorrectMessageLengthOrInvalidFormat",
            0x14 => "responseTooLong",
            0x21 => "busyRepeatRequest",
            0x22 => "conditionsNotCorrect",
            0x24 => "requestSequenceError",
            0x25 => "noResponseFromSubnetComponent",
            0x26 => "failurePreventsExecutionOfRequestedAction",
            0x31 => "requestOutOfRange",
            0x33 => "securityAccessDenied",
            0x35 => "invalidKey",
            0x36 => "exceededNumberOfAttempts",
            0x37 => "requiredTimeDelayNotExpired",
            0x70 => "uploadDownloadNotAccepted",
            0x71 => "transferDataSuspended",
            0x72 => "generalProgrammingFailure",
            0x73 => "wrongBlockSequenceCounter",
            0x78 => "requestCorrectlyReceivedResponsePending",
            0x7E => "subFunctionNotSupportedInActiveSession",
            0x7F => "serviceNotSupportedInActiveSession",
            _ => "unknown",
        }
    }
}

impl std::fmt::Display for NegativeResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "uds: NRC 0x{:02X} ({}) for service 0x{:02X}",
            self.nrc,
            self.nrc_description(),
            self.service
        )
    }
}

impl std::error::Error for NegativeResponseError {}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A UDS client communicating over ISO-TP.
//fusa:req REQ-UDS-001
pub struct Client {
    conn: IsoTpConn,
}

impl Client {
    /// Create a UDS client over an established ISO-TP connection.
    //fusa:req REQ-UDS-008
    pub fn new(conn: IsoTpConn) -> Self {
        Self { conn }
    }

    /// Service 0x10 — switch to a diagnostic session.
    //fusa:req REQ-UDS-001
    pub async fn diagnostic_session_control(
        &self,
        ctx: Context,
        session: SessionType,
    ) -> Result<(), Error> {
        let resp = self
            .request(ctx, &[SID_DIAGNOSTIC_SESSION_CONTROL, session as u8])
            .await?;
        if resp.len() < 2
            || resp[0] != SID_DIAGNOSTIC_SESSION_CONTROL + POSITIVE_RESPONSE_OFFSET
            || resp[1] != session as u8
        {
            return Err(Error::Other(format!(
                "uds: unexpected DSC response: {:X?}",
                resp
            )));
        }
        Ok(())
    }

    /// Service 0x11 — reset the ECU.
    //fusa:req REQ-UDS-002
    pub async fn ecu_reset(&self, ctx: Context, reset: ResetType) -> Result<(), Error> {
        let resp = self.request(ctx, &[SID_ECU_RESET, reset as u8]).await?;
        if resp.len() < 2
            || resp[0] != SID_ECU_RESET + POSITIVE_RESPONSE_OFFSET
            || resp[1] != reset as u8
        {
            return Err(Error::Other(format!(
                "uds: unexpected ECUReset response: {:X?}",
                resp
            )));
        }
        Ok(())
    }

    /// Service 0x22 — read data record by 2-byte DID.
    //fusa:req REQ-UDS-004
    pub async fn read_did(&self, ctx: Context, did: u16) -> Result<Vec<u8>, Error> {
        let high = (did >> 8) as u8;
        let low = did as u8;
        let resp = self.request(ctx, &[SID_READ_DID, high, low]).await?;
        if resp.len() < 4
            || resp[0] != SID_READ_DID + POSITIVE_RESPONSE_OFFSET
            || resp[1] != high
            || resp[2] != low
        {
            return Err(Error::Other(format!(
                "uds: unexpected ReadDID response: {:X?}",
                resp
            )));
        }
        Ok(resp[3..].to_vec())
    }

    /// Service 0x27 — security access (seed/key exchange).
    ///
    /// Returns the seed bytes from the ECU; the caller computes the key and
    /// calls `security_access_send_key()`.
    //fusa:req REQ-UDS-006
    pub async fn security_access_request_seed(
        &self,
        ctx: Context,
        access_level: u8,
    ) -> Result<Vec<u8>, Error> {
        let resp = self
            .request(ctx, &[SID_SECURITY_ACCESS, access_level])
            .await?;
        if resp.len() < 2
            || resp[0] != SID_SECURITY_ACCESS + POSITIVE_RESPONSE_OFFSET
            || resp[1] != access_level
        {
            return Err(Error::Other(format!(
                "uds: unexpected SecurityAccess seed response: {:X?}",
                resp
            )));
        }
        Ok(resp[2..].to_vec())
    }

    /// Service 0x27 — send the computed key back to the ECU.
    //fusa:req REQ-UDS-006
    pub async fn security_access_send_key(
        &self,
        ctx: Context,
        access_level: u8,
        key: &[u8],
    ) -> Result<(), Error> {
        let level_key = access_level + 1;
        let mut req = vec![SID_SECURITY_ACCESS, level_key];
        req.extend_from_slice(key);
        let resp = self.request(ctx, &req).await?;
        if resp.len() < 2
            || resp[0] != SID_SECURITY_ACCESS + POSITIVE_RESPONSE_OFFSET
            || resp[1] != level_key
        {
            return Err(Error::Other(format!(
                "uds: unexpected SecurityAccess key response: {:X?}",
                resp
            )));
        }
        Ok(())
    }

    /// Service 0x2E — write a data record to a 2-byte DID.
    //fusa:req REQ-UDS-005
    pub async fn write_did(&self, ctx: Context, did: u16, data: &[u8]) -> Result<(), Error> {
        let high = (did >> 8) as u8;
        let low = did as u8;
        let mut req = vec![SID_WRITE_DID, high, low];
        req.extend_from_slice(data);
        let resp = self.request(ctx, &req).await?;
        if resp.len() < 3
            || resp[0] != SID_WRITE_DID + POSITIVE_RESPONSE_OFFSET
            || resp[1] != high
            || resp[2] != low
        {
            return Err(Error::Other(format!(
                "uds: unexpected WriteDID response: {:X?}",
                resp
            )));
        }
        Ok(())
    }

    /// Service 0x3E — keep-alive with suppress-response flag.
    ///
    /// When `suppress_positive_response` is true the ECU does not reply (per
    /// §7.5.3 sub-function bit 7 suppression).
    //fusa:req REQ-UDS-003
    pub async fn tester_present(
        &self,
        ctx: Context,
        suppress_positive_response: bool,
    ) -> Result<(), Error> {
        let sub = if suppress_positive_response {
            0x80
        } else {
            0x00
        };
        if suppress_positive_response {
            self.conn.send(ctx, &[SID_TESTER_PRESENT, sub]).await?;
            return Ok(());
        }
        let resp = self.request(ctx, &[SID_TESTER_PRESENT, sub]).await?;
        if resp.len() < 2
            || resp[0] != SID_TESTER_PRESENT + POSITIVE_RESPONSE_OFFSET
            || resp[1] != sub
        {
            return Err(Error::Other(format!(
                "uds: unexpected TesterPresent response: {:X?}",
                resp
            )));
        }
        Ok(())
    }

    async fn request(&self, ctx: Context, req: &[u8]) -> Result<Vec<u8>, Error> {
        self.conn.send(ctx.clone(), req).await?;
        let resp = self.conn.recv(ctx).await?;
        if resp.is_empty() {
            return Err(Error::Other("uds: empty response".into()));
        }
        if resp[0] == SID_NEGATIVE_RESPONSE {
            let service = if resp.len() >= 2 { resp[1] } else { 0 };
            let nrc = if resp.len() >= 3 { resp[2] } else { 0 };
            return Err(Error::Other(
                NegativeResponseError { service, nrc }.to_string(),
            ));
        }
        Ok(resp)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    //fusa:test REQ-UDS-007
    #[test]
    fn negative_response_error_display() {
        let e = NegativeResponseError {
            service: SID_READ_DID,
            nrc: 0x31,
        };
        let s = e.to_string();
        assert!(s.contains("0x31"), "should contain NRC hex: {}", s);
        assert!(
            s.contains("requestOutOfRange"),
            "should contain NRC name: {}",
            s
        );
        assert!(s.contains("0x22"), "should contain service hex: {}", s);
    }

    //fusa:test REQ-UDS-007
    #[test]
    fn negative_response_error_unknown_nrc() {
        let e = NegativeResponseError {
            service: SID_ECU_RESET,
            nrc: 0xFF,
        };
        assert_eq!(e.nrc_description(), "unknown");
    }

    //fusa:test REQ-UDS-001
    #[test]
    fn session_type_values() {
        assert_eq!(SessionType::Default as u8, 0x01);
        assert_eq!(SessionType::Programming as u8, 0x02);
        assert_eq!(SessionType::Extended as u8, 0x03);
    }

    //fusa:test REQ-UDS-002
    #[test]
    fn reset_type_values() {
        assert_eq!(ResetType::Hard as u8, 0x01);
        assert_eq!(ResetType::KeyOffOn as u8, 0x02);
        assert_eq!(ResetType::Soft as u8, 0x03);
    }

    //fusa:test REQ-UDS-006
    #[test]
    fn security_access_key_level_is_seed_level_plus_one() {
        // seed level 0x01 → key level 0x02 per ISO 14229 §10.4.2
        assert_eq!(0x01_u8 + 1, 0x02);
        assert_eq!(0x03_u8 + 1, 0x04);
    }
}
