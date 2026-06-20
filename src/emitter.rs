//! UDP emitter.
//!
//! This sends the real telemetry to the SimHub PC over UDP. There are two layers
//! here on purpose:
//!
//! * `pack_native()` is OUR own little-endian packet (magic + version + fields).
//!   It is fully defined here, so it works today and can be verified with any
//!   loopback receiver. Use this to prove the read->network path end to end.
//!
//! * `pack_simhub()` is where SimHub's External Sim "contract" packet goes. That
//!   format includes a header with a game/telemetry signature that SimHub
//!   computes from the `.simdef` you author in its editor. Those exact bytes
//!   come from SimHub's "copy demo code (C#/C++)" button. We deliberately do not
//!   guess them. Once you paste that generated struct, it drops straight in here
//!   and the rest of the pipeline (socket, cadence, field mapping) is unchanged.

use std::io;
use std::net::UdpSocket;

use crate::reader::{Status, TelemetrySink};
use crate::telemetry::Telemetry;

/// Magic for the native packet: ASCII "WFT1".
pub const NATIVE_MAGIC: u32 = 0x57465431;
/// Native packet schema version. Bump on any field change.
pub const NATIVE_VERSION: u16 = 1;

/// Which wire format to emit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Our own documented packet (works now, for testing the pipe).
    Native,
    /// SimHub External Sim contract (pending the generated struct; see below).
    SimHub,
}

pub struct UdpEmitter {
    socket: UdpSocket,
    target: String,
    format: Format,
    seq: u64,
    sent: u64,
    log_status: bool,
}

impl UdpEmitter {
    /// Bind a sending socket and aim it at `target` (e.g. "192.168.50.2:22123").
    pub fn new(target: impl Into<String>, format: Format, log_status: bool) -> io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        let target = target.into();
        socket.connect(&target)?;
        Ok(Self {
            socket,
            target,
            format,
            seq: 0,
            sent: 0,
            log_status,
        })
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn packets_sent(&self) -> u64 {
        self.sent
    }
}

impl TelemetrySink for UdpEmitter {
    fn on_status(&mut self, status: Status) {
        if self.log_status {
            eprintln!("[status] {status:?}");
        }
    }

    fn on_frame(&mut self, frame: &Telemetry) {
        self.seq += 1;
        let bytes = match self.format {
            Format::Native => pack_native(frame, self.seq),
            Format::SimHub => pack_simhub(frame, self.seq),
        };
        if self.socket.send(&bytes).is_ok() {
            self.sent += 1;
        }
    }
}

/// Our own packet. Layout (all little-endian):
///   u32  magic   = "WFT1"
///   u16  version = 1
///   u16  _pad
///   u64  seq
///   f32  total_time
///   f32  pos_x, pos_y, pos_z
///   f32  pitch, yaw, roll                (rad)
///   f32  world_vel_x, world_vel_y, world_vel_z
///   f32  local_vel_x(sway), local_vel_y(heave), local_vel_z(surge)
///   f32  speed                           (units/s)
///   f32  g_lateral, g_vertical, g_longitudinal   (g)
///   f32  pitch_rate, yaw_rate, roll_rate (rad/s)
///   f32  impact                          (g)
pub fn pack_native(t: &Telemetry, seq: u64) -> Vec<u8> {
    let mut b = Vec::with_capacity(128);
    b.extend_from_slice(&NATIVE_MAGIC.to_le_bytes());
    b.extend_from_slice(&NATIVE_VERSION.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&seq.to_le_bytes());

    let mut f = |v: f32| b.extend_from_slice(&v.to_le_bytes());
    f(t.total_time);
    f(t.position.x);
    f(t.position.y);
    f(t.position.z);
    f(t.orientation.pitch);
    f(t.orientation.yaw);
    f(t.orientation.roll);
    f(t.world_velocity.x);
    f(t.world_velocity.y);
    f(t.world_velocity.z);
    f(t.local_velocity.x);
    f(t.local_velocity.y);
    f(t.local_velocity.z);
    f(t.speed);
    f(t.gforce.x);
    f(t.gforce.y);
    f(t.gforce.z);
    f(t.angular_velocity.pitch);
    f(t.angular_velocity.yaw);
    f(t.angular_velocity.roll);
    f(t.impact);
    b
}

/// SimHub External Sim contract packet.
///
/// PENDING: paste the struct + constants from SimHub's "copy demo code" button
/// (after authoring `wreckfest.simdef`) and serialise those exact fields/header
/// here. Until then this returns the native packet so the binary still runs end
/// to end; it will NOT validate against SimHub until the real layout is wired.
pub fn pack_simhub(t: &Telemetry, seq: u64) -> Vec<u8> {
    // TODO(simhub): replace with the generated External Sim structure:
    //   - SimHub header (magic + game signature + telemetry signature)
    //   - declared fields in the order shown by the editor, correct units
    pack_native(t, seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Euler, Vec3};

    #[test]
    fn native_packet_has_expected_header_and_size() {
        let t = Telemetry {
            speed: 42.0,
            position: Vec3::new(1.0, 2.0, 3.0),
            orientation: Euler {
                pitch: 0.1,
                yaw: 0.2,
                roll: 0.3,
            },
            ..Default::default()
        };
        let p = pack_native(&t, 7);
        assert_eq!(&p[0..4], &NATIVE_MAGIC.to_le_bytes());
        // header: 4+2+2+8 = 16 bytes, then 21 f32 = 84 bytes => 100 total.
        assert_eq!(p.len(), 16 + 21 * 4);
    }
}
