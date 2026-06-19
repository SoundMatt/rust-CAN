// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! OBD-II (ISO 15031 / SAE J1979) on-board diagnostics over ISO-TP.
//!
//! OBD-II is the standardised diagnostic interface mandated for all passenger
//! cars sold in the USA since 1996.
//!
//! OBD-II uses fixed CAN IDs:
//! - `0x7DF` functional broadcast request (all ECUs)
//! - `0x7E8` ECU #1 response (engine control unit)
//!
//! # Example
//! ```rust,no_run
//! # use std::sync::Arc;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use rust_can::{virtual_bus::VirtualBus, Bus};
//! use rust_can::isotp::{Config, IsoTpConn};
//! use rust_can::obdii::Client;
//!
//! let bus: Arc<dyn Bus> = Arc::new(VirtualBus::new());
//! let conn = IsoTpConn::new(bus, Config {
//!     tx_id: 0x7DF, rx_id: 0x7E8, ..Default::default()
//! }).await?;
//! let client = Client::new(conn);
//! # Ok(())
//! # }
//! ```

use crate::error::Error;
use crate::isotp::IsoTpConn;
use crate::relay::Context;

// ---------------------------------------------------------------------------
// Service IDs (OBD-II modes)
// ---------------------------------------------------------------------------

pub const MODE_CURRENT_DATA: u8 = 0x01;
pub const MODE_FREEZE_DTC: u8 = 0x02;
pub const MODE_STORED_DTC: u8 = 0x03;
pub const MODE_CLEAR_DTC: u8 = 0x04;
pub const MODE_VEHICLE_INFO: u8 = 0x09;

// Standard Mode 01 PIDs
pub const PID_ENGINE_LOAD: u8 = 0x04;
pub const PID_COOLANT_TEMP: u8 = 0x05;
pub const PID_ENGINE_RPM: u8 = 0x0C;
pub const PID_VEHICLE_SPEED: u8 = 0x0D;
pub const PID_THROTTLE_POSITION: u8 = 0x11;
pub const PID_INTAKE_AIR_TEMP: u8 = 0x0F;
pub const PID_MAF: u8 = 0x10;
pub const PID_FUEL_TANK_LEVEL: u8 = 0x2F;
pub const PID_BAROMETRIC_PRESSURE: u8 = 0x33;
pub const PID_AMBIENT_AIR_TEMP: u8 = 0x46;
pub const PID_TIMING_ADVANCE: u8 = 0x0E;
pub const PID_CONTROL_MODULE_VOLTAGE: u8 = 0x42;
pub const PID_RUNTIME_SINCE_START: u8 = 0x1F;
pub const PID_INTAKE_MANIFOLD_PRESSURE: u8 = 0x0B;

// Mode 09 PIDs
pub const PID_VIN: u8 = 0x02;

// ---------------------------------------------------------------------------
// Value
// ---------------------------------------------------------------------------

/// A decoded OBD-II PID value with its physical reading and unit.
pub struct Value {
    pub pid: u8,
    pub raw: Vec<u8>,
    pub float: f64,
    pub unit: &'static str,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An OBD-II client communicating over ISO-TP.
//fusa:req REQ-OBD-001
pub struct Client {
    conn: IsoTpConn,
}

impl Client {
    /// Create an OBD-II client.
    //fusa:req REQ-OBD-001
    pub fn new(conn: IsoTpConn) -> Self {
        Self { conn }
    }

    /// Request a single Mode 01 (current data) PID and return the decoded value.
    //fusa:req REQ-OBD-002
    pub async fn read_pid(&self, ctx: Context, pid: u8) -> Result<Value, Error> {
        let resp = self.request(ctx, &[MODE_CURRENT_DATA, pid]).await?;
        if resp.len() < 3 || resp[0] != MODE_CURRENT_DATA + 0x40 || resp[1] != pid {
            return Err(Error::Other(format!(
                "obdii: unexpected response for PID 0x{:02X}: {:X?}",
                pid, resp
            )));
        }
        Ok(decode_mode01(pid, resp[2..].to_vec()))
    }

    /// Request the Vehicle Identification Number (Mode 09 PID 0x02).
    //fusa:req REQ-OBD-003
    pub async fn read_vin(&self, ctx: Context) -> Result<String, Error> {
        let resp = self.request(ctx, &[MODE_VEHICLE_INFO, PID_VIN]).await?;
        if resp.len() < 4 || resp[0] != MODE_VEHICLE_INFO + 0x40 || resp[1] != PID_VIN {
            return Err(Error::Other(format!(
                "obdii: unexpected VIN response: {:X?}",
                resp
            )));
        }
        let vin_bytes: Vec<u8> = resp[3..].iter().copied().take_while(|&b| b != 0).collect();
        String::from_utf8(vin_bytes)
            .map_err(|e| Error::Other(format!("obdii: VIN not valid UTF-8: {}", e)))
    }

