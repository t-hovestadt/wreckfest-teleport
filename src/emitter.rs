//! UDP emitter.
//!
//! This sends the real telemetry to the SimHub PC over UDP. There are two layers
//! here on purpose:
//!
//! * `pack_native()` is OUR own little-endian packet (magic + version + fields).
//!   It is fully defined here, so it works today and can be verified with any
//!   loopback receiver. Use this to prove the read->network path end to end.
//!
//! * `pack_simhub()` emits the Codemasters extradata=3 (DiRT Rally 2.0) UDP
//!   packet, which SimHub reads natively via its DiRT Rally 2.0 plugin on UDP
//!   port 20777. WF1 has only motion data, so we fill position, world velocity,
//!   the orientation direction vectors, speed and lateral/longitudinal g, and
//!   leave every engine/suspension/wheel field at zero.

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
    /// SimHub via the Codemasters extradata=3 (DiRT Rally 2.0) UDP format.
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

/// SimHub-compatible packet: the Codemasters "extradata=3" UDP format used by
/// DiRT Rally / DiRT Rally 2.0 (264 bytes = 66 little-endian f32). SimHub reads
/// this natively via its DiRT Rally 2.0 plugin (UDP port 20777), so we skip the
/// External Sim editor entirely.
///
/// WF1 gives us real motion only, so we fill the fields we can and leave the
/// rest zero (honest absence, never fabricated):
///   [0]       total time (s)
///   [4..=6]   world position x/y/z
///   [7]       speed (m/s)
///   [8..=10]  world velocity x/y/z
///   [11..=13] roll vector  = car right-axis unit vector
///   [14..=16] pitch vector = car forward-axis unit vector
///   [34]      lateral g
///   [35]      longitudinal g
///   [36]      current lap = 1 (so SimHub treats the feed as on-stage)
/// Everything else (suspension, wheels, throttle, gear, RPM, fuel, ...) is 0,
/// because WF1 does not expose it.
pub fn pack_simhub(t: &Telemetry, _seq: u64) -> Vec<u8> {
    const FLOATS: usize = 66; // extradata=3 => 264 bytes
    let mut p = [0f32; FLOATS];
    p[0] = t.total_time;
    p[4] = t.position.x;
    p[5] = t.position.y;
    p[6] = t.position.z;
    p[7] = t.speed;
    p[8] = t.world_velocity.x;
    p[9] = t.world_velocity.y;
    p[10] = t.world_velocity.z;
    p[11] = t.right.x;
    p[12] = t.right.y;
    p[13] = t.right.z;
    p[14] = t.forward.x;
    p[15] = t.forward.y;
    p[16] = t.forward.z;
    p[34] = t.gforce.x; // lateral g
    p[35] = t.gforce.z; // longitudinal g
    p[36] = 1.0; // current lap: nonzero so SimHub sees an active stage
    let mut b = Vec::with_capacity(FLOATS * 4);
    for v in p {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
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

    #[test]
    fn simhub_packet_is_codemasters_extradata3_layout() {
        let t = Telemetry {
            total_time: 12.5,
            speed: 30.0,
            position: Vec3::new(1.0, 2.0, 3.0),
            world_velocity: Vec3::new(4.0, 5.0, 6.0),
            right: Vec3::new(1.0, 0.0, 0.0),
            forward: Vec3::new(0.0, 0.0, 1.0),
            gforce: Vec3::new(0.7, 0.0, -0.9),
            ..Default::default()
        };
        let p = pack_simhub(&t, 1);
        // extradata=3 is exactly 264 bytes (66 little-endian f32).
        assert_eq!(p.len(), 264);
        let f = |i: usize| f32::from_le_bytes([p[i * 4], p[i * 4 + 1], p[i * 4 + 2], p[i * 4 + 3]]);
        assert_eq!(f(0), 12.5); // total time
        assert_eq!(f(4), 1.0); // pos x
        assert_eq!(f(6), 3.0); // pos z
        assert_eq!(f(7), 30.0); // speed
        assert_eq!(f(10), 6.0); // world vel z
        assert_eq!(f(11), 1.0); // roll vector x = right.x
        assert_eq!(f(16), 1.0); // pitch vector z = forward.z
        assert_eq!(f(34), 0.7); // lateral g
        assert_eq!(f(35), -0.9); // longitudinal g
        assert_eq!(f(36), 1.0); // current lap
    }
}
