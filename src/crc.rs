// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! CRC utilities for rust-CAN.
//!
//! Provides CRC-16/CCITT-FALSE used by the E2E safety module.

/// Compute CRC-16/CCITT-FALSE over `data`.
///
/// Parameters: poly=0x1021, init=0xFFFF, refin=false, refout=false, xorout=0x0000
pub fn crc16_ccitt_false(data: &[u8]) -> u16 {
    const POLY: u16 = 0x1021;
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ POLY;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_value() {
        // CRC-16/CCITT-FALSE of b"123456789" == 0x29B1
        assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
    }

    #[test]
    fn empty_data() {
        assert_eq!(crc16_ccitt_false(&[]), 0xFFFF);
    }
}