    /// Return the supported PID bitmask for the given group (0x00, 0x20, …).
    pub async fn supported_pids(&self, ctx: Context, group: u8) -> Result<u32, Error> {
        let resp = self.request(ctx, &[MODE_CURRENT_DATA, group]).await?;
        if resp.len() < 6 || resp[0] != MODE_CURRENT_DATA + 0x40 || resp[1] != group {
            return Err(Error::Other(format!(
                "obdii: unexpected supported PIDs response: {:X?}",
                resp
            )));
        }
        Ok(
            (resp[2] as u32) << 24
                | (resp[3] as u32) << 16
                | (resp[4] as u32) << 8
                | resp[5] as u32,
        )
    }

    async fn request(&self, ctx: Context, req: &[u8]) -> Result<Vec<u8>, Error> {
        self.conn.send(ctx.clone(), req).await?;
        let resp = self.conn.recv(ctx).await?;
        if resp.is_empty() {
            return Err(Error::Other("obdii: empty response".into()));
        }
        if resp[0] == 0x7F {
            let nrc = if resp.len() >= 3 { resp[2] } else { 0 };
            return Err(Error::Other(format!(
                "obdii: negative response for mode 0x{:02X}: NRC 0x{:02X}",
                req[0], nrc
            )));
        }
        Ok(resp)
    }
}

#[allow(clippy::collapsible_match)]
fn decode_mode01(pid: u8, data: Vec<u8>) -> Value {
    let mut v = Value {
        pid,
        raw: data.clone(),
        float: 0.0,
        unit: "",
    };
    match pid {
        PID_ENGINE_RPM => {
            if data.len() >= 2 {
                v.float = (u16::from(data[0]) << 8 | u16::from(data[1])) as f64 / 4.0;
                v.unit = "rpm";
            }
        }
        PID_VEHICLE_SPEED => {
            if !data.is_empty() {
                v.float = data[0] as f64;
                v.unit = "km/h";
            }
        }
        PID_COOLANT_TEMP | PID_INTAKE_AIR_TEMP | PID_AMBIENT_AIR_TEMP => {
            if !data.is_empty() {
                v.float = data[0] as f64 - 40.0;
                v.unit = "°C";
            }
        }
        PID_ENGINE_LOAD | PID_THROTTLE_POSITION | PID_FUEL_TANK_LEVEL => {
            if !data.is_empty() {
                v.float = data[0] as f64 * 100.0 / 255.0;
                v.unit = "%";
            }
        }
        PID_MAF => {
            if data.len() >= 2 {
                v.float = (u16::from(data[0]) << 8 | u16::from(data[1])) as f64 / 100.0;
                v.unit = "g/s";
            }
        }
        PID_INTAKE_MANIFOLD_PRESSURE | PID_BAROMETRIC_PRESSURE => {
            if !data.is_empty() {
                v.float = data[0] as f64;
                v.unit = "kPa";
            }
        }
        PID_TIMING_ADVANCE => {
            if !data.is_empty() {
                v.float = data[0] as f64 / 2.0 - 64.0;
                v.unit = "°";
            }
        }
        PID_CONTROL_MODULE_VOLTAGE => {
            if data.len() >= 2 {
                v.float = (u16::from(data[0]) << 8 | u16::from(data[1])) as f64 / 1000.0;
                v.unit = "V";
            }
        }
        PID_RUNTIME_SINCE_START => {
            if data.len() >= 2 {
                v.float = (u16::from(data[0]) << 8 | u16::from(data[1])) as f64;
                v.unit = "s";
            }
        }
        _ => {}
    }
    v
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    //fusa:test REQ-OBD-002
    #[test]
    fn decode_rpm_from_bytes() {
        let v = decode_mode01(PID_ENGINE_RPM, vec![0x0B, 0xB8]);
        // 0x0BB8 = 3000 → 3000/4 = 750 rpm
        assert!(
            (v.float - 750.0).abs() < 0.01,
            "expected 750 rpm, got {}",
            v.float
        );
        assert_eq!(v.unit, "rpm");
    }

    //fusa:test REQ-OBD-002
    #[test]
    fn decode_speed_from_byte() {
        let v = decode_mode01(PID_VEHICLE_SPEED, vec![0x64]);
        assert!((v.float - 100.0).abs() < 0.01, "expected 100 km/h");
        assert_eq!(v.unit, "km/h");
    }

    //fusa:test REQ-OBD-002
    #[test]
    fn decode_coolant_temp() {
        let v = decode_mode01(PID_COOLANT_TEMP, vec![0x69]); // 0x69=105, 105-40=65°C
        assert!(
            (v.float - 65.0).abs() < 0.01,
            "expected 65°C, got {}",
            v.float
        );
        assert_eq!(v.unit, "°C");
    }
}
