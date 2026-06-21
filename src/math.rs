//! Small math helpers for interpreting the WF1 transform matrix.
//!
//! The matrix decomposition, world->local velocity projection, quaternion
//! extraction and pitch/yaw/roll conversion are ported faithfully from
//! SpaceMonkey's `GenericProviderBase.cs` / `Utils.cs` so the derived values
//! match a known-good reference. See NOTICE for attribution.

use std::f32::consts::PI;

/// Standard gravity (m/s^2). SpaceMonkey converts m/s^2 to g by multiplying by
/// 0.10197162129779283, which is exactly 1.0 / 9.80665.
pub const G: f32 = 9.80665;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    pub fn scale(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// A row-major 4x4 transform read from WF1 memory (16 f32).
///
/// Indexing follows SpaceMonkey / System.Numerics.Matrix4x4: the basis vectors
/// are rows 0..2 and the translation is row 3.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub m: [f32; 16],
}

impl Transform {
    pub fn from_floats(m: [f32; 16]) -> Self {
        Self { m }
    }

    /// Right / lateral axis (matrix row 0): M11,M12,M13.
    pub fn right(&self) -> Vec3 {
        Vec3::new(self.m[0], self.m[1], self.m[2])
    }
    /// Up / vertical axis (matrix row 1): M21,M22,M23.
    pub fn up(&self) -> Vec3 {
        Vec3::new(self.m[4], self.m[5], self.m[6])
    }
    /// Forward / longitudinal axis (matrix row 2): M31,M32,M33.
    pub fn forward(&self) -> Vec3 {
        Vec3::new(self.m[8], self.m[9], self.m[10])
    }
    /// Translation / world position (matrix row 3): M41,M42,M43.
    pub fn position(&self) -> Vec3 {
        Vec3::new(self.m[12], self.m[13], self.m[14])
    }

    /// SpaceMonkey's garbage check (`ProcessFwdUpRht`): a valid rotation has
    /// orthonormal basis vectors, so each must have magnitude ~1. If any is
    /// well below 1 we are not looking at a real transform.
    pub fn basis_looks_valid(&self) -> bool {
        let r = self.right().length();
        let u = self.up().length();
        let f = self.forward().length();
        let ok = |v: f32| (0.9..=1.1).contains(&v);
        self.position().is_finite() && ok(r) && ok(u) && ok(f)
    }

    /// Project a world-space vector into the car's local frame.
    ///
    /// For an orthonormal basis this equals SpaceMonkey's
    /// `Transform(worldVec, inverse(rotation))`: the local components are the
    /// projections of the world vector onto each basis vector.
    /// Returns (lateral_x, vertical_y, longitudinal_z).
    pub fn world_to_local(&self, world: Vec3) -> Vec3 {
        Vec3::new(
            world.dot(self.right()),
            world.dot(self.up()),
            world.dot(self.forward()),
        )
    }
}

