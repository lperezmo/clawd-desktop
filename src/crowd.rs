//! Several Clawds at once. Each has his own actor, RNG stream, pace and
//! timers; the only thing they know about each other is that two walking
//! into each other on the same surface do not pass through: the one with
//! the lower index turns around.

use crate::actor::{Actor, HALF_BODY, Rng, World};
use crate::sprites::{SHEETS, sheet_index};
use crate::surfaces::Surface;

pub const MAX_COUNT: usize = 5;

pub struct Member {
    pub actor: Actor,
    pub rng: Rng,
}

pub struct Crowd {
    pub members: Vec<Member>,
    scale: i32,
}

/// Where `n` Clawds start, as (x, feet y): spread over the surfaces wide
/// enough to walk on, more of them on wider surfaces, evenly spaced on each
/// with a little jitter. Empty when there is nowhere to stand.
pub fn spread_starts(surfaces: &[Surface], n: usize, scale: i32, rng: &mut Rng) -> Vec<(i32, i32)> {
    let half = HALF_BODY as i32 * scale;
    let usable: Vec<(i32, i32, i32)> = surfaces
        .iter()
        .map(|s| (s.x0 + half, s.x1 - half, s.y))
        .filter(|(lo, hi, _)| hi > lo)
        .collect();
    if usable.is_empty() {
        return Vec::new();
    }
    // Hand each Clawd to the surface that would leave the most room per Clawd.
    let mut counts = vec![0usize; usable.len()];
    for _ in 0..n {
        let best = (0..usable.len())
            .max_by(|&a, &b| {
                let room = |i: usize| (usable[i].1 - usable[i].0) as f32 / (counts[i] + 1) as f32;
                room(a).total_cmp(&room(b))
            })
            .unwrap_or(0);
        counts[best] += 1;
    }
    let mut out = Vec::with_capacity(n);
    for (&(lo, hi, y), &c) in usable.iter().zip(&counts) {
        let gap = (hi - lo) as f32 / c.max(1) as f32;
        for k in 0..c {
            let x = lo as f32 + gap * (k as f32 + 0.5) + rng.range(-0.2, 0.2) * gap;
            out.push((x.round().clamp(lo as f32, hi as f32) as i32, y));
        }
    }
    // Interleave so adding one Clawd at a time still spreads them out.
    let mut order: Vec<usize> = (0..out.len()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, (rng.next_u64() % (i as u64 + 1)) as usize);
    }
    order.into_iter().map(|i| out[i]).collect()
}

/// Two walking Clawds on the same surface, close enough that their sprites
/// would overlap and heading at each other: the lower index turns around.
/// No pushing; everyone else ignores everyone else.
pub fn resolve_meetings(actors: &mut [&mut Actor]) {
    for i in 0..actors.len() {
        for j in i + 1..actors.len() {
            let (a, b) = (&*actors[i], &*actors[j]);
            if !a.is_walking() || !b.is_walking() || a.y.round() != b.y.round() {
                continue;
            }
            let reach = SHEETS[sheet_index("walk")].w as f32 * a.scale as f32;
            let dx = b.x - a.x;
            let toward = (dx > 0.0 && a.dir > 0.0 && b.dir < 0.0) || (dx < 0.0 && a.dir < 0.0 && b.dir > 0.0);
            if toward && dx.abs() < reach {
                actors[i].turn_around();
            }
        }
    }
}

