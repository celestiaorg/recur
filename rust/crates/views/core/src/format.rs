//! Compact number labels for UI chrome (`lib/format.ts`).

/// `formatCompactCount`: 460, 1.2k, 184k, 1.6M.
pub fn format_compact_count(n: f64) -> String {
    let v = n.round().max(0.);
    if v < 1000. {
        return format!("{v}");
    }
    let one_decimal = |x: f64| {
        let s = format!("{x:.1}");
        s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
    };
    if v < 10_000. {
        return format!("{}k", one_decimal(v / 1000.));
    }
    if v < 1_000_000. {
        return format!("{}k", (v / 1000.).round());
    }
    format!("{}M", one_decimal(v / 1_000_000.))
}

#[cfg(test)]
mod tests {
    use super::format_compact_count as f;

    #[test]
    fn compact_counts() {
        assert_eq!(f(0.), "0");
        assert_eq!(f(999.), "999");
        assert_eq!(f(1000.), "1k");
        assert_eq!(f(1200.), "1.2k");
        assert_eq!(f(9999.), "10k");
        assert_eq!(f(40_000.), "40k");
        assert_eq!(f(184_200.), "184k");
        assert_eq!(f(1_000_000.), "1M");
        assert_eq!(f(1_600_000.), "1.6M");
        assert_eq!(f(-5.), "0");
    }
}
