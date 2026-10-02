//! The Eureka Precisa's Bluetooth protocol. The scale is a Krell CFS-9002
//! sold under Eureka's name; the protocol follows the Decent de1app
//! (`de1plus/bluetooth.tcl`, "Eureka Precisa / Krell CFS-9002").
//!
//! * Service `FFF0`; weight notifications on `FFF1`; commands to `FFF2`,
//!   written without response.
//! * Notification: `AA 09 41 <timer running> <timer s, u16 le> <negative>
//!   <weight 0.1 g, u16 le> 00 <checksum>`.
//! * Command: `AA 02 3x 3x`: 1 tare, 2 power off, 3 start timer, 4 stop
//!   timer, 5 reset timer (also stops), 7 beep twice. Unit: `AA 03 36 0u cs`.

use uuid::Uuid;

pub const STATUS: Uuid = Uuid::from_u128(0x0000_fff1_0000_1000_8000_0080_5f9b_34fb);
pub const COMMAND: Uuid = Uuid::from_u128(0x0000_fff2_0000_1000_8000_0080_5f9b_34fb);

/// What the scale calls itself when advertising.
pub const DEFAULT_NAME_PREFIX: &str = "CFS-9002";

pub const TARE: [u8; 4] = [0xAA, 0x02, 0x31, 0x31];
pub const START_TIMER: [u8; 4] = [0xAA, 0x02, 0x33, 0x33];
pub const STOP_TIMER: [u8; 4] = [0xAA, 0x02, 0x34, 0x34];
pub const RESET_TIMER: [u8; 4] = [0xAA, 0x02, 0x35, 0x35];
pub const BEEP_TWICE: [u8; 4] = [0xAA, 0x02, 0x37, 0x37];
pub const UNIT_GRAMS: [u8; 5] = [0xAA, 0x03, 0x36, 0x00, 0x36];

/// One weight notification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub grams: f64,
    pub timer_running: bool,
}

/// Parses a notification from `FFF1`; `None` for anything else. The
/// checksum is not checked (neither does the de1app).
pub fn parse(data: &[u8]) -> Option<Reading> {
    if data.len() < 9 || data[0] != 0xAA || data[1] != 0x09 || data[2] != 0x41 {
        return None;
    }
    // Bytes 4-5 are the scale's own timer in whole seconds; gbs-anywhere
    // measures the time itself.
    let sign = if data[6] == 1 { -1.0 } else { 1.0 };
    Some(Reading {
        grams: sign * f64::from(u16::from_le_bytes([data[7], data[8]])) / 10.0,
        timer_running: data[3] != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_example() {
        // Timer running (at 10 s), positive, 0.5 g.
        let r = parse(&[0xAA, 0x09, 0x41, 0x01, 0x0A, 0x00, 0x00, 0x05, 0x00, 0x00, 0x51]).unwrap();
        assert_eq!(
            r,
            Reading {
                grams: 0.5,
                timer_running: true,
            }
        );
    }

    #[test]
    fn parses_negative_and_large_weights() {
        let r = parse(&[0xAA, 0x09, 0x41, 0x00, 0x00, 0x00, 0x01, 0x0F, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(r.grams, -1.5);
        assert!(!r.timer_running);
        // 3810 tenths = 381.0 g
        let r = parse(&[0xAA, 0x09, 0x41, 0x00, 0x00, 0x00, 0x00, 0xE2, 0x0E, 0x00, 0x00]).unwrap();
        assert_eq!(r.grams, 381.0);
    }

    #[test]
    fn ignores_other_packets() {
        assert_eq!(parse(&[0xAA, 0x09, 0x41, 0x00]), None);
        assert_eq!(parse(&[0xAA, 0x02, 0x31, 0x31, 0, 0, 0, 0, 0]), None);
        assert_eq!(parse(&[]), None);
    }
}
