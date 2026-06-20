//! The telemetry we can honestly produce for Wreckfest 1.
//!
//! WF1 exposes only a transform matrix (position + orientation) in memory. Every
//! field below is either read directly from that matrix or derived from it over
//! time with real physics. There are deliberately **no** RPM, gear, throttle,
//! brake, suspension, tyre or fuel fields: WF1 does not expose that data, and we
//! do not invent it. (SpaceMonkey fakes RPM from the gamepad trigger and
//! hardcodes gear to 1; we refuse to ship fabricated values.)

use crate::math::{angular_change, euler_from_transform, Euler, Transform, Vec3, G};

/// One frame of derived Wreckfest telemetry. All units SI unless noted.
#[derive(Clone, Copy, Debug, Default)]
pub struct Telemetry {
    /// Seconds since lock (integrated dt).
    pub total_time: f32,

    // --- read directly from the matrix ---
    /// World position (game units).
    pub position: Vec3,
    /// Orientation in radians (pitch, yaw, roll).
    pub orientation: Euler,

    // --- derived from the matrix over time ---
    /// World-space velocity vector (units/s).
    pub world_velocity: Vec3,
    /// Velocity in the car's local frame: x=lateral(sway), y=vertical(heave),
    /// z=longitudinal(surge), in units/s.
    pub local_velocity: Vec3,
    /// Scalar speed (units/s). Multiply by 3.6 for km/h if units are metres.
    pub speed: f32,
    /// Acceleration in the car's local frame, in g: x=lateral, y=vertical,
    /// z=longitudinal. Feeds SimHub surge/sway/heave and bass shakers.
    pub gforce: Vec3,
    /// Angular velocity in rad/s (pitch, yaw, roll rates).
    pub angular_velocity: Euler,
    /// Impact magnitude: size of the sudden change in the g-force vector between
    /// frames. Spikes on collisions; good for crash haptics. Derived, in g.
    pub impact: f32,
}

/// Stateful converter: feed it the raw matrix + dt each frame, get derived
/// telemetry out. Mirrors the derivation order in SpaceMonkey's
/// `GenericProviderBase.ProcessTransform`.
#[derive(Default)]
pub struct Deriver {
    primed: bool,
    total_time: f32,
    last_position: Vec3,
    last_local_velocity: Vec3,
    last_gforce: Vec3,
    last_orientation: Euler,
}

impl Deriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset state (e.g. after a re-scan / new lock).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Produce one telemetry frame. `dt` is seconds since the previous frame.
    /// The first call after a reset primes state and returns position +
    /// orientation only (velocity/accel need two samples).
    pub fn update(&mut self, transform: &Transform, dt: f32) -> Telemetry {
        let position = transform.position();
        let orientation = euler_from_transform(transform);

        if !self.primed || dt <= 0.0 {
            self.primed = true;
            self.last_position = position;
            self.last_local_velocity = Vec3::default();
            self.last_gforce = Vec3::default();
            self.last_orientation = orientation;
            return Telemetry {
                total_time: self.total_time,
                position,
                orientation,
                ..Default::default()
            };
        }

        self.total_time += dt;

        // World velocity from position delta.
        let world_velocity = position.sub(self.last_position).scale(1.0 / dt);
        // Project into the car frame: lateral / vertical / longitudinal.
        let local_velocity = transform.world_to_local(world_velocity);
        let speed = world_velocity.length();

        // Local acceleration in g (1/G == SpaceMonkey's 0.10197162129779283).
        let gforce = local_velocity
            .sub(self.last_local_velocity)
            .scale(1.0 / dt)
            .scale(1.0 / G);

        // Angular velocity from wrapped angle deltas.
        let angular_velocity = Euler {
            pitch: angular_change(self.last_orientation.pitch, orientation.pitch) / dt,
            yaw: angular_change(self.last_orientation.yaw, orientation.yaw) / dt,
            roll: angular_change(self.last_orientation.roll, orientation.roll) / dt,
        };

        // Impact = magnitude of the change in the g-force vector.
        let impact = gforce.sub(self.last_gforce).length();

        self.last_position = position;
        self.last_local_velocity = local_velocity;
        self.last_gforce = gforce;
        self.last_orientation = orientation;

        Telemetry {
            total_time: self.total_time,
            position,
            orientation,
            world_velocity,
            local_velocity,
            speed,
            gforce,
            angular_velocity,
            impact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_at(x: f32, y: f32, z: f32) -> Transform {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        m[12] = x;
        m[13] = y;
        m[14] = z;
        Transform::from_floats(m)
    }

    #[test]
    fn first_frame_primes_without_velocity() {
        let mut d = Deriver::new();
        let t = d.update(&identity_at(0.0, 0.0, 0.0), 1.0 / 60.0);
        assert_eq!(t.speed, 0.0);
        assert_eq!(t.world_velocity, Vec3::default());
    }

    #[test]
    fn forward_motion_produces_surge_speed() {
        let mut d = Deriver::new();
        let dt = 0.1;
        d.update(&identity_at(0.0, 0.0, 0.0), dt);
        // Moved +10 along world Z in 0.1s => 100 units/s forward.
        let t = d.update(&identity_at(0.0, 0.0, 10.0), dt);
        assert!((t.speed - 100.0).abs() < 1e-3);
        // With identity orientation, forward axis is world Z => surge (local z).
        assert!((t.local_velocity.z - 100.0).abs() < 1e-3);
        assert!(t.local_velocity.x.abs() < 1e-3);
    }
}
