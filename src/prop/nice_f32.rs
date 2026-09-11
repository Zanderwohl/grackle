//! Writing an `f32` to a file the way somebody typed it.
//!
//! Serde widens an `f32` to the `f64` TOML stores, and the widening is exact
//! rather than helpful: `0.05f32` comes out as `0.05000000074505806`. A file
//! full of those is technically diffable and practically not.
//!
//! So a value is widened through its own shortest decimal instead. Rust prints
//! the shortest decimal that reads back as the same `f32`, so parsing that as
//! an `f64` is lossless by construction —
//! `every_f32_survives_being_written_nicely` pins it.
//!
//! Applied by hand through `#[serde(with = ...)]` rather than to every float
//! in the crate, which keeps it where the argument holds: a file somebody
//! reads.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The shortest decimal that reads back as this exact `f32`.
fn widen(value: f32) -> f64 {
    // Infinities and NaN have no decimal form to shorten, and cannot be written
    // to TOML either — so the failure stays serde's rather than a panic here.
    format!("{value}").parse().unwrap_or(value as f64)
}

/// One `f32`.
pub mod scalar {
    use super::*;

    pub fn serialize<S: Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
        widen(*value).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
        f64::deserialize(deserializer).map(|value| value as f32)
    }
}

/// A fixed-length array of them — a position, a rotation, a colour.
pub mod array {
    use super::*;

    pub fn serialize<S: Serializer, const N: usize>(
        value: &[f32; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(widen).serialize(serializer)
    }

    /// Through a `Vec`, because serde writes its array impls out one length at
    /// a time rather than over a const generic. The length check is what an
    /// array impl would have done, and what a hand-edited file deserves.
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

/// A list of them, for a profile whose corners were typed out.
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

    /// Shortening is a *presentation* change and never a numeric one. If this
    /// fails, a prop reopens very slightly the wrong shape — the kind of drift
    /// nobody notices until two parts stop meeting.
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

    /// What a reviewer actually sees.
    #[test]
    fn a_typed_number_is_written_as_it_was_typed() {
        assert_eq!(widen(0.05), 0.05f64);
        assert_eq!(widen(-1.25), -1.25f64);
    }
}
