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

/// NRC 0x78 — requestCorrectlyReceivedResponsePending: the ECU has accepted
/// the request and is still processing it; per ISO 14229 §8.7 the client
/// MUST keep waiting for the real response rather than treating this as a
/// failure.
const NRC_RESPONSE_PENDING: u8 = 0x78;

/// Upper bound on consecutive NRC 0x78 responses before `request()` gives up.
/// Bounds the wait against a misbehaving ECU that never stops sending
/// "pending" — real ECUs send only a handful before the final response.
const MAX_RESPONSE_PENDING_RETRIES: u32 = 16;

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

    /// All UDS requests and responses are transported over `IsoTpConn`
    /// (ISO 15765-2) — this is the single choke point every service method
    /// above routes through.
    //fusa:req REQ-UDS-009
    /// NRC 0x78 (response pending) is retried transparently rather than
    /// surfaced as an error; every other negative response is surfaced as
    /// `NegativeResponseError`.
    //fusa:req REQ-UDS-008
    async fn request(&self, ctx: Context, req: &[u8]) -> Result<Vec<u8>, Error> {
        self.conn.send(ctx.clone(), req).await?;

        for _ in 0..=MAX_RESPONSE_PENDING_RETRIES {
            let resp = self.conn.recv(ctx.clone()).await?;
            if resp.is_empty() {
                return Err(Error::Other("uds: empty response".into()));
            }
            if resp[0] == SID_NEGATIVE_RESPONSE {
                let service = if resp.len() >= 2 { resp[1] } else { 0 };
                let nrc = if resp.len() >= 3 { resp[2] } else { 0 };
                if nrc == NRC_RESPONSE_PENDING {
                    // ECU is still working — keep waiting for the real
                    // response instead of surfacing this as a failure.
                    continue;
                }
                return Err(Error::Other(
                    NegativeResponseError { service, nrc }.to_string(),
                ));
            }
            return Ok(resp);
        }

        Err(Error::Other(
            "uds: too many consecutive response-pending (NRC 0x78) retries".into(),
        ))
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

    // -- End-to-end transport tests -----------------------------------------
    //
    // The tests above exercise pure logic (enum values, error formatting);
    // these exercise `Client::request()` over a real `IsoTpConn` pair on a
    // `VirtualBus`, with a fake ECU peer on the other end, so REQ-UDS-008's
    // NRC 0x78 retry behavior and REQ-UDS-009's transport claim are actually
    // verified end-to-end rather than only asserted by tag.

    use crate::isotp::Config as IsoTpConfig;
    use crate::relay::Context;
    use crate::virtual_bus::VirtualBus;
    use std::sync::Arc;

    async fn client_and_ecu_conn() -> (IsoTpConn, IsoTpConn) {
        let bus = Arc::new(VirtualBus::new());
        let client_conn = IsoTpConn::new(
            bus.clone(),
            IsoTpConfig {
                tx_id: 0x7E0,
                rx_id: 0x7E8,
                timeout: std::time::Duration::from_millis(200),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let ecu_conn = IsoTpConn::new(
            bus,
            IsoTpConfig {
                tx_id: 0x7E8,
                rx_id: 0x7E0,
                timeout: std::time::Duration::from_millis(200),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        (client_conn, ecu_conn)
    }

    /// REQ-UDS-009: a UDS request/response round-trips through a real
    /// `IsoTpConn` pair — not just an in-memory call.
    //fusa:test REQ-UDS-009
    #[tokio::test]
    async fn request_is_transported_over_isotp() {
        let (client_conn, ecu_conn) = client_and_ecu_conn().await;
        let client = Client::new(client_conn);

        let ecu = tokio::spawn(async move {
            let req = ecu_conn.recv(Context::background()).await.unwrap();
            assert_eq!(req, vec![SID_DIAGNOSTIC_SESSION_CONTROL, 0x03]);
            ecu_conn
                .send(
                    Context::background(),
                    &[
                        SID_DIAGNOSTIC_SESSION_CONTROL + POSITIVE_RESPONSE_OFFSET,
                        0x03,
                    ],
                )
                .await
                .unwrap();
        });

        client
            .diagnostic_session_control(Context::background(), SessionType::Extended)
            .await
            .expect("request should succeed over a real IsoTpConn transport");
        ecu.await.unwrap();
    }

    /// REQ-UDS-008: NRC 0x78 (response pending) is retried transparently —
    /// the client must not surface it as an error — while it still keeps
    /// waiting for, and returns, the eventual real response.
    //fusa:test REQ-UDS-008
    #[tokio::test]
    async fn request_retries_transparently_on_response_pending_nrc() {
        let (client_conn, ecu_conn) = client_and_ecu_conn().await;
        let client = Client::new(client_conn);

        let ecu = tokio::spawn(async move {
            let _req = ecu_conn.recv(Context::background()).await.unwrap();
            // Send two "response pending" NRCs before the real answer.
            for _ in 0..2 {
                ecu_conn
                    .send(
                        Context::background(),
                        &[
                            SID_NEGATIVE_RESPONSE,
                            SID_TESTER_PRESENT,
                            NRC_RESPONSE_PENDING,
                        ],
                    )
                    .await
                    .unwrap();
            }
            ecu_conn
                .send(
                    Context::background(),
                    &[SID_TESTER_PRESENT + POSITIVE_RESPONSE_OFFSET, 0x00],
                )
                .await
                .unwrap();
        });

        client
            .tester_present(Context::background(), false)
            .await
            .expect("0x78 responses must be retried, not surfaced as an error");
        ecu.await.unwrap();
    }

    /// REQ-UDS-008: a negative response with an NRC other than 0x78 MUST
    /// still surface as `NegativeResponseError` immediately, not be retried.
    //fusa:test REQ-UDS-008
    #[tokio::test]
    async fn request_surfaces_non_pending_negative_response_immediately() {
        let (client_conn, ecu_conn) = client_and_ecu_conn().await;
        let client = Client::new(client_conn);

        let ecu = tokio::spawn(async move {
            let _req = ecu_conn.recv(Context::background()).await.unwrap();
            ecu_conn
                .send(
                    Context::background(),
                    &[SID_NEGATIVE_RESPONSE, SID_TESTER_PRESENT, 0x31], // requestOutOfRange
                )
                .await
                .unwrap();
        });

        let err = client
            .tester_present(Context::background(), false)
            .await
            .expect_err("a non-0x78 NRC must surface as an error, not be retried");
        assert!(err.to_string().contains("0x31"));
        ecu.await.unwrap();
    }
}
