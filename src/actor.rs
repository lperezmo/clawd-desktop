//! Clawd's behaviour: walking, acts, rides, falling. Advances in fixed steps
//! of one animation frame (1/12 s) and knows nothing about Win32.

use crate::sprites::{ACTS, FPS, SHEETS, SpriteRef, act_index, frame_count, sheet_index};
use crate::surfaces::{GROUND, Surface, fall_target, nearest_ground, support};

/// splitmix64: small, seedable, good enough for picking acts.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
}

/// Index into `weights` picked with probability proportional to its weight.
pub fn pick_weighted(weights: &[f32], rng: &mut Rng) -> usize {
    let total: f32 = weights.iter().sum();
    let mut r = rng.f32() * total;
    for (i, &w) in weights.iter().enumerate() {
        if w > 0.0 && r < w {
            return i;
        }
        r -= w;
    }
    weights.iter().rposition(|&w| w > 0.0).unwrap_or(0)
}

/// claude.dev's random-pick weights, one per entry of `ACTS`.
pub fn act_weights() -> Vec<f32> {
    ACTS.iter().map(|a| a.weight).collect()
}

/// Weights used after two minutes without input.
pub fn sleepy_weights() -> Vec<f32> {
    ACTS.iter()
        .map(|a| match a.name {
            "snooze" | "meditating" | "headphones" => a.weight.max(1.0),
            _ => 0.0,
        })
        .collect()
}

pub const SLEEP_AFTER_SECS: f32 = 120.0;
const RIDES: [&str; 4] = ["skate", "car", "scooter", "bike"];
const RIDE_CHANCE: f32 = 0.2;
const DT: f32 = 1.0 / FPS as f32;
/// Half of Clawd's body width in sprite pixels: how close to an end he walks.
const HALF_BODY: f32 = 8.0;
const HOP: [i32; 4] = [-2, -3, -2, -1];

/// What the world looks like this step.
pub struct World<'a> {
    pub surfaces: &'a [Surface],
    pub idle_secs: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Walk,
    Act {
        act: usize,
        frame: usize,
        sleepy: bool,
    },
    Ride {
        sheet: usize,
        frame: usize,
        cruise: usize,
        left: f32,
    },
    Fall {
        vy: f32,
        frame: usize,
    },
    Hop {
        step: usize,
    },
}

/// Everything the renderer needs: which frame, which way, where the feet are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pose {
    pub sprite: SpriteRef,
    pub frame: usize,
    pub mirrored: bool,
    /// Clawd's centre line, screen pixels.
    pub x: i32,
    /// The row his feet stand on, screen pixels.
    pub y: i32,
}

pub struct Actor {
    pub x: f32,
    pub y: f32,
    pub dir: f32,
    pub scale: i32,
    owner: isize,
    mode: Mode,
    walk_frame: usize,
    next_act_in: f32,
    walk: usize,
    turning: usize,
    weights: Vec<f32>,
    sleepy: Vec<f32>,
}

impl Actor {
    /// Starts falling from (`x`, `y`).
    pub fn new(x: i32, y: i32, scale: i32, rng: &mut Rng) -> Self {
        Actor {
            x: x as f32,
            y: y as f32,
            dir: if rng.f32() < 0.5 { -1.0 } else { 1.0 },
            scale,
            owner: GROUND,
            mode: Mode::Fall { vy: 0.0, frame: 0 },
            walk_frame: 0,
            next_act_in: rng.range(20.0, 60.0),
            walk: sheet_index("walk"),
            turning: act_index("turning"),
            weights: act_weights(),
            sleepy: sleepy_weights(),
        }
    }

    #[cfg(test)]
    pub fn is_falling(&self) -> bool {
        matches!(self.mode, Mode::Fall { .. })
    }

    pub fn describe(&self) -> String {
        match self.mode {
            Mode::Walk => "walk".into(),
            Mode::Act { act, .. } => format!("act {}", ACTS[act].name),
            Mode::Ride { sheet, .. } => format!("ride {}", SHEETS[sheet].name),
            Mode::Fall { .. } => "fall".into(),
            Mode::Hop { .. } => "hop".into(),
        }
    }

