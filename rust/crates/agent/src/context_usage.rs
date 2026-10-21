//! Composer context-meter rules (frontend `lib/contextUsage.ts`).

use recur_domain::ChatItem;

/// The last turn's context fill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextUsage {
    pub input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub context_window: i64,
    /// The provider is summarizing context right now.
    pub compacting: bool,
    /// The session compacted at least once (last turn or live).
    pub compacted: bool,
}

impl ContextUsage {
    /// Prompt-side tokens occupying the window (excludes output).
    pub fn used_tokens(&self) -> i64 {
        self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens
    }

    /// Whether a fill ring can render: the limit is known and there is fill
    /// or a post-compact empty.
    pub fn ready(&self) -> bool {
        self.context_window > 0 && (self.used_tokens() > 0 || self.compacted)
    }

    /// Drops the pre-compact fill: occupied tokens after a compact are
    /// unknown until the next usage frame.
    pub fn after_compact(&self) -> ContextUsage {
        ContextUsage {
            context_window: self.context_window,
            compacted: true,
            ..Default::default()
        }
    }
}

/// A turn footer's usage, or `None` when it cannot fill the ring.
pub fn from_turn(it: &ChatItem, compacting: bool, compacted: bool) -> Option<ContextUsage> {
    let u = ContextUsage {
        input_tokens: it.input_tokens,
        cache_creation_tokens: it.cache_creation_tokens,
        cache_read_tokens: it.cache_read_tokens,
        context_window: it.context_window,
        compacting,
        compacted,
    };
    (u.context_window > 0 && (u.used_tokens() > 0 || u.compacted)).then_some(u)
}

/// The latest usable turn's context stats. A compact newer than that turn
/// resets the fill.
pub fn from_chat_items(items: &[ChatItem]) -> Option<ContextUsage> {
    let mut last_usable = None;
    let mut last_compact = None;
    for (i, it) in items.iter().enumerate() {
        if it.kind == "compact" {
            last_compact = Some(i);
        } else if it.kind == "turn" && from_turn(it, false, false).is_some() {
            last_usable = Some(i);
        }
    }
    let usable = last_usable?;
    let turn = from_turn(&items[usable], false, false)?;
    match last_compact {
        Some(c) if c > usable => Some(turn.after_compact()),
        Some(_) => Some(ContextUsage {
            compacted: true,
            ..turn
        }),
        None => Some(turn),
    }
}

/// Compact token label: 460, 1.2k, 184k, 1.0M style (frontend
/// `formatCompactCount`).
pub fn format_tokens(n: i64) -> String {
    let v = n.max(0);
    if v < 1000 {
        return v.to_string();
    }
    if v < 10_000 {
        return format!("{}k", to_fixed_1(v as f64 / 1000.0));
    }
    if v < 1_000_000 {
        return format!("{}k", (v as f64 / 1000.0 + 0.5).floor() as i64);
    }
    format!("{}M", to_fixed_1(v as f64 / 1_000_000.0))
}

/// JavaScript `x.toFixed(1)` with a trailing `.0` dropped, for finite
/// positive `x` below 1e21: the nearest tenth of the exact binary value,
/// ties rounding up.
fn to_fixed_1(x: f64) -> String {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i64 - 1075;
    let mant = (bits & ((1 << 52) - 1)) | (1 << 52);
    let tenths: u128 = if exp >= 0 {
        ((mant as u128) << exp) * 10
    } else {
        let shift = (-exp) as u32;
        let scaled = mant as u128 * 10;
        let q = scaled >> shift;
        let r = scaled & ((1u128 << shift) - 1);
        if r >= 1u128 << (shift - 1) { q + 1 } else { q }
    };
    match tenths % 10 {
        0 => (tenths / 10).to_string(),
        d => format!("{}.{d}", tenths / 10),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(i: i64, c: i64, w: i64) -> ContextUsage {
        ContextUsage {
            input_tokens: i,
            cache_read_tokens: c,
            context_window: w,
            ..Default::default()
        }
    }

    fn turn(i: i64, c: i64, w: i64) -> ChatItem {
        ChatItem {
            kind: "turn".into(),
            input_tokens: i,
            cache_read_tokens: c,
            context_window: w,
            ..Default::default()
        }
    }

    fn kind(k: &str) -> ChatItem {
        ChatItem {
            kind: k.into(),
            ..Default::default()
        }
    }

    #[test]
    fn ready_rules() {
        assert!(!usage(0, 0, 200_000).ready());
        assert!(!usage(10, 0, 0).ready());
        assert!(usage(10, 0, 200_000).ready());
        let compacted = ContextUsage {
            compacted: true,
            ..usage(0, 0, 200_000)
        };
        assert!(compacted.ready());
        let u = ContextUsage {
            cache_creation_tokens: 2,
            ..usage(10, 8, 100)
        };
        assert_eq!(u.used_tokens(), 20);
    }

    #[test]
    fn from_turn_drops_unfillable_rows() {
        assert_eq!(from_turn(&turn(0, 0, 100), false, false), None);
        assert_eq!(
            from_turn(&turn(12, 3, 100), false, true),
            Some(ContextUsage {
                compacted: true,
                ..usage(12, 3, 100)
            })
        );
    }

    #[test]
    fn after_compact_zeroes_fill() {
        let u = ContextUsage {
            compacting: true,
            ..usage(150_000, 50_000, 200_000)
        };
        assert_eq!(
            u.after_compact(),
            ContextUsage {
                compacted: true,
                ..usage(0, 0, 200_000)
            }
        );
    }

    #[test]
    fn from_chat_items_rules() {
        assert_eq!(from_chat_items(&[]), None);
        let got = from_chat_items(&[
            kind("text"),
            turn(1, 0, 10),
            kind("compact"),
            turn(4, 6, 10),
        ]);
        assert_eq!(
            got,
            Some(ContextUsage {
                compacted: true,
                ..usage(4, 6, 10)
            })
        );
        let got = from_chat_items(&[turn(180_000, 20_000, 200_000), kind("compact")]);
        assert_eq!(
            got,
            Some(ContextUsage {
                compacted: true,
                ..usage(0, 0, 200_000)
            })
        );
        let got = from_chat_items(&[turn(12, 3, 100), turn(0, 0, 100)]);
        assert_eq!(got, Some(usage(12, 3, 100)));
        let got = from_chat_items(&[
            turn(4, 6, 10),
            kind("compact"),
            turn(30, 0, 10),
            turn(0, 0, 10),
        ]);
        assert_eq!(
            got,
            Some(ContextUsage {
                compacted: true,
                ..usage(30, 0, 10)
            })
        );
    }

    #[test]
    fn token_labels_match_js() {
        for (n, want) in [
            (-5, "0"),
            (460, "460"),
            (1000, "1k"),
            (1050, "1.1k"),
            (1150, "1.1k"),
            (1250, "1.3k"),
            (1200, "1.2k"),
            (9999, "10k"),
            (184_400, "184k"),
            (184_500, "185k"),
            (999_999, "1000k"),
            (1_000_000, "1M"),
            (1_250_000, "1.3M"),
            (1_050_000, "1.1M"),
            (12_340_000, "12.3M"),
        ] {
            assert_eq!(format_tokens(n), want, "{n}");
        }
    }
}
