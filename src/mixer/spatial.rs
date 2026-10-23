//! Point-source 3D placement, horizontal speaker layouts and an explicit stereo fold.
//! Coordinates are right-handed: +X right, +Y up, default forward -Z. Positions
//! and distances use the same units; velocities and sound speed use units/second.
//! Velocity controls Doppler only: the host supplies updated positions explicitly.
//!
//! Surround speaker directions are horizontal: quad +/-45 and +/-135 degrees;
//! 5.1 front +/-30, centre 0, surrounds +/-110; 7.1 adds sides +/-90 and backs
//! +/-150. Angles are clockwise from forward. Elevation affects distance and
//! Doppler; a directly vertical/coincident emitter uses the front direction.
//! There is no HRTF, geometric occlusion, propagation delay or bass management.
//!
//! Let d=clamp(distance,min,max) and r=rolloff. Linear gain is
//! clamp(1-r*(d-min)/(max-min),0,1), inverse is min/(min+r*(d-min)), exponential is
//! (d/min)^(-r). These follow the clamped models in the
//! [OpenAL specification](https://www.openal.org/documentation/openal-1.1-specification.pdf).
//! Max distance clamps evaluation, not voice lifetime; inverse/exponential retain
//! a floor. The independent existing Space2d model keeps its previous behavior.
//!
//! Doppler uses the listener-to-emitter unit vector n and ratio
//! (c+factor*dot(listener_velocity,n))/(c+factor*dot(emitter_velocity,n)). Scaled
//! radial velocities clamp to +/-0.9c and the ratio to 0.25..=4; a coincident source
//! has ratio 1. The final rate*pitch*Doppler playback speed clamps to 1/1024..=512,
//! preserving the mixer's existing fixed-point/resampler bound.
//!
//! Directional gains have unit power and are renormalized during ramps. Distance
//! remains a separate ramp used by quietest-voice policy. Listener/emitter targets
//! apply in command sequence order, using the listener then current; spatial
//! clocks freeze with their voice/bus. Legacy voices route to the front pair on
//! surround output. Effects process every channel, compressors/limiters link all
//! channels, and modulation phase offsets repeat according to channel index.
//! Channel order/masks follow
//! [WAVEFORMATEXTENSIBLE](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-waveformatextensible).
use crate::math::{exp2, log2, sin_turns};
use crate::{Error, Result};

pub const MAX_OUTPUT_CHANNELS: usize = 8;

