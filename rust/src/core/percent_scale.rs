//! Resolve whether a provider reports usage as whole percentages or as
//! fractions of a limit, from the raw values in one response.

/// Detect whether a response reports usage as fractions of a limit (`0.23` =
/// 23%) or whole percentages (`23` = 23%).
///
/// Two things settle it, and only real evidence in the payload counts:
///
/// - a value above `1.0` can only be a percentage, since a fraction never
///   exceeds the limit;
/// - a value strictly between `0` and `1` can only be a fraction, since these
///   APIs report whole percentages.
///
/// With neither, every window is `0` or `1` and the response is genuinely
/// ambiguous. That is read as percentages, because the alternative is worse in
/// practice: an account that has just been used reports `1` for 1%, and calling
/// that a fraction renders a barely-touched window as **100% used**, which is
/// what the per-window `<= 1.0` rule previously did.
pub fn detect_fraction_scale(values: impl IntoIterator<Item = f64>) -> bool {
    let mut saw_fraction = false;
    for value in values {
        if !value.is_finite() {
            continue;
        }
        if value > 1.0 {
            return false;
        }
        if value > 0.0 && value < 1.0 {
            saw_fraction = true;
        }
    }
    saw_fraction
}

/// Convert one raw reported value to a 0-100 percentage.
pub fn to_percent(raw: f64, fraction_scale: bool) -> f64 {
    if fraction_scale {
        (raw * 100.0).clamp(0.0, 100.0)
    } else {
        raw.clamp(0.0, 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_scale_from_evidence() {
        assert!(!detect_fraction_scale([50.0, 3.0, 0.0]));
        assert!(detect_fraction_scale([0.5, 0.0, 0.0]));
        assert!(detect_fraction_scale([0.14, 0.0]));
        assert!(!detect_fraction_scale([1.0, 0.0]));
        assert!(!detect_fraction_scale([0.0, 0.0]));
        assert!(!detect_fraction_scale([]));
    }

    #[test]
    fn converts_with_selected_scale() {
        assert!((to_percent(1.0, false) - 1.0).abs() < 0.001);
        assert!((to_percent(1.0, true) - 100.0).abs() < 0.001);
        assert!((to_percent(0.14, true) - 14.0).abs() < 0.001);
        assert!((to_percent(23.0, false) - 23.0).abs() < 0.001);
    }
}
