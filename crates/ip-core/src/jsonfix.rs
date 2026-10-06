//! Cleans floating point noise out of JSON responses (docs/api-contract-m5.md section D).
//!
//! Many values are computed or stored as `f32` and widened to `f64` on the way out, which
//! makes `0.3f32` print as `0.30000001192092896`. [`tidy`] walks a JSON value and
//!
//! * turns every `f64` that is exactly a widened `f32` back into the shortest decimal of that
//!   `f32` (`0.3`), which loses nothing;
//! * rounds any other fractional number below 10^7 to 6 decimals (results of `f64` arithmetic
//!   on such values, e.g. a weighted mean of `f32` scores).

use serde_json::{Number, Value};

/// Decimals kept for numbers that are not widened `f32`s.
const DECIMALS: f64 = 1e6;

pub fn tidy_f64(x: f64) -> f64 {
    if !x.is_finite() || x.fract() == 0.0 || x.abs() >= 1e7 {
        return x;
    }
    let f = x as f32;
    if f as f64 == x {
        if let Ok(y) = format!("{f}").parse::<f64>() {
            return y;
        }
    }
    let r = (x * DECIMALS).round() / DECIMALS;
    if r.is_finite() {
        r
    } else {
        x
    }
}

/// Applies [`tidy_f64`] to every floating point number in `v`.
pub fn tidy(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if n.is_f64() {
                if let Some(x) = n.as_f64() {
                    let y = tidy_f64(x);
                    if y != x {
                        if let Some(n2) = Number::from_f64(y) {
                            *n = n2;
                        }
                    }
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(tidy),
        Value::Object(o) => o.values_mut().for_each(tidy),
        _ => {}
    }
}

/// Cheap byte scan: `false` means [`tidy`] would leave the document unchanged, so callers can
/// skip the parse / re-serialise round trip. Conservative (false positives only cost time):
/// it looks for a number with 7+ fraction digits (`tidy` leaves anything shorter alone) or a
/// negative exponent (`1e-7`, which `tidy` rounds to 0).
pub fn needs_tidy(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' && i > 0 && bytes[i - 1].is_ascii_digit() {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if digits >= 7 {
                return true;
            }
            i += 1 + digits;
            continue;
        }
        if (b == b'e' || b == b'E')
            && i > 0
            && bytes[i - 1].is_ascii_digit()
            && bytes.get(i + 1) == Some(&b'-')
            && bytes.get(i + 2).is_some_and(u8::is_ascii_digit)
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Serialises `value` with [`tidy`] applied, skipping the extra pass when nothing needs it.
pub fn to_tidy_string<T: serde::Serialize>(value: &T) -> serde_json::Result<String> {
    let text = serde_json::to_string(value)?;
    if !needs_tidy(text.as_bytes()) {
        return Ok(text);
    }
    let mut v: Value = serde_json::from_str(&text)?;
    tidy(&mut v);
    serde_json::to_string(&v)
}

/// Longest run of fraction digits among the numbers of `v` (diagnostics and tests).
pub fn max_fraction_digits(v: &Value) -> usize {
    match v {
        Value::Number(n) if n.is_f64() => {
            let s = n.to_string();
            s.split_once('.')
                .map(|(_, f)| f.split(['e', 'E']).next().unwrap_or("").len())
                .unwrap_or(0)
        }
        Value::Array(a) => a.iter().map(max_fraction_digits).max().unwrap_or(0),
        Value::Object(o) => o.values().map(max_fraction_digits).max().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn needs_tidy_matches_what_tidy_changes() {
        for (text, expect) in [
            (r#"{"a":0.3,"b":[1.25,7],"c":"IMG_1.jpg"}"#, false),
            (r#"{"a":0.1234567}"#, true),
            (r#"{"a":0.123456}"#, false),
            (r#"{"a":0.30000001192092896}"#, true),
            (r#"{"a":3.2e-8}"#, true),
            (r#"{"v":"3e5b2c","x":1e300}"#, false),
        ] {
            assert_eq!(needs_tidy(text.as_bytes()), expect, "{text}");
            let mut v: Value = serde_json::from_str(text).unwrap();
            let before = v.clone();
            tidy(&mut v);
            if !expect {
                assert_eq!(v, before, "{text}");
            }
        }
        assert_eq!(
            to_tidy_string(&serde_json::json!({"a": 0.3f32 as f64})).unwrap(),
            r#"{"a":0.3}"#
        );
    }

    #[test]
    fn widened_f32_noise_is_removed() {
        let mut v = json!({"a": 0.3f32 as f64, "b": [0.2f32 as f64, 1.5], "c": 7, "d": 0.1});
        assert!(max_fraction_digits(&v) > 8);
        tidy(&mut v);
        assert_eq!(v, json!({"a": 0.3, "b": [0.2, 1.5], "c": 7, "d": 0.1}));
    }

    #[test]
    fn f64_arithmetic_noise_is_rounded() {
        let x = (0.1f32 as f64 + 0.2f32 as f64) / 3.0;
        let mut v = json!([x]);
        tidy(&mut v);
        assert!(max_fraction_digits(&v) <= 6, "{v}");
        assert!((v[0].as_f64().unwrap() - x).abs() < 1e-6);
    }

    #[test]
    fn large_and_integral_numbers_are_untouched() {
        let mut v = json!([1.7e12, 3.0, 12345678.9, 2.5e-9]);
        let before = v.clone();
        tidy(&mut v);
        assert_eq!(v[0], before[0]);
        assert_eq!(v[1], before[1]);
        assert_eq!(v[2], before[2]);
    }
}
