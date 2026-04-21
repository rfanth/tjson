use std::fmt;
use std::str::FromStr;

/// A JSON number value, stored as its original string representation.
///
/// Validation is delegated to `serde_json`'s number parser, so any string accepted here
/// is guaranteed to be a valid JSON number. NaN and infinity are rejected.
///
/// # Construction
///
/// ```
/// use tjson::Number;
///
/// let n: Number = "42".parse().unwrap();
/// let n: Number = "-3.14".parse().unwrap();
/// let n: Number = "1e100".parse().unwrap();
///
/// assert!(Number::try_from(f64::NAN).is_err());
/// assert!(Number::try_from(f64::INFINITY).is_err());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Number(pub(crate) String);

/// Error returned when a value is not a finite, valid JSON number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidNumber(String);

impl fmt::Display for InvalidNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid JSON number: {}", self.0)
    }
}

impl std::error::Error for InvalidNumber {}

impl Number {
    /// Returns the number as its string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the value as an `i64` if it is an integer that fits.
    pub fn as_i64(&self) -> Option<i64> {
        self.0.parse().ok()
    }

    /// Returns the value as a `u64` if it is a non-negative integer that fits.
    pub fn as_u64(&self) -> Option<u64> {
        self.0.parse().ok()
    }

    /// Returns the value as an `f64`.
    ///
    /// Returns `None` only if the string somehow fails to parse as a float, which cannot
    /// happen for any `Number` constructed through the public API. Large integers and
    /// high-precision decimals may lose precision in the conversion.
    pub fn as_f64(&self) -> Option<f64> {
        self.0.parse().ok()
    }

    /// Returns `true` if the number has no fractional or exponent part.
    pub fn is_integer(&self) -> bool {
        !self.0.contains('.') && !self.0.contains('e') && !self.0.contains('E')
    }

    /// Convert to a `serde_json::Number`. The string was validated by `serde_json`'s own
    /// parser at construction, so this parse cannot fail.
    pub(crate) fn to_serde_json_number(&self) -> serde_json::Number {
        self.0.parse().expect("Number string validated by serde_json at construction")
    }
}

impl FromStr for Number {
    type Err = InvalidNumber;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Use serde_json for validation. We store the original string, not the
        // serde_json representation, to preserve exact round-trip fidelity.
        s.parse::<serde_json::Number>()
            .map(|_| Self(s.to_owned()))
            .map_err(|_| InvalidNumber(s.to_owned()))
    }
}

impl TryFrom<f64> for Number {
    type Error = InvalidNumber;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        // from_f64 returns None for NaN and infinity.
        serde_json::Number::from_f64(value)
            .map(|n| Self(n.to_string()))
            .ok_or_else(|| InvalidNumber(value.to_string()))
    }
}

impl From<i64> for Number {
    fn from(value: i64) -> Self { Self(value.to_string()) }
}

impl From<u64> for Number {
    fn from(value: u64) -> Self { Self(value.to_string()) }
}

impl From<i32> for Number {
    fn from(value: i32) -> Self { Self(value.to_string()) }
}

impl From<u32> for Number {
    fn from(value: u32) -> Self { Self(value.to_string()) }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for Number {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_serde_json_number().serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Number {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde_json::Number::deserialize(deserializer).map(|n| Self(n.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid() {
        for s in ["0", "-0", "1", "-1", "42", "3.14", "-3.14", "1e10", "1E10",
                  "1.5e-3", "1.5E+3", "0.0", "99999999999999999999"] {
            assert!(s.parse::<Number>().is_ok(), "expected valid: {s}");
        }
    }

    #[test]
    fn parse_invalid() {
        for s in ["", "nan", "NaN", "inf", "Infinity", "-inf",
                  "1.", ".5", "1e", "1e+", "01", "--1", "+1"] {
            assert!(s.parse::<Number>().is_err(), "expected invalid: {s}");
        }
    }

    #[test]
    fn roundtrip_string() {
        for s in ["42", "-3.14", "1e100", "1E10", "99999999999999999999"] {
            let n: Number = s.parse().unwrap();
            assert_eq!(n.as_str(), s, "roundtrip failed for {s}");
        }
    }

    #[test]
    fn from_f64_rejects_non_finite() {
        assert!(Number::try_from(f64::NAN).is_err());
        assert!(Number::try_from(f64::INFINITY).is_err());
        assert!(Number::try_from(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn from_f64_finite() {
        let n = Number::try_from(3.14_f64).unwrap();
        assert_eq!(n.as_str(), "3.14");
    }

    #[test]
    fn from_integers() {
        assert_eq!(Number::from(42i64).as_str(), "42");
        assert_eq!(Number::from(u64::MAX).as_str(), "18446744073709551615");
        assert_eq!(Number::from(-1i64).as_str(), "-1");
    }

    #[test]
    fn as_accessors() {
        let n: Number = "42".parse().unwrap();
        assert_eq!(n.as_i64(), Some(42));
        assert_eq!(n.as_u64(), Some(42));

        let n: Number = "-5".parse().unwrap();
        assert_eq!(n.as_i64(), Some(-5));
        assert_eq!(n.as_u64(), None);

        let n: Number = "3.14".parse().unwrap();
        assert_eq!(n.as_i64(), None);
        assert!((n.as_f64().unwrap() - 3.14).abs() < 1e-10);
    }

    #[test]
    fn is_integer() {
        assert!("42".parse::<Number>().unwrap().is_integer());
        assert!(!"3.14".parse::<Number>().unwrap().is_integer());
        assert!(!"1e10".parse::<Number>().unwrap().is_integer());
    }
}
