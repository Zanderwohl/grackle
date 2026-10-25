//! Writing an `f32` to a file the way somebody typed it.
//!
//! Serde widens an `f32` to `f64` exactly, so `0.05f32` is written as
//! `0.05000000074505806`. Widening through Rust's shortest round-tripping
//! decimal instead is lossless. Applied per field with `#[serde(with = ...)]`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) fn widen(value: f32) -> f64 {
    // Infinities and NaN: leave the failure to serde rather than panic here.
    format!("{value}").parse().unwrap_or(value as f64)
}

pub mod scalar {
    use super::*;

    pub fn serialize<S: Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
        widen(*value).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
        f64::deserialize(deserializer).map(|value| value as f32)
    }
}

pub mod array {
    use super::*;

    pub fn serialize<S: Serializer, const N: usize>(
        value: &[f32; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(widen).serialize(serializer)
    }

    /// Through a `Vec`, because serde has no const-generic array impl.
    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        deserializer: D,
    ) -> Result<[f32; N], D::Error> {
        let components = Vec::<f64>::deserialize(deserializer)?;
        let length = components.len();
        <[f64; N]>::try_from(components)
            .map(|value| value.map(|component| component as f32))
            .map_err(|_| serde::de::Error::invalid_length(length, &"exactly N numbers"))
    }
}

pub mod pairs {
    use super::*;

    pub fn serialize<S: Serializer>(
        value: &[[f32; 2]],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.iter().map(|pair| pair.map(widen)).collect::<Vec<_>>().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<[f32; 2]>, D::Error> {
        Vec::<[f64; 2]>::deserialize(deserializer)
            .map(|points| points.into_iter().map(|pair| pair.map(|c| c as f32)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if writing a value changes it, so a prop reopens slightly off.
    #[test]
    fn every_f32_survives_being_written_nicely() {
        let awkward = [
            0.05, 0.1, 1.0 / 3.0, 0.0, -0.0, 1e-8, 1.5e12, 89.999, 0.038,
            std::f32::consts::PI, f32::MIN_POSITIVE, f32::MAX,
        ];
        for value in awkward {
            assert_eq!(
                widen(value) as f32,
                value,
                "{value} did not survive being widened through its own decimal form",
            );
        }
    }

    #[test]
    fn a_typed_number_is_written_as_it_was_typed() {
        assert_eq!(widen(0.05), 0.05f64);
        assert_eq!(widen(-1.25), -1.25f64);
    }
}
