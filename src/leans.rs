//! Leans: multipliers on the site's act weights from what is going on around
//! Clawd. They tilt the normal pick; they never force an act.

use crate::sprites::ACTS;

/// How much one lean multiplies an act's weight.
pub const LEAN: f32 = 3.0;

/// What the leans are worked out from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// Local hour, 0 to 23.
    pub hour: u32,
    /// Something is playing through the system media controls.
    pub media: bool,
    /// On battery power with under 20 percent left.
    pub low_battery: bool,
}

/// Acts the context leans toward, each once per reason.
pub fn reasons(ctx: &Context) -> Vec<&'static str> {
    let mut out = Vec::new();
    if ctx.media {
        out.extend(["headphones", "dancing"]);
    }
    if ctx.low_battery {
        out.push("snooze");
    }
    match ctx.hour {
        7..=9 => out.push("coffee"),
        13..=15 => out.push("sunglasses"),
        23 | 0..=5 => out.push("snooze"),
        _ => {}
    }
    out
}

/// One multiplier per entry of `ACTS`: `LEAN` for each reason that names it.
pub fn leans(ctx: &Context) -> Vec<f32> {
    from_names(&reasons(ctx))
}

/// One multiplier per entry of `ACTS`, `LEAN` for every mention of it in `names`.
pub fn from_names(names: &[&str]) -> Vec<f32> {
    ACTS.iter()
        .map(|a| LEAN.powi(names.iter().filter(|&&n| n == a.name).count() as i32))
        .collect()
}

/// Entry-by-entry product of two lean lists.
#[allow(dead_code)] // used once Claude Code moods land
pub fn combine(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x * y).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::Rng;
    use crate::actor::{act_weights, leaned, pick_weighted};
    use crate::sprites::act_index;

    fn share(leans: &[f32], name: &str, seed: u64) -> f32 {
        let w = leaned(&act_weights(), leans);
        let mut rng = Rng::new(seed);
        let n = 50_000;
        let i = act_index(name);
        (0..n).filter(|_| pick_weighted(&w, &mut rng) == i).count() as f32 / n as f32
    }

    #[test]
    fn clock_leans() {
        let at = |hour| {
            reasons(&Context {
                hour,
                ..Context::default()
            })
        };
        assert_eq!(at(7), ["coffee"]);
        assert_eq!(at(9), ["coffee"]);
        assert!(at(10).is_empty());
        assert_eq!(at(13), ["sunglasses"]);
        assert_eq!(at(15), ["sunglasses"]);
        assert!(at(16).is_empty());
        assert_eq!(at(23), ["snooze"]);
        assert_eq!(at(0), ["snooze"]);
        assert_eq!(at(5), ["snooze"]);
        assert!(at(6).is_empty());
        assert!(at(12).is_empty());
    }

    #[test]
    fn media_and_battery_lean_without_forcing() {
        let ctx = Context {
            hour: 12,
            media: true,
            low_battery: true,
        };
        let l = leans(&ctx);
        assert_eq!(l[act_index("headphones")], LEAN);
        assert_eq!(l[act_index("dancing")], LEAN);
        assert_eq!(l[act_index("snooze")], LEAN);
        assert_eq!(l[act_index("coffee")], 1.0);
        // Late at night on a low battery, snooze is leaned on twice.
        let night = leans(&Context { hour: 2, ..ctx });
        assert_eq!(night[act_index("snooze")], LEAN * LEAN);
    }

    #[test]
    fn leaned_picks_shift_by_the_multiplier() {
        let none = vec![1.0; ACTS.len()];
        let ctx = Context {
            hour: 8,
            ..Context::default()
        };
        let l = leans(&ctx);
        let before = share(&none, "coffee", 3);
        let after = share(&l, "coffee", 3);
        // coffee's weight triples; everything else stays.
        let w = act_weights();
        let total: f32 = w.iter().sum();
        let c = w[act_index("coffee")];
        let want = 3.0 * c / (total + 2.0 * c);
        assert!((before - c / total).abs() < 0.01, "{before}");
        assert!((after - want).abs() < 0.01, "{after} vs {want}");
        // Other acts are still picked: a lean, not a forced act.
        assert!(share(&l, "thinking", 4) > 0.02);
        // Same seed, same picks.
        let w = leaned(&act_weights(), &l);
        let a: Vec<_> = (0..20)
            .map({
                let mut r = Rng::new(8);
                move |_| pick_weighted(&w, &mut r)
            })
            .collect();
        let w = leaned(&act_weights(), &l);
        let b: Vec<_> = (0..20)
            .map({
                let mut r = Rng::new(8);
                move |_| pick_weighted(&w, &mut r)
            })
            .collect();
        assert_eq!(a, b);
    }

    #[test]
    fn combine_multiplies() {
        let a = from_names(&["coffee"]);
        let b = from_names(&["coffee", "snooze"]);
        let c = combine(&a, &b);
        assert_eq!(c[act_index("coffee")], LEAN * LEAN);
        assert_eq!(c[act_index("snooze")], LEAN);
        assert_eq!(c[act_index("laptop")], 1.0);
    }
}