    pub fn pose(&self) -> Pose {
        let mirrored = self.dir < 0.0;
        let (sprite, frame, dy) = match self.mode {
            Mode::Walk => (SpriteRef::Sheet(self.walk), self.walk_frame, 0),
            Mode::Act { act, frame, .. } => (SpriteRef::Act(act), frame, 0),
            Mode::Ride { sheet, frame, .. } => (SpriteRef::Sheet(sheet), frame, 0),
            Mode::Fall { frame, .. } => (SpriteRef::Act(self.turning), frame, 0),
            Mode::Hop { step } => (
                SpriteRef::Sheet(self.walk),
                0,
                HOP[step.min(HOP.len() - 1)] * self.scale,
            ),
        };
        Pose {
            sprite,
            frame,
            mirrored,
            x: self.x.round() as i32,
            y: self.y.round() as i32 + dy,
        }
    }

    fn start_walk(&mut self, rng: &mut Rng) {
        self.mode = Mode::Walk;
        self.walk_frame = 0;
        self.next_act_in = rng.range(20.0, 60.0);
    }

    fn start_fall(&mut self) {
        self.mode = Mode::Fall { vy: 0.0, frame: 0 };
    }

    /// Walking bounds on surface `s` for the centre line, screen pixels.
    fn bounds(&self, s: &Surface) -> (f32, f32) {
        let half = HALF_BODY * self.scale as f32;
        (s.x0 as f32 + half, s.x1 as f32 - half)
    }

    /// Advances one frame.
    pub fn step(&mut self, world: &World, rng: &mut Rng) {
        let xi = self.x.round() as i32;
        let yi = self.y.round() as i32;

        if let Mode::Fall { vy, frame } = self.mode {
            self.fall_step(world, vy, frame, xi, yi, rng);
            return;
        }

        let Some((si, owner)) = support(world.surfaces, xi, yi, self.owner) else {
            self.start_fall();
            return;
        };
        let s = &world.surfaces[si];
        self.owner = owner;
        self.y = s.y as f32;

        match self.mode {
            Mode::Walk => self.walk_step(world, s, rng),
            Mode::Act { act, frame, sleepy } => {
                let sleepy = sleepy && world.idle_secs >= 1.0;
                let next = frame + 1;
                if next < ACTS[act].frames.len() {
                    self.mode = Mode::Act {
                        act,
                        frame: next,
                        sleepy,
                    };
                } else if sleepy {
                    self.mode = Mode::Act { act, frame: 0, sleepy };
                } else {
                    self.start_walk(rng);
                }
            }
            Mode::Ride {
                sheet,
                frame,
                cruise,
                left,
            } => self.ride_step(s, sheet, frame, cruise, left, rng),
            Mode::Hop { step } => {
                if step + 1 < HOP.len() {
                    self.mode = Mode::Hop { step: step + 1 };
                } else {
                    self.start_walk(rng);
                }
            }
            Mode::Fall { .. } => unreachable!(),
        }
    }

    fn fall_step(&mut self, world: &World, vy: f32, frame: usize, xi: i32, yi: i32, rng: &mut Rng) {
        let g = 400.0 * self.scale as f32;
        let vmax = 300.0 * self.scale as f32;
        let vy = (vy + g * DT).min(vmax);
        let ny = self.y + vy * DT;
        match fall_target(world.surfaces, xi, yi) {
            Some(t) if ny >= world.surfaces[t].y as f32 => {
                let s = &world.surfaces[t];
                self.y = s.y as f32;
                self.owner = s.owner_at(xi);
                let (lo, hi) = self.bounds(s);
                if hi > lo {
                    self.x = self.x.clamp(lo, hi);
                }
                self.mode = Mode::Hop { step: 0 };
            }
            Some(_) => {
                self.y = ny;
                self.mode = Mode::Fall {
                    vy,
                    frame: (frame + 1) % frame_count(SpriteRef::Act(self.turning)),
                };
            }
            None => {
                // Nothing below: go stand on the nearest ground.
                if let Some(g) = nearest_ground(world.surfaces, xi) {
                    let s = &world.surfaces[g];
                    let (lo, hi) = self.bounds(s);
                    self.x = if hi > lo {
                        self.x.clamp(lo, hi)
                    } else {
                        ((s.x0 + s.x1) / 2) as f32
                    };
                    self.y = s.y as f32;
                    self.owner = GROUND;
                    self.start_walk(rng);
                }
            }
        }
    }