/// Interleaved WAVE speaker order. LFE receives no point-source signal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputLayout {
    Mono,
    Stereo,
    Quad,
    Surround51,
    Surround71,
}
impl OutputLayout {
    pub const fn channels(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::Quad => 4,
            Self::Surround51 => 6,
            Self::Surround71 => 8,
        }
    }
    pub fn from_channels(channels: usize) -> Result<Self> {
        match channels {
            1 => Ok(Self::Mono),
            2 => Ok(Self::Stereo),
            4 => Ok(Self::Quad),
            6 => Ok(Self::Surround51),
            8 => Ok(Self::Surround71),
            _ => Err(Error::invalid(
                "Output layout needs 1, 2, 4, 6 or 8 channels",
            )),
        }
    }
    /// WAVEFORMATEXTENSIBLE speaker mask; 7.1 uses back and side pairs.
    pub const fn channel_mask(self) -> u32 {
        match self {
            Self::Mono => 4,
            Self::Stereo => 3,
            Self::Quad => 0x33,
            Self::Surround51 => 0x3f,
            Self::Surround71 => 0x63f,
        }
    }
    /// Stereo matrix fold. Requires complete input frames and enough output room;
    /// the unwritten output suffix is untouched. No allocation on valid input.
    /// Each full-range column has unit squared gain, preserving individual-channel
    /// and uncorrelated-input power. Correlated channels can sum above unity; apply
    /// headroom/limiting AFTER this fold. LFE is omitted (no bass management).
    pub fn downmix_stereo(self, input: &[f32], out: &mut [f32]) -> Result<usize> {
        let ch = self.channels();
        let frames = input.len() / ch;
        if input.len() % ch != 0 || out.len() / 2 < frames {
            return Err(Error::invalid(
                "Downmix needs complete frames and sufficient stereo output",
            ));
        }
        if self == Self::Stereo {
            out[..input.len()].copy_from_slice(input);
            return Ok(frames);
        }
        let matrix = self.stereo_matrix();
        for (frame, target) in input.chunks_exact(ch).zip(out.chunks_exact_mut(2)) {
            let (mut l, mut r) = (0.0, 0.0);
            for c in 0..ch {
                l += frame[c] as f64 * matrix[c][0];
                r += frame[c] as f64 * matrix[c][1];
            }
            target[0] = l as f32;
            target[1] = r as f32;
        }
        Ok(frames)
    }
    fn stereo_matrix(self) -> [[f64; 2]; 8] {
        let mut m = [[0.0; 2]; 8];
        let centre = std::f64::consts::FRAC_1_SQRT_2;
        if self == Self::Mono {
            m[0] = [centre, centre];
            return m;
        }
        m[0] = [1.0, 0.0];
        m[1] = [0.0, 1.0];
        if self == Self::Quad {
            m[2] = [1.0, 0.0];
            m[3] = [0.0, 1.0];
        } else {
            m[2] = [centre, centre];
            m[4] = [1.0, 0.0];
            m[5] = [0.0, 1.0];
            if self == Self::Surround71 {
                m[6] = [1.0, 0.0];
                m[7] = [0.0, 1.0];
            }
        }
        m
    }
    fn pan(self, x: f64, z: f64) -> [f64; 8] {
        let mut gains = [0.0; 8];
        if self == Self::Mono {
            gains[0] = 1.0;
            return gains;
        }
        let norm = (x * x + z * z).sqrt();
        let (x, z) = if norm < 1e-12 {
            (0.0, 1.0)
        } else {
            (x / norm, z / norm)
        };
        if self == Self::Stereo {
            let turns = (x + 1.0) / 8.0;
            gains[0] = sin_turns(0.25 - turns);
            gains[1] = sin_turns(turns);
            return gains;
        }
        // Clockwise adjacent speaker pairs. Solve the 2x2 amplitude system and
        // normalize its positive gains, avoiding an atan2/platform trig call.
        let speakers: &[(usize, f64, f64)] = match self {
            Self::Quad => &[(0, -Q, Q), (1, Q, Q), (3, Q, -Q), (2, -Q, -Q)],
            Self::Surround51 => &[
                (0, -0.5, T),
                (2, 0.0, 1.0),
                (1, 0.5, T),
                (5, S110, C110),
                (4, -S110, C110),
            ],
            _ => &[
                (0, -0.5, T),
                (2, 0.0, 1.0),
                (1, 0.5, T),
                (7, 1.0, 0.0),
                (5, 0.5, -T),
                (4, -0.5, -T),
                (6, -1.0, 0.0),
            ],
        };
        for i in 0..speakers.len() {
            let (a, ax, az) = speakers[i];
            let (b, bx, bz) = speakers[(i + 1) % speakers.len()];
            let det = ax * bz - bx * az;
            let ga = (x * bz - bx * z) / det;
            let gb = (ax * z - x * az) / det;
            if ga >= -1e-12 && gb >= -1e-12 {
                let (ga, gb) = (ga.max(0.0), gb.max(0.0));
                let power = (ga * ga + gb * gb).sqrt();
                gains[a] = ga / power;
                gains[b] = gb / power;
                break;
            }
        }
        gains
    }
}
const Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
const T: f64 = 0.866_025_403_784_438_6;
const S110: f64 = 0.939_692_620_785_908_4;
const C110: f64 = -0.342_020_143_325_668_7;

