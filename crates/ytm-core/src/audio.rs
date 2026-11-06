//! Playback speed and audio effects applied in the page by `assets/audiofx.js`.

use serde::{Deserialize, Serialize};

pub const SPEED_RANGE: (f32, f32) = (0.5, 1.5);
pub const BASS_MAX_DB: f32 = 12.0;
/// Equalizer band gain and preamp range, in dB.
pub const EQ_MAX_DB: f32 = 12.0;
/// Centre frequencies of the 10 equalizer bands (Hz), ISO octave bands.
pub const EQ_BANDS: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFx {
    /// Playback rate (never applied to ads).
    pub speed: f32,
    /// Keep the pitch when the speed changes. Off = "slowed" / "nightcore" sound.
    pub preserve_pitch: bool,
    /// Reverb wet level, 0..=1.
    pub reverb: f32,
    /// Low-shelf boost in dB, 0..=12.
    pub bass_db: f32,
    /// Dynamics compressor that evens out loudness.
    pub normalize: bool,
    /// Equalizer gains in dB for [`EQ_BANDS`], -12..=12.
    pub eq: [f32; 10],
    /// Gain applied before the equalizer, -12..=12 dB.
    pub preamp_db: f32,
    /// Stereo balance, -1 (left) ..= 1 (right).
    pub balance: f32,
    /// Downmix to mono.
    pub mono: bool,
    /// "8D" effect: the sound slowly circles from left to right.
    pub spatial: bool,
    /// Hard limiter at -1 dBFS on the output (no clipping peaks).
    pub limiter: bool,
}

impl Default for AudioFx {
    fn default() -> Self {
        Self {
            speed: 1.0,
            preserve_pitch: true,
            reverb: 0.0,
            bass_db: 0.0,
            normalize: false,
            eq: [0.0; 10],
            preamp_db: 0.0,
            balance: 0.0,
            mono: false,
            spatial: false,
            limiter: false,
        }
    }
}

fn clamp(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl AudioFx {
    pub fn sanitized(self) -> Self {
        Self {
            speed: clamp(self.speed, SPEED_RANGE.0, SPEED_RANGE.1, 1.0),
            reverb: clamp(self.reverb, 0.0, 1.0, 0.0),
            bass_db: clamp(self.bass_db, 0.0, BASS_MAX_DB, 0.0),
            eq: self.eq.map(|g| clamp(g, -EQ_MAX_DB, EQ_MAX_DB, 0.0)),
            preamp_db: clamp(self.preamp_db, -EQ_MAX_DB, EQ_MAX_DB, 0.0),
            balance: clamp(self.balance, -1.0, 1.0, 0.0),
            ..self
        }
    }

    /// Effect presets (speed, pitch, reverb, bass). They leave the equalizer and
    /// the output options untouched.
    pub fn preset(name: &str) -> Option<Self> {
        let base = Self::default();
        Some(match name {
            "off" => base,
            "slowed" => Self {
                speed: 0.85,
                preserve_pitch: false,
                ..base
            },
            "slowed_reverb" => Self {
                speed: 0.85,
                preserve_pitch: false,
                reverb: 0.45,
                ..base
            },
            "nightcore" => Self {
                speed: 1.25,
                preserve_pitch: false,
                ..base
            },
            "bass_boost" => Self {
                bass_db: 8.0,
                ..base
            },
            "concert_hall" => Self {
                reverb: 0.35,
                ..base
            },
            _ => return None,
        })
    }

    /// JS applying these settings in the page. The value is serialized JSON,
    /// a valid JS literal; numbers are already clamped.
    pub fn to_js(&self) -> String {
        let json = serde_json::to_string(&self.sanitized()).unwrap_or_else(|_| "{}".into());
        format!("window.__ytmFx && window.__ytmFx.apply({json})")
    }
}

/// Equalizer presets (gains for [`EQ_BANDS`], dB).
pub fn eq_preset(name: &str) -> Option<[f32; 10]> {
    Some(match name {
        "flat" => [0.0; 10],
        "bass" => [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        "rock" => [4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 1.0, 3.0, 4.0, 4.0],
        "pop" => [-1.0, 1.0, 3.0, 4.0, 3.0, 0.0, -1.0, -1.0, 1.0, 2.0],
        "vocal" => [-2.0, -2.0, -1.0, 1.0, 3.0, 4.0, 3.0, 2.0, 0.0, -1.0],
        "classical" => [3.0, 2.0, 1.0, 0.0, 0.0, 0.0, -1.0, 1.0, 2.0, 3.0],
        "electronic" => [5.0, 4.0, 1.0, 0.0, -2.0, 1.0, 0.0, 2.0, 4.0, 5.0],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_and_rejects_nan() {
        let mut eq = [0.0; 10];
        eq[0] = 40.0;
        eq[9] = f32::NAN;
        let fx = AudioFx {
            speed: 9.0,
            reverb: f32::NAN,
            bass_db: -3.0,
            eq,
            preamp_db: -30.0,
            balance: 2.0,
            ..AudioFx::default()
        }
        .sanitized();
        assert_eq!((fx.speed, fx.reverb, fx.bass_db), (1.5, 0.0, 0.0));
        assert_eq!((fx.eq[0], fx.eq[9]), (EQ_MAX_DB, 0.0));
        assert_eq!((fx.preamp_db, fx.balance), (-EQ_MAX_DB, 1.0));
    }

    #[test]
    fn presets_are_valid_and_unknown_is_none() {
        for name in [
            "off",
            "slowed",
            "slowed_reverb",
            "nightcore",
            "bass_boost",
            "concert_hall",
        ] {
            let p = AudioFx::preset(name).unwrap();
            assert_eq!(p, p.sanitized(), "{name}");
        }
        assert!(AudioFx::preset("loud").is_none());
        assert!(!AudioFx::preset("slowed").unwrap().preserve_pitch);
        for name in [
            "flat",
            "bass",
            "rock",
            "pop",
            "vocal",
            "classical",
            "electronic",
        ] {
            let eq = eq_preset(name).unwrap();
            assert!(eq.iter().all(|g| g.abs() <= EQ_MAX_DB), "{name}");
        }
        assert!(eq_preset("x").is_none());
    }

    #[test]
    fn to_js_embeds_clamped_json() {
        let js = AudioFx {
            speed: 3.0,
            ..AudioFx::default()
        }
        .to_js();
        assert!(js.starts_with("window.__ytmFx && window.__ytmFx.apply({"));
        assert!(js.contains("\"speed\":1.5"));
        assert!(js.contains("\"eq\":[0.0"));
    }
}