    fn walk_step(&mut self, world: &World, s: &Surface, rng: &mut Rng) {
        let sheet = &SHEETS[self.walk];
        if world.idle_secs >= SLEEP_AFTER_SECS {
            let act = pick_weighted(&self.sleepy, rng);
            self.mode = Mode::Act {
                act,
                frame: 0,
                sleepy: true,
            };
            return;
        }
        self.next_act_in -= DT;
        if self.next_act_in <= 0.0 {
            if s.ground && s.x1 - s.x0 >= 600 && rng.f32() < RIDE_CHANCE {
                let sheet = sheet_index(RIDES[(rng.next_u64() % RIDES.len() as u64) as usize]);
                let (lo, hi) = self.bounds(s);
                self.dir = if hi - self.x > self.x - lo { 1.0 } else { -1.0 };
                self.mode = Mode::Ride {
                    sheet,
                    frame: 0,
                    cruise: 0,
                    left: rng.range(4.0, 10.0),
                };
            } else {
                let act = pick_weighted(&self.weights, rng);
                self.mode = Mode::Act {
                    act,
                    frame: 0,
                    sleepy: false,
                };
            }
            return;
        }

        // Frames before cruise stand up and set off; then the cruise loop.
        let cruise_start = sheet.cruise.first().copied().unwrap_or(sheet.launch);
        if self.walk_frame + 1 < cruise_start {
            self.walk_frame += 1;
        } else {
            let k = sheet
                .cruise
                .iter()
                .position(|&f| f == self.walk_frame)
                .map_or(0, |k| (k + 1) % sheet.cruise.len());
            self.walk_frame = sheet.cruise[k];
        }
        if self.walk_frame >= sheet.launch {
            self.advance(s, sheet.speed);
        }
    }

    /// Moves along `s`, turning around at its ends. Returns false at an end.
    fn advance(&mut self, s: &Surface, speed: f32) -> bool {
        let (lo, hi) = self.bounds(s);
        if hi <= lo {
            self.x = (s.x0 + s.x1) as f32 / 2.0;
            return false;
        }
        self.x += self.dir * speed * self.scale as f32 * DT;
        if self.x >= hi && self.dir > 0.0 {
            self.x = hi;
            self.dir = -1.0;
            false
        } else if self.x <= lo && self.dir < 0.0 {
            self.x = lo;
            self.dir = 1.0;
            false
        } else {
            true
        }
    }

