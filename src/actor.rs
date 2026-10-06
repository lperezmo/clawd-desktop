//! Clawd's behaviour: walking, acts, rides, falling. Advances in fixed steps
//! of one animation frame (1/12 s) and knows nothing about Win32.

use crate::sprites::{ACTS, FPS, SHEETS, SpriteRef, act_index, frame_count, sheet_index};
use crate::surfaces::{GROUND, Surface, fall_target, nearest_ground, support};

/// splitmix64: small, seedable, good enough for picking acts.
#[derive(Clone)]
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
/// Seconds of walking between acts.
pub const ACT_GAP: (f32, f32) = (8.0, 20.0);
/// How often a timer act runs straight into a second one.
pub const CHAIN_CHANCE: f32 = 0.25;
const RIDES: [&str; 4] = ["skate", "car", "scooter", "bike"];
const RIDE_CHANCE: f32 = 0.2;
const DT: f32 = 1.0 / FPS as f32;
/// Half of Clawd's body width in sprite pixels: how close to an end he walks.
pub const HALF_BODY: f32 = 8.0;
const HOP: [i32; 4] = [-2, -3, -2, -1];

/// What the world looks like this step.
pub struct World<'a> {
    pub surfaces: &'a [Surface],
    pub idle_secs: f32,
    /// Multipliers on the site's weights, one per act (time of day, media,
    /// battery, Claude Code's mood). Empty for none.
    pub leans: &'a [f32],
}

/// Why an act is playing, which decides what follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActKind {
    /// The walk timer ran out; one in four chains into a second act.
    Timer,
    /// The second act of a chain; nothing follows it.
    Chain,
    /// The user is away: loops until input comes back.
    Sleepy,
    /// Something happened: over in about three seconds (`reaction_cap`),
    /// then a double hop when `then_hop`.
    Reaction { then_hop: bool },
}

/// Who asked for a reaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The desktop: only while walking or in a timer act, one at a time,
    /// with a short rest between.
    Desktop,
    /// The user (a click, Perform, Claude Code celebrating): anything but a
    /// fall or a drag gives way.
    User,
}

/// Frames a reaction may run: about three seconds, or ten for a work
/// session at the laptop or desktop.
pub const REACTION_FRAMES: usize = 3 * FPS as usize;
pub const WORK_FRAMES: usize = 10 * FPS as usize;
/// A capped act jumps to its last frames this long before the cap, so it
/// still ends on the act's own way back to standing.
const TAIL: usize = 6;
/// Seconds after a reaction before the desktop may start another.
pub const REACT_REST: f32 = 2.0;

pub fn reaction_cap(act: usize) -> usize {
    match ACTS[act].name {
        "laptop" | "desktop" => WORK_FRAMES,
        _ => REACTION_FRAMES,
    }
}

/// The frame after `frame` when `played` frames have been shown so far, for
/// an act of `len` frames capped at `cap`; None when it is over.
pub fn next_act_frame(frame: usize, played: usize, len: usize, cap: usize) -> Option<usize> {
    let played = played + 1;
    if len <= cap {
        return (frame + 1 < len).then_some(frame + 1);
    }
    if played >= cap {
        None
    } else if played == cap - TAIL {
        Some(len - TAIL)
    } else {
        Some(frame + 1)
    }
}

/// `weights` times `leans`, entry by entry; no leans leaves them as they are.
pub fn leaned(weights: &[f32], leans: &[f32]) -> Vec<f32> {
    weights
        .iter()
        .enumerate()
        .map(|(i, &w)| w * leans.get(i).copied().unwrap_or(1.0))
        .collect()
}

