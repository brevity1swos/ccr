use chrono::Duration;

/// Parse an age string into a `Duration`. Accepted forms:
/// `Nd` (days), `Nw` (weeks), `Nmo` (months, 30d each), `Ny` (years, 365d each),
/// or a bare integer (= days).
pub fn parse_age(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let split = s.find(|c: char| c.is_alphabetic()).unwrap_or(s.len());
    let (num_part, unit) = s.split_at(split);
    let n: i64 = num_part.trim().parse().ok()?;
    if n < 0 {
        return None;
    }
    let days = match unit.trim() {
        "" | "d" => n,
        "w" => n.checked_mul(7)?,
        "mo" => n.checked_mul(30)?,
        "y" => n.checked_mul(365)?,
        _ => return None,
    };
    Some(Duration::days(days))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_number_means_days() {
        assert_eq!(parse_age("7"), Some(Duration::days(7)));
    }

    #[test]
    fn days_suffix() {
        assert_eq!(parse_age("30d"), Some(Duration::days(30)));
    }

    #[test]
    fn weeks_suffix() {
        assert_eq!(parse_age("2w"), Some(Duration::days(14)));
    }

    #[test]
    fn months_suffix() {
        assert_eq!(parse_age("3mo"), Some(Duration::days(90)));
    }

    #[test]
    fn years_suffix() {
        assert_eq!(parse_age("1y"), Some(Duration::days(365)));
    }

    #[test]
    fn rejects_negative() {
        assert_eq!(parse_age("-5d"), None);
    }

    #[test]
    fn rejects_unknown_unit() {
        assert_eq!(parse_age("5h"), None);
        assert_eq!(parse_age("5foo"), None);
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(parse_age(""), None);
        assert_eq!(parse_age("   "), None);
    }

    #[test]
    fn rejects_nonsense() {
        assert_eq!(parse_age("abc"), None);
        assert_eq!(parse_age("d"), None);
    }
}
