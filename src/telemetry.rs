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
    /// Car's right and forward unit vectors in world space, taken straight from
    /// the matrix basis. Used to fill the Codemasters roll/pitch direction
    /// vectors for SimHub.
    pub right: Vec3,
    pub forward: Vec3,

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
    /// Position seen on the previous frame (to detect whether the game advanced).
    last_seen_position: Vec3,
    /// Position at the last *actual* change, plus time accumulated since then.
    change_anchor_pos: Vec3,
    secs_since_change: f32,
    /// Raw world velocity, held across duplicate frames.
    raw_world_velocity: Vec3,
    /// EMA-smoothed world velocity (what we report / derive accel from).
    smooth_world_velocity: Vec3,
    last_smooth_local_velocity: Vec3,
    smooth_gforce: Vec3,
    last_out_gforce: Vec3,
    smooth_angular: Euler,
    last_orientation: Euler,
}

/// Position change (game units) below which a frame is treated as a duplicate.
const POSITION_EPS: f32 = 1.0e-4;
/// If the transform does not advance for this long, treat the car as stopped.
const STOP_TIMEOUT: f32 = 0.12;
/// Per-frame EMA factors for velocity and acceleration smoothing.
const VEL_SMOOTH: f32 = 0.35;
const ACC_SMOOTH: f32 = 0.25;
/// Clamp each acceleration axis to a sane range so a quantisation step can never
/// emit a multi-hundred-g spike to a motion rig.
const MAX_G: f32 = 16.0;

