//! Confidence values on the wire.
//!
//! Confidence is a probability in `0.0..=1.0`. It is computed as `f32` because
//! that is what the providers return and 24 bits of mantissa is far more than a
//! recognition score justifies.
//!
//! It is **not** serialized as a raw `f32`, for two reasons:
//!
//! 1. `serde_json` writes the shortest decimal that round-trips an `f32`, so
//!    `0.97f32` becomes `0.9700000286102295`. On a text protocol that is both
//!    longer than the `f64` form and unreadable to anyone implementing the
//!    schema from the docs.
//! 2. A provider emitting `0.8000001` would put noise in the wire format that
//!    every consumer then has to tolerate.
//!
//! Rounding to three decimal places — one tenth of a percentage point, well
//! below any decision the client or gateway makes on the value — keeps payloads
//! human-readable and stable. Deserialization additionally **rejects**
//! out-of-range values: a confidence of `1.5` or `-1` means a provider adapter
//! is broken, and clamping it silently would hide that bug behind a plausible
//! number.

use serde::{Deserialize, Deserializer, Serializer};

/// Decimal places retained when a confidence crosses the wire.
const WIRE_DECIMALS: u32 = 3;

/// Round a confidence to the precision the wire format carries, as `f32`.
///
/// Used when normalizing a value that has just been deserialized.
pub fn round(value: f32) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    let factor = 10f32.powi(WIRE_DECIMALS as i32);
    (value * factor).round() / factor
}

/// Round a confidence and widen to `f64` for serialization.
///
/// This is not the same as `round(value) as f64`. `serde_json` serializes an
/// `f32` by widening it with `as f64`, which re-exposes the binary
/// representation: `0.97f32 as f64` is `0.9700000286102295`. Rounding *in*
/// `f64` instead lands on the `f64` nearest to `0.97`, which serializes as the
/// three-character string a reader expects.
pub fn round_to_f64(value: f32) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let factor = 10f64.powi(WIRE_DECIMALS as i32);
    ((value as f64) * factor).round() / factor
}

/// Serialize a confidence as a rounded JSON number.
pub fn serialize<S: Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(round_to_f64(*value))
}

/// Deserialize a confidence, rejecting values outside `0.0..=1.0`.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let value = f32::deserialize(deserializer)?;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(serde::de::Error::custom(format!(
            "confidence {value} is outside 0.0..=1.0"
        )));
    }
    Ok(round(value))
}

/// `serde` glue for `Option<f32>` confidences, preserving the same rounding and
/// range rules.
pub mod option {
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serialize an optional confidence.
    pub fn serialize<S: Serializer>(
        value: &Option<f32>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(v) => serializer.serialize_some(&super::round_to_f64(*v)),
            None => serializer.serialize_none(),
        }
    }

    /// Deserialize an optional confidence with range validation.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<f32>, D::Error> {
        let value = Option::<f32>::deserialize(deserializer)?;
        match value {
            Some(v) => super::confidence_checked(v)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// Shared range check used by both the required and optional paths.
pub(crate) fn confidence_checked(value: f32) -> Result<f32, String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("confidence {value} is outside 0.0..=1.0"));
    }
    Ok(round(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Wrapper {
        #[serde(with = "crate::confidence")]
        value: f32,
    }

    fn to_json(value: f32) -> String {
        serde_json::to_string(&Wrapper { value }).unwrap()
    }

    #[test]
    fn common_confidences_serialize_readably() {
        // The bug this module exists to fix: these must not become
        // 0.9700000286102295.
        assert_eq!(to_json(0.97), r#"{"value":0.97}"#);
        assert_eq!(to_json(0.94), r#"{"value":0.94}"#);
        assert_eq!(to_json(0.71), r#"{"value":0.71}"#);
        assert_eq!(to_json(0.82), r#"{"value":0.82}"#);
        assert_eq!(to_json(1.0), r#"{"value":1.0}"#);
        assert_eq!(to_json(0.0), r#"{"value":0.0}"#);
    }

    #[test]
    fn float_noise_is_rounded_away() {
        assert_eq!(to_json(0.9700000_286), r#"{"value":0.97}"#);
        assert_eq!(to_json(0.123_456_79), r#"{"value":0.123}"#);
    }

    #[test]
    fn round_trip_is_stable() {
        for value in [0.0f32, 0.5, 0.94, 0.97, 1.0] {
            let json = to_json(value);
            let back: Wrapper = serde_json::from_str(&json).unwrap();
            assert_eq!(to_json(back.value), json, "unstable for {value}");
        }
    }

    #[test]
    fn out_of_range_is_rejected_not_clamped() {
        assert!(serde_json::from_str::<Wrapper>(r#"{"value":1.5}"#).is_err());
        assert!(serde_json::from_str::<Wrapper>(r#"{"value":-0.1}"#).is_err());
    }

    #[test]
    fn non_finite_input_does_not_panic() {
        assert_eq!(round(f32::NAN), 0.0);
        assert_eq!(round(f32::INFINITY), 0.0);
    }

    #[test]
    fn boundaries_are_inclusive() {
        assert!(serde_json::from_str::<Wrapper>(r#"{"value":0.0}"#).is_ok());
        assert!(serde_json::from_str::<Wrapper>(r#"{"value":1.0}"#).is_ok());
    }
}