impl Crowd {
    pub fn new(scale: i32) -> Crowd {
        Crowd {
            members: Vec::new(),
            scale,
        }
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn set_scale(&mut self, scale: i32) {
        self.scale = scale;
        for m in &mut self.members {
            m.actor.scale = scale;
        }
    }

    /// Grows or shrinks to `n` (1 to `MAX_COUNT`). Survivors are untouched;
    /// newcomers each get their own RNG stream seeded from `seeds` and drop
    /// in a little above a spread-out spot. `fallback` is where to drop in
    /// when there are no surfaces yet.
    pub fn resize(&mut self, n: usize, surfaces: &[Surface], seeds: &mut Rng, fallback: (i32, i32)) {
        let n = n.clamp(1, MAX_COUNT);
        self.members.truncate(n);
        if self.members.len() == n {
            return;
        }
        let have = self.members.len();
        let starts = spread_starts(surfaces, n, self.scale, seeds);
        let drop = SHEETS[sheet_index("walk")].h as i32 * self.scale;
        for k in have..n {
            let mut rng = Rng::new(seeds.next_u64());
            let (x, y) = starts.get(k).map_or(fallback, |&(x, y)| (x, y - drop));
            let actor = Actor::new(x, y, self.scale, &mut rng);
            self.members.push(Member { actor, rng });
        }
    }

    /// Advances everyone one frame.
    pub fn step(&mut self, world: &World) {
        for m in &mut self.members {
            m.actor.step(world, &mut m.rng);
        }
        let mut actors: Vec<&mut Actor> = self.members.iter_mut().map(|m| &mut m.actor).collect();
        resolve_meetings(&mut actors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::{MonitorInfo, Rect, WindowInfo, build};

    fn mon() -> MonitorInfo {
        MonitorInfo {
            rect: Rect::new(0, 0, 1920, 1080),
            work: Rect::new(0, 0, 1920, 1032),
            primary: true,
        }
    }

    fn desk() -> Vec<Surface> {
        let ws = [
            WindowInfo {
                hwnd: 5,
                rect: Rect::new(200, 400, 1100, 900),
            },
            WindowInfo {
                hwnd: 6,
                rect: Rect::new(1200, 300, 1800, 800),
            },
        ];
        build(&ws, &[mon()], 72)
    }

    fn world(s: &[Surface]) -> World<'_> {
        World {
            surfaces: s,
            idle_secs: 0.0,
        }
    }

    #[test]
    fn starts_are_spread_over_surfaces() {
        let s = desk();
        for seed in 0..50 {
            let mut rng = Rng::new(seed);
            let starts = spread_starts(&s, 5, 4, &mut rng);
            assert_eq!(starts.len(), 5);
            let ys: std::collections::BTreeSet<i32> = starts.iter().map(|p| p.1).collect();
            assert!(ys.len() >= 2, "all on one surface: {starts:?}");
            for (i, a) in starts.iter().enumerate() {
                let on = s.iter().find(|s| s.y == a.1 && a.0 >= s.x0 + 32 && a.0 <= s.x1 - 32);
                assert!(on.is_some(), "{a:?} is not on a surface");
                for b in &starts[i + 1..] {
                    assert!(a.1 != b.1 || (a.0 - b.0).abs() >= 100, "{a:?} and {b:?} too close");
                }
            }
        }
        // One surface only: still spaced out.
        let ground = build(&[], &[mon()], 72);
        let mut xs: Vec<i32> = spread_starts(&ground, 5, 4, &mut Rng::new(1))
            .iter()
            .map(|p| p.0)
            .collect();
        xs.sort();
        assert!(xs.windows(2).all(|w| w[1] - w[0] >= 200), "{xs:?}");
        assert!(spread_starts(&[], 3, 4, &mut Rng::new(1)).is_empty());
    }

    /// Two Clawds walking on the ground, facing each other.
    fn facing_pair(s: &[Surface]) -> Crowd {
        let mut c = Crowd::new(4);
        c.resize(2, s, &mut Rng::new(9), (0, 0));
        // Let them land and start walking.
        for _ in 0..60 {
            c.step(&world(s));
        }
        let [a, b] = &mut c.members[..] else { unreachable!() };
        a.actor.x = 800.0;
        a.actor.dir = 1.0;
        b.actor.x = 1000.0;
        b.actor.dir = -1.0;
        a.actor.y = 1032.0;
        b.actor.y = 1032.0;
        c
    }

    #[test]
    fn lower_index_turns_around_when_they_meet() {
        let s = build(&[], &[mon()], 72);
        let mut c = facing_pair(&s);
        let mut turned = false;
        for _ in 0..12 * 20 {
            let (ma, mb) = (&c.members[0].actor, &c.members[1].actor);
            if ma.is_walking() && mb.is_walking() && ma.dir > 0.0 && mb.dir < 0.0 {
                // Never walk through each other while both are walking.
                assert!(mb.x - ma.x > 0.0, "passed through: {} {}", ma.x, mb.x);
            }
            let before = (c.members[0].actor.dir, c.members[1].actor.dir);
            c.step(&world(&s));
            let after = (c.members[0].actor.dir, c.members[1].actor.dir);
            let (ma, mb) = (&c.members[0].actor, &c.members[1].actor);
            if before == (1.0, -1.0) && after == (-1.0, -1.0) && (mb.x - ma.x) < 96.0 + 8.0 {
                turned = true;
                break;
            }
        }
        assert!(turned, "Clawd 0 never turned on meeting Clawd 1");

        // Direct check of the rule, both orders of position.
        let mut c = facing_pair(&s);
        let (a, b) = c.members.split_at_mut(1);
        let (a, b) = (&mut a[0].actor, &mut b[0].actor);
        if a.is_walking() && b.is_walking() {
            a.x = 900.0;
            b.x = 960.0;
            resolve_meetings(&mut [&mut *a, &mut *b]);
            assert_eq!((a.dir, b.dir), (-1.0, -1.0));
            // Walking apart: nothing changes.
            resolve_meetings(&mut [&mut *a, &mut *b]);
            assert_eq!((a.dir, b.dir), (-1.0, -1.0));
            // Far apart: nothing changes.
            a.dir = 1.0;
            b.x = 1400.0;
            resolve_meetings(&mut [&mut *a, &mut *b]);
            assert_eq!((a.dir, b.dir), (1.0, -1.0));
            // Different surface (y): nothing changes.
            b.x = 960.0;
            b.y = 400.0;
            resolve_meetings(&mut [&mut *a, &mut *b]);
            assert_eq!((a.dir, b.dir), (1.0, -1.0));
        } else {
            panic!("pair not walking: {} / {}", a.describe(), b.describe());
        }
    }

    #[test]
    fn resizing_keeps_survivors() {
        let s = desk();
        let mut seeds = Rng::new(77);
        let mut c = Crowd::new(4);
        c.resize(3, &s, &mut seeds, (0, 0));
        assert_eq!(c.len(), 3);
        for _ in 0..100 {
            c.step(&world(&s));
        }
        let snap = |c: &Crowd| -> Vec<(f32, f32, f32, String)> {
            c.members
                .iter()
                .map(|m| (m.actor.x, m.actor.y, m.actor.dir, m.actor.describe()))
                .collect()
        };
        let before = snap(&c);
        let rng_next: Vec<u64> = c.members.iter().map(|m| m.rng.clone().next_u64()).collect();

        c.resize(5, &s, &mut seeds, (0, 0));
        assert_eq!(c.len(), 5);
        assert_eq!(&snap(&c)[..3], &before[..]);
        let after_rng: Vec<u64> = c.members[..3].iter().map(|m| m.rng.clone().next_u64()).collect();
        assert_eq!(after_rng, rng_next);
        // Newcomers start apart from each other and have their own pace.
        let (n3, n4) = (&c.members[3].actor, &c.members[4].actor);
        assert!(n3.x != n4.x || n3.y != n4.y);
        assert!(c.members.iter().all(|m| (0.85..=1.15).contains(&m.actor.speed_mul)));

        c.resize(2, &s, &mut seeds, (0, 0));
        assert_eq!(c.len(), 2);
        assert_eq!(snap(&c)[..], before[..2]);
        c.resize(0, &s, &mut seeds, (0, 0));
        assert_eq!(c.len(), 1, "count is clamped to at least one");
        c.resize(9, &s, &mut seeds, (0, 0));
        assert_eq!(c.len(), MAX_COUNT);
    }

    #[test]
    fn clawds_desynchronise() {
        let s = build(&[], &[mon()], 72);
        let mut c = Crowd::new(4);
        c.resize(5, &s, &mut Rng::new(3), (0, 0));
        let mut paces: Vec<f32> = c.members.iter().map(|m| m.actor.speed_mul).collect();
        paces.dedup();
        assert!(paces.len() > 1);
        let mut seen_different = false;
        for _ in 0..12 * 60 {
            c.step(&world(&s));
            let d: std::collections::BTreeSet<String> = c.members.iter().map(|m| m.actor.describe()).collect();
            seen_different |= d.len() > 1;
        }
        assert!(seen_different);
    }
}