fn ema(prev: Vec3, new: Vec3, alpha: f32) -> Vec3 {
    Vec3::new(
        prev.x + alpha * (new.x - prev.x),
        prev.y + alpha * (new.y - prev.y),
        prev.z + alpha * (new.z - prev.z),
    )
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
    ///
    /// Wreckfest updates the transform at the game's own rate, which is lower
    /// than our poll rate, so naive per-frame differencing aliases badly: the
    /// velocity reads zero on frames where the game has not advanced and spikes
    /// on the frames where it has (producing absurd hundreds-of-g accelerations).
    /// To fix this we measure velocity over the real interval between actual
    /// position changes, hold it across duplicate frames, smooth it, and clamp
    /// the derived acceleration.
    pub fn update(&mut self, transform: &Transform, dt: f32) -> Telemetry {
        let position = transform.position();
        let orientation = euler_from_transform(transform);

        if !self.primed || dt <= 0.0 {
            self.primed = true;
            self.last_seen_position = position;
            self.change_anchor_pos = position;
            self.secs_since_change = 0.0;
            self.raw_world_velocity = Vec3::default();
            self.smooth_world_velocity = Vec3::default();
            self.last_smooth_local_velocity = Vec3::default();
            self.smooth_gforce = Vec3::default();
            self.last_out_gforce = Vec3::default();
            self.smooth_angular = Euler::default();
            self.last_orientation = orientation;
            return Telemetry {
                total_time: self.total_time,
                position,
                orientation,
                right: transform.right(),
                forward: transform.forward(),
                ..Default::default()
            };
        }

        self.total_time += dt;
        self.secs_since_change += dt;

        // Did the game actually advance the transform this frame?
        let moved = position.sub(self.last_seen_position).length() > POSITION_EPS;
        if moved {
            let interval = self.secs_since_change.max(1.0e-4);
            self.raw_world_velocity = position.sub(self.change_anchor_pos).scale(1.0 / interval);
            self.change_anchor_pos = position;
            self.secs_since_change = 0.0;
        } else if self.secs_since_change > STOP_TIMEOUT {
            // No update for a while: the car has actually stopped.
            self.raw_world_velocity = Vec3::default();
        }
        // else: duplicate frame within the update window -> hold velocity.
        self.last_seen_position = position;

        // Smooth world velocity, then project into the car frame.
        self.smooth_world_velocity =
            ema(self.smooth_world_velocity, self.raw_world_velocity, VEL_SMOOTH);
        let world_velocity = self.smooth_world_velocity;
        let local_velocity = transform.world_to_local(world_velocity);
        let speed = world_velocity.length();

        // Acceleration from the smoothed local velocity, smoothed again and
        // clamped. x=lateral, y=vertical, z=longitudinal (g).
        let raw_g = local_velocity
            .sub(self.last_smooth_local_velocity)
            .scale(1.0 / dt)
            .scale(1.0 / G);
        self.last_smooth_local_velocity = local_velocity;
        self.smooth_gforce = ema(self.smooth_gforce, raw_g, ACC_SMOOTH);
        let gforce = Vec3::new(
            self.smooth_gforce.x.clamp(-MAX_G, MAX_G),
            self.smooth_gforce.y.clamp(-MAX_G, MAX_G),
            self.smooth_gforce.z.clamp(-MAX_G, MAX_G),
        );

        // Angular velocity (rad/s) from wrapped angle deltas, EMA-smoothed to
        // suppress the same per-frame aliasing.
        let raw_angular = Euler {
            pitch: angular_change(self.last_orientation.pitch, orientation.pitch) / dt,
            yaw: angular_change(self.last_orientation.yaw, orientation.yaw) / dt,
            roll: angular_change(self.last_orientation.roll, orientation.roll) / dt,
        };
        self.last_orientation = orientation;
        self.smooth_angular = Euler {
            pitch: self.smooth_angular.pitch
                + VEL_SMOOTH * (raw_angular.pitch - self.smooth_angular.pitch),
            yaw: self.smooth_angular.yaw + VEL_SMOOTH * (raw_angular.yaw - self.smooth_angular.yaw),
            roll: self.smooth_angular.roll
                + VEL_SMOOTH * (raw_angular.roll - self.smooth_angular.roll),
        };

        // Impact: magnitude of the change in the (smoothed, clamped) g vector.
        let impact = gforce.sub(self.last_out_gforce).length();
        self.last_out_gforce = gforce;

        Telemetry {
            total_time: self.total_time,
            position,
            orientation,
            right: transform.right(),
            forward: transform.forward(),
            world_velocity,
            local_velocity,
            speed,
            gforce,
            angular_velocity: self.smooth_angular,
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
    fn steady_forward_motion_converges_to_surge_speed() {
        let mut d = Deriver::new();
        let dt = 0.1;
        // Move +1 unit of world Z every frame => 10 u/s forward.
        let mut z = 0.0f32;
        d.update(&identity_at(0.0, 0.0, z), dt);
        let mut last = Telemetry::default();
        for _ in 0..40 {
            z += 1.0;
            last = d.update(&identity_at(0.0, 0.0, z), dt);
        }
        // After smoothing converges, speed sits at ~10 u/s along surge (local z).
        assert!((last.speed - 10.0).abs() < 0.5, "speed={}", last.speed);
        assert!((last.local_velocity.z - 10.0).abs() < 0.5);
        assert!(last.local_velocity.x.abs() < 0.5);
    }

    #[test]
    fn aliased_updates_stay_stable_and_bounded() {
        // Reproduce the real bug: the reader polls at a steady rate but the game
        // only advances the transform every other poll. Naive differencing would
        // alternate speed 0/large and emit hundreds of g. The deriver must keep
        // speed steady and acceleration bounded.
        let mut d = Deriver::new();
        let dt = 0.01; // 100 Hz poll
        let mut z = 0.0f32;
        d.update(&identity_at(0.0, 0.0, z), dt);
        let mut min_spd = f32::INFINITY;
        let mut max_spd = f32::NEG_INFINITY;
        let mut max_g = 0.0f32;
        for i in 0..200 {
            if i % 2 == 1 {
                z += 0.3; // 0.3 units per 0.02 s => 15 u/s
            }
            let t = d.update(&identity_at(0.0, 0.0, z), dt);
            if i > 80 {
                min_spd = min_spd.min(t.speed);
                max_spd = max_spd.max(t.speed);
                max_g = max_g
                    .max(t.gforce.x.abs())
                    .max(t.gforce.y.abs())
                    .max(t.gforce.z.abs());
            }
        }
        assert!(
            min_spd > 12.0 && max_spd < 18.0,
            "speed should hug 15, got {min_spd}..{max_spd}"
        );
        assert!(max_g < 5.0, "steady motion must not spike g, got {max_g}");
    }
}
