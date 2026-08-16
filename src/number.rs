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
/// let n: Number = "-3.45".parse().unwrap();
/// let n: Number = "1e100".parse().unwrap();
///
/// assert!(Number::try_from(f64::NAN).is_err());
/// assert!(Number::try_from(f64::INFINITY).is_err());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Number(pub(crate) String);

/// The two spellings of the exponent marker in a JSON number.
///
/// One definition, because the set is easy to write down wrong: `contains('e')`
/// looks complete and silently misses `1E5`. Every place in the crate that asks
/// where a number's exponent is -- number folding in `util`, trailing-zero
/// detection in `de`, and the predicates here -- reads this rather than spelling
/// the pair out again.
pub(crate) const EXPONENT_MARKERS: [char; 2] = ['e', 'E'];

/// The decimal point in a JSON number. JSON allows exactly this one.
pub(crate) const DECIMAL_POINT: char = '.';

/// The only sign a JSON number may be written with. JSON has no leading `+`.
pub(crate) const MINUS_SIGN: char = '-';

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

    /// Returns the value as an `f64`, or `None` when it has no finite `f64`.
    ///
    /// A valid JSON number can name a magnitude `f64` cannot hold. `1e400` is a
    /// perfectly good JSON number and converts to infinity, and handing that back
    /// would return a value this type's own constructors refuse --
    /// `Number::try_from(f64::INFINITY)` is an error -- so overflow reports `None`
    /// instead of an infinity. serde_json reaches the same answer from both of its
    /// configurations: `None` under `arbitrary_precision`, and without it `1e400`
    /// is rejected at the parse as "number out of range".
    ///
    /// Magnitudes too small to represent are *not* covered and cannot be: `1e-400`
    /// converts to `0.0`, a finite `f64` indistinguishable from a written zero.
    /// Large integers and high-precision decimals lose precision here as they
    /// always have.
    ///
    /// Pinned by `as_f64_never_returns_a_non_finite_value`.
    pub fn as_f64(&self) -> Option<f64> {
        self.0.parse::<f64>().ok().filter(|value| value.is_finite())
    }

    /// Returns `true` if the number was written with an exponent.
    ///
    /// A fact about the spelling, not the value: `1e2` and `100` are the same
    /// number and answer differently.
    pub fn has_exponent(&self) -> bool {
        self.0.contains(EXPONENT_MARKERS)
    }

    /// Returns `true` if the number was written with a decimal point.
    ///
    /// A fact about the spelling, not the value: `1.0` and `1` are the same number
    /// and answer differently.
    pub fn has_decimal(&self) -> bool {
        self.0.contains(DECIMAL_POINT)
    }

    /// Returns `true` if the number was written with a leading minus sign.
    ///
    /// Named for the sign rather than for being negative, because the two part
    /// company at exactly one value. `-0` carries a minus sign, and `-0.0 < 0.0` is
    /// false -- it is not less than zero, while `(-0.0_f64).signum()` is `-1` and
    /// `1.0 / -0.0` is `-inf`. Both readings are defensible, so the name says which
    /// one this is. `std` draws the same line: `is_sign_negative` on floats, which
    /// have a signed zero, and `is_negative` only on integers, which do not.
    ///
    /// This matters here more than most places, since `-0` is a distinction TJSON
    /// carries end to end.
    pub fn is_sign_negative(&self) -> bool {
        self.0.starts_with(MINUS_SIGN)
    }

    /// Returns `true` if the written form is exactly an integer and carries nothing
    /// beyond the integer value.
    ///
    /// JSON writes a number as `[ minus ] int [ frac ] [ exp ]`, and each thing
    /// excluded here is excluded for the same reason -- it is something the text
    /// says that the integer value does not:
    ///
    /// - `1.0` carries a fraction marker
    /// - `1e2` carries exponent notation, though it names a whole number
    /// - `-0` carries a sign, and `-0.0 == 0.0`
    ///
    /// So a `true` here means the spelling is disposable: nothing is lost by
    /// treating this as its integer value. `-0` is the interesting exclusion, since
    /// it is the one case that looks like a plain integer and is not one -- see
    /// [`Number::is_sign_negative`].
    ///
    /// This says nothing about *range*. `999999999999999999999999` is a plain
    /// integer here and [`Number::as_i64`] still returns `None` for it.
    pub fn is_plain_integer(&self) -> bool {
        // Past the first two checks there is no fraction and no exponent, and JSON
        // forbids leading zeros, so the signed zero has exactly one spelling left.
        !self.has_decimal() && !self.has_exponent() && self.0 != "-0"
    }

    /// Returns `true` if the number has no fractional or exponent part.
    #[deprecated(
        since = "0.9.0",
        note = "reads the spelling, not the value -- `1e2` is a whole number and \
                answers false, `-0` answers true. Use `is_plain_integer()` for \
                \"the written form carries nothing beyond the integer value\", or \
                `has_decimal()` / `has_exponent()` to ask the parts directly"
    )]
    pub fn is_integer(&self) -> bool {
        !self.has_decimal() && !self.has_exponent()
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
        for s in ["0", "-0", "1", "-1", "42", "3.45", "-3.45", "1e10", "1E10",
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
        for s in ["42", "-3.45", "1e100", "1E10", "99999999999999999999"] {
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
        let n = Number::try_from(3.45_f64).unwrap();
        assert_eq!(n.as_str(), "3.45");
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

        let n: Number = "3.45".parse().unwrap();
        assert_eq!(n.as_i64(), None);
        assert!((n.as_f64().unwrap() - 3.45).abs() < 1e-10);
    }

    /// `is_integer` is deprecated but still shipped, so its behavior is still a
    /// promise and still gets pinned. The `allow` is here because the test is
    /// deliberately exercising the deprecated method -- it is the only place in the
    /// crate that should need one.
    #[test]
    #[allow(deprecated)]
    fn is_integer_still_reads_the_spelling() {
        assert!("42".parse::<Number>().unwrap().is_integer());
        assert!(!"3.45".parse::<Number>().unwrap().is_integer());
        assert!(!"1e10".parse::<Number>().unwrap().is_integer());

        // The two answers that sent callers to `is_plain_integer` instead.
        assert!(!"1e2".parse::<Number>().unwrap().is_integer());
        assert!("-0".parse::<Number>().unwrap().is_integer());
        assert!(!"-0".parse::<Number>().unwrap().is_plain_integer());
    }

    /// The three spelling predicates, including the cases where spelling and value
    /// part company -- which is the whole reason they are named for the spelling.
    #[test]
    fn spelling_predicates() {
        let has_exponent = |s: &str| s.parse::<Number>().unwrap().has_exponent();
        assert!(has_exponent("1e2"));
        assert!(has_exponent("1E2"), "the capital spelling counts too");
        assert!(has_exponent("1.5e-3"));
        assert!(!has_exponent("100"));
        assert!(!has_exponent("1.5"));

        let has_decimal = |s: &str| s.parse::<Number>().unwrap().has_decimal();
        assert!(has_decimal("1.5"));
        assert!(has_decimal("1.0e2"));
        assert!(!has_decimal("1"));
        assert!(!has_decimal("1e2"), "an exponent is not a decimal point");

        let is_sign_negative = |s: &str| s.parse::<Number>().unwrap().is_sign_negative();
        assert!(is_sign_negative("-1"));
        assert!(is_sign_negative("-0"), "the value this predicate exists for");
        assert!(is_sign_negative("-0.0"));
        assert!(!is_sign_negative("0"));
        assert!(
            !is_sign_negative("1e-5"),
            "the minus belongs to the exponent, not the number"
        );

        // Every exclusion for the same reason: the text says something the integer
        // value does not.
        let is_plain_integer = |s: &str| s.parse::<Number>().unwrap().is_plain_integer();
        assert!(is_plain_integer("42"));
        assert!(is_plain_integer("-1"), "an ordinary negative integer still is one");
        assert!(is_plain_integer("0"));
        assert!(
            is_plain_integer("999999999999999999999999"),
            "form, not range -- as_i64 gives None for this and it is still plain"
        );
        assert!(!is_plain_integer("3.45"), "carries a fraction marker");
        assert!(!is_plain_integer("1e2"), "carries exponent notation");
        assert!(!is_plain_integer("-0"), "carries a sign the value does not have");
    }

    /// The type doc says NaN and infinity are rejected and `try_from(f64)` enforces
    /// it, so the string entrance must not hand one back either. A valid JSON number
    /// can name a magnitude no `f64` holds.
    #[test]
    fn as_f64_never_returns_a_non_finite_value() {
        for too_big in ["1e400", "-1e400", "10e500", "-10e500", "1e999"] {
            let n: Number = too_big
                .parse()
                .expect("a valid JSON number, whatever its magnitude");
            assert_eq!(n.as_f64(), None, "{too_big} has no finite f64");
        }

        // Finite values are unaffected, including the ones that lose precision.
        assert_eq!("0".parse::<Number>().unwrap().as_f64(), Some(0.0));
        assert_eq!("-0".parse::<Number>().unwrap().as_f64(), Some(-0.0));
        assert_eq!("1e308".parse::<Number>().unwrap().as_f64(), Some(1e308));

        // Documented as out of reach: this lands on a finite zero and cannot be
        // told apart from a written one.
        assert_eq!("1e-400".parse::<Number>().unwrap().as_f64(), Some(0.0));
    }
}