/// Whether an act of `kind` that just ended is followed straight away by another.
pub fn chains(kind: ActKind, rng: &mut Rng) -> bool {
    kind == ActKind::Timer && rng.f32() < CHAIN_CHANCE
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Walk,
    Act {
        act: usize,
        frame: usize,
        kind: ActKind,
        played: usize,
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
        twice: bool,
    },
    /// Picked up with the mouse: dangles, turning, wherever he is put.
    Drag {
        frame: usize,
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
    /// This Clawd's pace: 0.85 to 1.15 times the sheet speed.
    pub speed_mul: f32,
    owner: isize,
    mode: Mode,
    walk_frame: usize,
    next_act_in: f32,
    walk: usize,
    turning: usize,
    weights: Vec<f32>,
    sleepy: Vec<f32>,
    /// Seconds until the desktop may start a reaction.
    rest: f32,
}

impl Actor {
    /// Starts falling from (`x`, `y`).
    pub fn new(x: i32, y: i32, scale: i32, rng: &mut Rng) -> Self {
        Actor {
            x: x as f32,
            y: y as f32,
            dir: if rng.f32() < 0.5 { -1.0 } else { 1.0 },
            scale,
            speed_mul: rng.range(0.85, 1.15),
            owner: GROUND,
            mode: Mode::Fall { vy: 0.0, frame: 0 },
            walk_frame: 0,
            next_act_in: rng.range(ACT_GAP.0, ACT_GAP.1),
            walk: sheet_index("walk"),
            turning: act_index("turning"),
            weights: act_weights(),
            sleepy: sleepy_weights(),
            rest: 0.0,
        }
    }

    #[cfg(test)]
    pub fn is_falling(&self) -> bool {
        matches!(self.mode, Mode::Fall { .. })
    }

    /// Walking along a surface (not acting, riding, hopping or falling).
    pub fn is_walking(&self) -> bool {
        matches!(self.mode, Mode::Walk)
    }

    /// One of the site's acts, its weights times `leans`.
    pub fn pick(&self, leans: &[f32], rng: &mut Rng) -> usize {
        pick_weighted(&leaned(&self.weights, leans), rng)
    }

    /// Whether a reaction from `trigger` may start now.
    pub fn can_react(&self, trigger: Trigger) -> bool {
        match self.mode {
            Mode::Fall { .. } | Mode::Drag { .. } => false,
            _ if trigger == Trigger::User => true,
            Mode::Walk => self.rest <= 0.0,
            Mode::Act { kind, .. } => self.rest <= 0.0 && !matches!(kind, ActKind::Reaction { .. }),
            _ => false,
        }
    }

    /// Plays `act` as a reaction now, if `can_react`; then a double hop when
    /// `then_hop`. Returns whether it started.
    pub fn react(&mut self, act: usize, trigger: Trigger, then_hop: bool) -> bool {
        if !self.can_react(trigger) {
            return false;
        }
        self.mode = Mode::Act {
            act,
            frame: 0,
            kind: ActKind::Reaction { then_hop },
            played: 0,
        };
        true
    }

    pub fn is_dragged(&self) -> bool {
        matches!(self.mode, Mode::Drag { .. })
    }

    /// Picked up: whatever he was doing stops and he dangles.
    pub fn grab(&mut self) {
        if !self.is_dragged() {
            self.mode = Mode::Drag { frame: 0 };
        }
    }

    /// Carried to (`x`, `y`): his centre line and feet, screen pixels.
    pub fn drag_to(&mut self, x: i32, y: i32) {
        if self.is_dragged() {
            self.x = x as f32;
            self.y = y as f32;
        }
    }

    /// Let go: he falls onto the first surface below his feet.
    pub fn release(&mut self) {
        if self.is_dragged() {
            self.owner = GROUND;
            self.start_fall();
        }
    }

    pub fn turn_around(&mut self) {
        self.dir = -self.dir;
    }

    pub fn describe(&self) -> String {
        match self.mode {
            Mode::Walk => "walk".into(),
            Mode::Act { act, .. } => format!("act {}", ACTS[act].name),
            Mode::Ride { sheet, .. } => format!("ride {}", SHEETS[sheet].name),
            Mode::Fall { .. } => "fall".into(),
            Mode::Hop { .. } => "hop".into(),
            Mode::Drag { .. } => "drag".into(),
        }
    }

    pub fn pose(&self) -> Pose {
        let mirrored = self.dir < 0.0;
        let (sprite, frame, dy) = match self.mode {
            Mode::Walk => (SpriteRef::Sheet(self.walk), self.walk_frame, 0),
            Mode::Act { act, frame, .. } => (SpriteRef::Act(act), frame, 0),
            Mode::Ride { sheet, frame, .. } => (SpriteRef::Sheet(sheet), frame, 0),
            Mode::Fall { frame, .. } | Mode::Drag { frame } => (SpriteRef::Act(self.turning), frame, 0),
            Mode::Hop { step, .. } => (
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
        self.next_act_in = rng.range(ACT_GAP.0, ACT_GAP.1);
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
        self.rest = (self.rest - DT).max(0.0);
        if let Mode::Drag { frame } = self.mode {
            self.mode = Mode::Drag {
                frame: (frame + 1) % frame_count(SpriteRef::Act(self.turning)),
            };
            return;
        }
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
            Mode::Act {
                act,
                frame,
                kind,
                played,
            } => {
                // Back from away: finish this one and walk on.
                let kind = if kind == ActKind::Sleepy && world.idle_secs < 1.0 {
                    ActKind::Chain
                } else {
                    kind
                };
                let len = ACTS[act].frames.len();
                let cap = match kind {
                    ActKind::Reaction { .. } => reaction_cap(act),
                    _ => usize::MAX,
                };
                if let Some(next) = next_act_frame(frame, played, len, cap) {
                    self.mode = Mode::Act {
                        act,
                        frame: next,
                        kind,
                        played: played + 1,
                    };
                } else if kind == ActKind::Sleepy {
                    self.mode = Mode::Act {
                        act,
                        frame: 0,
                        kind,
                        played: 0,
                    };
                } else if let ActKind::Reaction { then_hop } = kind {
                    self.rest = REACT_REST;
                    if then_hop {
                        self.mode = Mode::Hop { step: 0, twice: true };
                    } else {
                        self.start_walk(rng);
                    }
                } else if chains(kind, rng) {
                    let act = self.pick(world.leans, rng);
                    self.mode = Mode::Act {
                        act,
                        frame: 0,
                        kind: ActKind::Chain,
                        played: 0,
                    };
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
            Mode::Hop { step, twice } => {
                if step + 1 < HOP.len() {
                    self.mode = Mode::Hop { step: step + 1, twice };
                } else if twice {
                    self.mode = Mode::Hop { step: 0, twice: false };
                } else {
                    self.start_walk(rng);
                }
            }
            Mode::Fall { .. } | Mode::Drag { .. } => unreachable!(),
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
                self.mode = Mode::Hop { step: 0, twice: false };
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
                kind: ActKind::Sleepy,
                played: 0,
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
                let act = self.pick(world.leans, rng);
                self.mode = Mode::Act {
                    act,
                    frame: 0,
                    kind: ActKind::Timer,
                    played: 0,
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
        self.x += self.dir * speed * self.speed_mul * self.scale as f32 * DT;
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

    #[test]
    fn one_timer_act_in_four_chains_and_chains_stop_at_two() {
        let mut rng = Rng::new(21);
        let n = 100_000;
        let chained = (0..n).filter(|_| chains(ActKind::Timer, &mut rng)).count();
        let got = chained as f32 / n as f32;
        assert!((got - CHAIN_CHANCE).abs() < 0.01, "{got}");
        for _ in 0..1000 {
            assert!(!chains(ActKind::Chain, &mut rng));
            assert!(!chains(ActKind::Sleepy, &mut rng));
        }
    }

    #[test]
    fn acts_come_every_8_to_20_seconds_and_some_chain() {
        let s = build(&[], &[mon()], 72);
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
            leans: &[],
        };
        let mut gaps = Vec::new();
        let mut chained = 0;
        let mut acts = 0;
        for seed in 0..40 {
            let mut rng = Rng::new(seed);
            let mut a = Actor::new(900, 1032, 4, &mut rng);
            let mut walked = 0;
            let mut prev: Option<Mode> = None;
            for _ in 0..12 * 120 {
                a.step(&world, &mut rng);
                match (prev, a.mode) {
                    (_, Mode::Walk) => walked += 1,
                    (Some(Mode::Walk), Mode::Act { kind, .. }) => {
                        assert_eq!(kind, ActKind::Timer);
                        gaps.push(walked as f32 / 12.0);
                        walked = 0;
                        acts += 1;
                    }
                    (
                        Some(Mode::Act { kind: k0, .. }),
                        Mode::Act {
                            kind: ActKind::Chain, ..
                        },
                    ) if k0 != ActKind::Chain => {
                        assert_eq!(k0, ActKind::Timer);
                        chained += 1;
                    }
                    (_, Mode::Act { .. }) => {}
                    // A ride ends a walk without an act.
                    _ => walked = 0,
                }
                prev = Some(a.mode);
            }
        }
        // The first gap includes the landing hop and the walk's stand-up frames.
        assert!(gaps.iter().all(|&g| (7.5..=21.0).contains(&g)), "{gaps:?}");
        assert!(acts > 150, "{acts}");
        let share = chained as f32 / acts as f32;
        assert!((0.12..0.4).contains(&share), "{chained} chains in {acts} acts");
    }

    /// A Clawd standing on the ground, walking.
    fn walker(rng: &mut Rng, s: &[Surface]) -> Actor {
        let mut a = Actor::new(900, 1032, 4, rng);
        let world = World {
            surfaces: s,
            idle_secs: 0.0,
            leans: &[],
        };
        a.next_act_in = 1000.0;
        for _ in 0..12 {
            a.step(&world, rng);
        }
        assert!(a.is_walking(), "{}", a.describe());
        a
    }

    #[test]
    fn reactions_are_one_at_a_time_and_short() {
        let s = build(&[], &[mon()], 72);
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
            leans: &[],
        };
        let mut rng = Rng::new(2);
        let mut a = walker(&mut rng, &s);
        let notebook = act_index("notebook");
        assert!(ACTS[notebook].frames.len() > REACTION_FRAMES);
        assert!(a.react(notebook, Trigger::Desktop, false));
        // A second desktop reaction waits for the first.
        assert!(!a.can_react(Trigger::Desktop));
        assert!(!a.react(act_index("coffee"), Trigger::Desktop, false));
        let mut frames = 1;
        while matches!(a.mode, Mode::Act { .. }) {
            a.step(&world, &mut rng);
            frames += 1;
        }
        assert_eq!(frames, REACTION_FRAMES + 1, "about three seconds, then walking");
        assert!(a.is_walking());
        // A short rest before the desktop may start another.
        assert!(!a.can_react(Trigger::Desktop));
        for _ in 0..(REACT_REST * FPS as f32) as usize + 1 {
            a.step(&world, &mut rng);
        }
        assert!(a.can_react(Trigger::Desktop), "{}", a.describe());
        // The user does not wait.
        assert!(a.react(act_index("coffee"), Trigger::User, false));
        assert!(a.react(act_index("dancing"), Trigger::User, false));
        assert_eq!(a.describe(), "act dancing");
    }

    #[test]
    fn capped_acts_end_on_their_own_last_frames() {
        let len = 81;
        let cap = REACTION_FRAMES;
        let mut seq = vec![0];
        let (mut f, mut p) = (0, 0);
        while let Some(n) = next_act_frame(f, p, len, cap) {
            seq.push(n);
            f = n;
            p += 1;
        }
        assert_eq!(seq.len(), cap);
        assert_eq!(*seq.last().unwrap(), len - 1);
        assert_eq!(seq[cap - TAIL], len - TAIL);
        // Short acts play whole.
        let (mut f, mut p, mut n) = (0, 0, 1);
        while let Some(x) = next_act_frame(f, p, 20, cap) {
            f = x;
            p += 1;
            n += 1;
        }
        assert_eq!(n, 20);
        // Work sessions run whole, under ten seconds.
        for name in ["laptop", "desktop"] {
            let i = act_index(name);
            assert_eq!(reaction_cap(i), WORK_FRAMES);
            assert!(ACTS[i].frames.len() <= WORK_FRAMES);
        }
    }

    #[test]
    fn no_reaction_during_a_fall_or_a_ride() {
        let s = build(&[], &[mon()], 72);
        let mut rng = Rng::new(4);
        let mut a = Actor::new(900, 0, 4, &mut rng);
        assert!(a.is_falling());
        assert!(!a.react(act_index("coffee"), Trigger::Desktop, false));
        assert!(!a.react(act_index("coffee"), Trigger::User, false));
        let mut a = walker(&mut rng, &s);
        a.mode = Mode::Ride {
            sheet: sheet_index("skate"),
            frame: 0,
            cruise: 0,
            left: 5.0,
        };
        assert!(!a.can_react(Trigger::Desktop));
        a.mode = Mode::Hop { step: 0, twice: false };
        assert!(!a.can_react(Trigger::Desktop));
    }

    #[test]
    fn dragged_he_dangles_then_drops_onto_the_surface_below() {
        let ws = [WindowInfo {
            hwnd: 5,
            rect: Rect::new(200, 600, 1200, 900),
        }];
        let s = build(&ws, &[mon()], 72);
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
            leans: &[],
        };
        let mut rng = Rng::new(8);
        let mut a = walker(&mut rng, &s);
        assert!(a.react(act_index("coffee"), Trigger::Desktop, false));
        a.grab();
        assert!(a.is_dragged());
        // No reaction of any kind while carried.
        assert!(!a.can_react(Trigger::Desktop));
        assert!(!a.react(act_index("dancing"), Trigger::User, true));
        a.drag_to(700, 300);
        for _ in 0..30 {
            a.step(&world, &mut rng);
            assert!(a.is_dragged());
            assert_eq!((a.pose().x, a.pose().y), (700, 300), "stays where he is held");
            assert_eq!(a.pose().sprite, SpriteRef::Act(act_index("turning")));
        }
        a.release();
        assert!(a.is_falling());
        assert!(!a.can_react(Trigger::User), "no reaction during the fall either");
        for _ in 0..60 {
            a.step(&world, &mut rng);
        }
        assert_eq!(a.pose().y, 600, "landed on the window under the cursor");
        // Dropped beside the window: the ground.
        a.grab();
        a.drag_to(1500, 300);
        a.release();
        for _ in 0..60 {
            a.step(&world, &mut rng);
        }
        assert_eq!(a.pose().y, 1032);
    }

    #[test]
    fn a_celebration_ends_in_a_double_hop() {
        let s = build(&[], &[mon()], 72);
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
            leans: &[],
        };
        let mut rng = Rng::new(6);
        let mut a = walker(&mut rng, &s);
        assert!(a.react(act_index("excited"), Trigger::User, true));
        let mut hops = 0;
        for _ in 0..12 * 4 {
            a.step(&world, &mut rng);
            if matches!(a.mode, Mode::Hop { .. }) {
                hops += 1;
            }
        }
        assert_eq!(hops, 2 * HOP.len());
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
            leans: &[],
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
            leans: &[],
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
            leans: &[],
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
            leans: &[],
        };
        for _ in 0..10 {
            a.step(&world, &mut rng);
        }
        let idle = World {
            surfaces: &s,
            idle_secs: 200.0,
            leans: &[],
        };
        a.step(&idle, &mut rng);
        let d = a.describe();
        assert!(
            d == "act snooze" || d == "act meditating" || d == "act headphones",
            "{d}"
        );
    }
}