/// Pitch/yaw/roll in radians.
#[derive(Clone, Copy, Debug, Default)]
pub struct Euler {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

#[derive(Clone, Copy, Debug)]
struct Quat {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

/// Build a quaternion from the rotation part of the transform.
///
/// Standard matrix->quaternion (matches System.Numerics
/// `Quaternion.CreateFromRotationMatrix`), using 1-indexed entries:
/// M11=m[0] M12=m[1] M13=m[2] / M21=m[4] M22=m[5] M23=m[6] / M31=m[8] M32=m[9] M33=m[10].
fn quat_from_transform(t: &Transform) -> Quat {
    let m11 = t.m[0];
    let m12 = t.m[1];
    let m13 = t.m[2];
    let m21 = t.m[4];
    let m22 = t.m[5];
    let m23 = t.m[6];
    let m31 = t.m[8];
    let m32 = t.m[9];
    let m33 = t.m[10];

    let trace = m11 + m22 + m33;
    if trace > 0.0 {
        let mut s = (trace + 1.0).sqrt();
        let w = s * 0.5;
        s = 0.5 / s;
        Quat {
            x: (m23 - m32) * s,
            y: (m31 - m13) * s,
            z: (m12 - m21) * s,
            w,
        }
    } else if m11 >= m22 && m11 >= m33 {
        let s = (1.0 + m11 - m22 - m33).sqrt();
        let inv = 0.5 / s;
        Quat {
            x: 0.5 * s,
            y: (m12 + m21) * inv,
            z: (m13 + m31) * inv,
            w: (m23 - m32) * inv,
        }
    } else if m22 > m33 {
        let s = (1.0 + m22 - m11 - m33).sqrt();
        let inv = 0.5 / s;
        Quat {
            x: (m21 + m12) * inv,
            y: 0.5 * s,
            z: (m32 + m23) * inv,
            w: (m31 - m13) * inv,
        }
    } else {
        let s = (1.0 + m33 - m11 - m22).sqrt();
        let inv = 0.5 / s;
        Quat {
            x: (m31 + m13) * inv,
            y: (m32 + m23) * inv,
            z: 0.5 * s,
            w: (m12 - m21) * inv,
        }
    }
}

/// Port of SpaceMonkey `Utils.GetPYRFromQuaternion`.
fn pyr_from_quat(r: Quat) -> Euler {
    let yaw = (2.0 * (r.y * r.w + r.x * r.z)).atan2(1.0 - 2.0 * (r.x * r.x + r.y * r.y));
    let pitch = (2.0 * (r.x * r.w - r.y * r.z)).clamp(-1.0, 1.0).asin();
    let roll = (2.0 * (r.x * r.y + r.z * r.w)).atan2(1.0 - 2.0 * (r.x * r.x + r.z * r.z));
    Euler { pitch, yaw, roll }
}

/// Port of SpaceMonkey `Utils.LoopAngleRad`.
fn loop_angle_rad(angle: f32, min_mag: f32) -> f32 {
    let abs_angle = angle.abs();
    if abs_angle <= min_mag {
        return angle;
    }
    let direction = angle / abs_angle;
    (PI * direction) - angle
}

/// Extract pitch/yaw/roll from the transform, applying SpaceMonkey's exact sign
/// conventions (`CalcAngles`): pitch=-pyr.x, yaw=-pyr.y, roll=loop(-pyr.z, pi/2).
pub fn euler_from_transform(t: &Transform) -> Euler {
    let pyr = pyr_from_quat(quat_from_transform(t));
    Euler {
        pitch: -pyr.pitch,
        yaw: -pyr.yaw,
        roll: loop_angle_rad(-pyr.roll, PI * 0.5),
    }
}

/// Shortest signed difference b-a, wrapped to [-pi, pi]. Used for angular rates
/// (equivalent to SpaceMonkey `Utils.CalculateAngularChange`).
pub fn angular_change(a: f32, b: f32) -> f32 {
    let mut d = b - a;
    while d > PI {
        d -= 2.0 * PI;
    }
    while d < -PI {
        d += 2.0 * PI;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_transform_has_valid_basis_and_zero_angles() {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        let t = Transform::from_floats(m);
        assert!(t.basis_looks_valid());
        let e = euler_from_transform(&t);
        assert!(e.pitch.abs() < 1e-4);
        assert!(e.yaw.abs() < 1e-4);
        assert!(e.roll.abs() < 1e-4);
    }

    #[test]
    fn garbage_basis_is_rejected() {
        let t = Transform::from_floats([0.0; 16]);
        assert!(!t.basis_looks_valid());
    }

    #[test]
    fn world_to_local_on_identity_is_passthrough() {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        let t = Transform::from_floats(m);
        let v = t.world_to_local(Vec3::new(3.0, -2.0, 5.0));
        assert_eq!(v, Vec3::new(3.0, -2.0, 5.0));
    }
}

impl Transform {
    /// Build a transform from 64 little-endian bytes: the row-major 4x4 f32
    /// matrix exactly as it sits in Wreckfest's process memory.
    pub fn from_le_bytes(bytes: &[u8; 64]) -> Self {
        let mut m = [0f32; 16];
        for (i, s) in m.iter_mut().enumerate() {
            let o = i * 4;
            *s = f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        }
        Self::from_floats(m)
    }
}