    fn ride_step(&mut self, s: &Surface, sheet: usize, frame: usize, cruise: usize, left: f32, rng: &mut Rng) {
        let sh = &SHEETS[sheet];
        if !s.ground {
            self.start_walk(rng);
            return;
        }
        let cruise_start = sh.cruise.first().copied().unwrap_or(sh.outro);
        let (frame, cruise, left) = if frame >= sh.outro {
            if frame + 1 >= sh.frames.len() {
                self.start_walk(rng);
                return;
            }
            (frame + 1, cruise, left)
        } else if frame + 1 < cruise_start {
            (frame + 1, cruise, left)
        } else if left <= 0.0 || sh.cruise.is_empty() {
            (sh.outro, cruise, left)
        } else {
            let k = (cruise + 1) % sh.cruise.len();
            (sh.cruise[k], k, left - DT)
        };
        let moving = frame >= sh.launch && frame < sh.outro;
        let dir = self.dir;
        if moving && !self.advance(s, sh.speed) {
            // Hit the end of the ground: get off, still facing the way he came.
            self.dir = dir;
            self.mode = Mode::Ride {
                sheet,
                frame: sh.outro,
                cruise,
                left: 0.0,
            };
            return;
        }
        self.mode = Mode::Ride {
            sheet,
            frame,
            cruise,
            left,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::{MonitorInfo, Rect, WindowInfo, build};

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut c = Rng::new(43);
        assert_ne!(Rng::new(42).next_u64(), c.next_u64());
    }

    #[test]
    fn act_picks_follow_weights() {
        let w = act_weights();
        let total: f32 = w.iter().sum();
        let mut counts = vec![0u32; w.len()];
        let mut rng = Rng::new(7);
        let n = 200_000;
        for _ in 0..n {
            counts[pick_weighted(&w, &mut rng)] += 1;
        }
        for (i, a) in ACTS.iter().enumerate() {
            let want = a.weight / total;
            let got = counts[i] as f32 / n as f32;
            if a.weight == 0.0 {
                assert_eq!(counts[i], 0, "{} has weight 0 but was picked", a.name);
            } else {
                assert!((got - want).abs() < 0.005, "{}: got {got}, want {want}", a.name);
            }
        }
        // Same seed, same first picks.
        let mut r1 = Rng::new(99);
        let mut r2 = Rng::new(99);
        let s1: Vec<_> = (0..20).map(|_| pick_weighted(&w, &mut r1)).collect();
        let s2: Vec<_> = (0..20).map(|_| pick_weighted(&w, &mut r2)).collect();
        assert_eq!(s1, s2);
    }

    #[test]
    fn sleepy_picks_only_rest_acts() {
        let w = sleepy_weights();
        let mut rng = Rng::new(1);
        for _ in 0..1000 {
            let n = ACTS[pick_weighted(&w, &mut rng)].name;
            assert!(matches!(n, "snooze" | "meditating" | "headphones"), "{n}");
        }
    }

    fn mon() -> MonitorInfo {
        MonitorInfo {
            rect: Rect::new(0, 0, 1920, 1080),
            work: Rect::new(0, 0, 1920, 1032),
            primary: true,
        }
    }

    #[test]
    fn falls_onto_window_then_ground_when_it_closes() {
        let mut rng = Rng::new(3);
        let ws = [WindowInfo {
            hwnd: 5,
            rect: Rect::new(200, 500, 1200, 900),
        }];
        let with = build(&ws, &[mon()], 72);
        let mut a = Actor::new(600, 0, 4, &mut rng);
        let world = World {
            surfaces: &with,
            idle_secs: 0.0,
        };
        for _ in 0..60 {
            a.step(&world, &mut rng);
        }
        assert_eq!(a.pose().y, 500);
        assert!(!a.is_falling());

        let without = build(&[], &[mon()], 72);
        let world = World {
            surfaces: &without,
            idle_secs: 0.0,
        };
        a.step(&world, &mut rng);
        assert!(a.is_falling());
        for _ in 0..60 {
            a.step(&world, &mut rng);
        }
        assert_eq!(a.y as i32, 1032);
    }

    #[test]
    fn walks_and_turns_at_the_end() {
        let mut rng = Rng::new(11);
        let ws = [WindowInfo {
            hwnd: 5,
            rect: Rect::new(200, 500, 500, 900),
        }];
        let s = build(&ws, &[mon()], 72);
        let mut a = Actor::new(350, 400, 4, &mut rng);
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
        };
        let (mut min, mut max) = (i32::MAX, i32::MIN);
        for _ in 0..12 * 15 {
            a.step(&world, &mut rng);
            if matches!(a.mode, Mode::Walk) {
                min = min.min(a.pose().x);
                max = max.max(a.pose().x);
            }
        }
        assert_eq!(a.pose().y, 500);
        assert!(min >= 200 + 32 && max <= 500 - 32, "{min}..{max}");
        assert!(max - min > 150, "did not walk both ways: {min}..{max}");
    }

    #[test]
    fn idle_user_gets_a_rest_act() {
        let mut rng = Rng::new(5);
        let s = build(&[], &[mon()], 72);
        let mut a = Actor::new(900, 1032, 4, &mut rng);
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
        };
        for _ in 0..10 {
            a.step(&world, &mut rng);
        }
        let idle = World {
            surfaces: &s,
            idle_secs: 200.0,
        };
        a.step(&idle, &mut rng);
        let d = a.describe();
        assert!(
            d == "act snooze" || d == "act meditating" || d == "act headphones",
            "{d}"
        );
    }
}