/// Listener pose. Orientation axes need not be unit or exactly perpendicular;
/// preparation orthonormalizes them. Zero/near-parallel axes are rejected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
}
impl Default for Listener {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            velocity: [0.0; 3],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        }
    }
}
impl Listener {
    pub fn validate(&self) -> Result<()> {
        finite(self.position)?;
        finite(self.velocity)?;
        finite(self.forward)?;
        finite(self.up)?;
        let f = normalize(vector(self.forward));
        let u = normalize(vector(self.up));
        if f.is_none()
            || u.is_none()
            || length(cross(f.unwrap_or([0.0; 3]), u.unwrap_or([0.0; 3]))) < 1e-6
        {
            return Err(Error::invalid(
                "Listener orientation axes must be nonzero and not parallel",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DistanceModel {
    Linear,
    #[default]
    Inverse,
    Exponential,
}
/// Clamped distance models: below min gain is one; beyond max distance is clamped
/// to max. Inverse/exponential therefore retain a nonzero floor. Rolloff zero
/// disables attenuation. Equations are documented in the module README/reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attenuation {
    /// Reference distance, 0.001..=1e9 units. Default 1.
    pub min_distance: f32,
    /// Maximum evaluated distance, greater than min and <=1e12 units. Default 100.
    pub max_distance: f32,
    /// 0..=16. Default 1.
    pub rolloff: f32,
    pub model: DistanceModel,
}
impl Default for Attenuation {
    fn default() -> Self {
        Self {
            min_distance: 1.0,
            max_distance: 100.0,
            rolloff: 1.0,
            model: DistanceModel::Inverse,
        }
    }
}
impl Attenuation {
    pub fn validate(&self) -> Result<()> {
        if !(0.001..=1e9).contains(&self.min_distance)
            || !self.max_distance.is_finite()
            || self.max_distance <= self.min_distance
            || self.max_distance > 1e12
            || !(0.0..=16.0).contains(&self.rolloff)
        {
            return Err(Error::invalid(
                "Invalid 3D attenuation distances or rolloff",
            ));
        }
        Ok(())
    }
    pub fn gain(&self, distance: f64) -> Result<f64> {
        self.validate()?;
        if !distance.is_finite() || distance < 0.0 {
            return Err(Error::invalid("Distance must be finite and nonnegative"));
        }
        Ok(self.evaluate(distance))
    }
    fn evaluate(&self, distance: f64) -> f64 {
        let (min, max, r) = (
            self.min_distance as f64,
            self.max_distance as f64,
            self.rolloff as f64,
        );
        let d = distance.clamp(min, max);
        match self.model {
            DistanceModel::Linear => (1.0 - r * (d - min) / (max - min)).clamp(0.0, 1.0),
            DistanceModel::Inverse => min / (min + r * (d - min)),
            DistanceModel::Exponential => exp2(-r * log2(d / min)),
        }
    }
}
/// A mono point emitter. Stereo inputs collapse to (L+R)/2, which can cancel
/// antiphase content. Non-spatial voices retain their usual stereo image.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Emitter {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub attenuation: Attenuation,
}
impl Emitter {
    pub fn validate(&self) -> Result<()> {
        finite(self.position)?;
        finite(self.velocity)?;
        self.attenuation.validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialConfig {
    /// 1..=1e6 units/second. Default 343.3 (metres/second in air).
    pub speed_of_sound: f32,
    /// Velocity multiplier 0..=16. Zero disables Doppler. Default 1.
    pub doppler_factor: f32,
}
impl Default for SpatialConfig {
    fn default() -> Self {
        Self {
            speed_of_sound: 343.3,
            doppler_factor: 1.0,
        }
    }
}
/// Normalized direction gains plus independent attenuation and playback ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub gains: [f64; 8],
    pub distance_gain: f64,
    pub doppler: f64,
}
impl SpatialConfig {
    pub fn validate(&self) -> Result<()> {
        if !(1.0..=1e6).contains(&self.speed_of_sound)
            || !(0.0..=16.0).contains(&self.doppler_factor)
        {
            return Err(Error::invalid("Invalid sound speed or Doppler factor"));
        }
        Ok(())
    }
    pub fn place(
        &self,
        listener: &Listener,
        emitter: &Emitter,
        layout: OutputLayout,
    ) -> Result<Placement> {
        self.validate()?;
        listener.validate()?;
        emitter.validate()?;
        Ok(self.place_validated(listener, emitter, layout))
    }
    pub(super) fn place_validated(
        &self,
        listener: &Listener,
        emitter: &Emitter,
        layout: OutputLayout,
    ) -> Placement {
        let relative =
            std::array::from_fn(|i| emitter.position[i] as f64 - listener.position[i] as f64);
        let distance = length(relative);
        let forward = normalize(vector(listener.forward)).unwrap();
        let right = normalize(cross(forward, vector(listener.up))).unwrap();
        let gains = layout.pan(dot(relative, right), dot(relative, forward));
        let doppler = if distance == 0.0 || self.doppler_factor == 0.0 {
            1.0
        } else {
            let direction = relative.map(|v| v / distance);
            let c = self.speed_of_sound as f64;
            let factor = self.doppler_factor as f64;
            let l = (factor * dot(vector(listener.velocity), direction)).clamp(-0.9 * c, 0.9 * c);
            let s = (factor * dot(vector(emitter.velocity), direction)).clamp(-0.9 * c, 0.9 * c);
            ((c + l) / (c + s)).clamp(0.25, 4.0)
        };
        Placement {
            gains,
            distance_gain: emitter.attenuation.evaluate(distance),
            doppler,
        }
    }
}
fn finite(v: [f32; 3]) -> Result<()> {
    if v.iter().all(|x| x.is_finite()) {
        Ok(())
    } else {
        Err(Error::invalid("Spatial vectors must be finite"))
    }
}
fn vector(v: [f32; 3]) -> [f64; 3] {
    v.map(|x| x as f64)
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(v: [f64; 3]) -> f64 {
    dot(v, v).sqrt()
}
fn normalize(v: [f64; 3]) -> Option<[f64; 3]> {
    let n = length(v);
    (n > 0.0).then(|| v.map(|x| x / n))
}
