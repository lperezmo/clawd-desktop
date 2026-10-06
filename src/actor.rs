//! Clawd's behaviour: walking, acts, rides, falling. Advances in fixed steps
//! of one animation frame (1/12 s) and knows nothing about Win32.

use crate::sprites::{ACTS, FPS, SHEETS, SpriteRef, act_index, frame_count, sheet_index};
use crate::surfaces::{GROUND, Rect, Surface, fall_target, nearest_ground, support};

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
/// Sprite rows from Clawd's feet to the top of his head while he stands:
/// where a Clawd riding on him stands.
pub const HEAD: f32 = 16.0;
/// Frames a startled Clawd stands with the "?!" over his head.
pub const STARTLE_FRAMES: usize = 15;
/// Of those, the first frames shake him from side to side.
const SHAKE_FRAMES: usize = 6;
/// Frames a Clawd rides on another before he gets down.
pub const PERCH_FRAMES: usize = 18;

/// Zero g: fastest he drifts, sprite pixels per second.
pub const FLOAT_CAP: f32 = 8.0;
/// How fast he lifts off while suiting up, sprite pixels per second.
const LIFT_SPEED: f32 = 7.0;
/// Seconds of drifting about, between suiting up and coming back down.
pub const FLOAT_CRUISE: (f32, f32) = (6.0, 10.0);
/// Seconds between changes of drift.
const DRIFT_EVERY: (f32, f32) = (1.5, 4.0);
/// How much of the gap to the new drift he closes each frame.
const DRIFT_EASE: f32 = 0.08;
/// A passenger's short float eases faster, so it shows.
const PASSENGER_EASE: f32 = 0.25;
/// The soft bob while floating: sprite pixels up, and frames per bob.
const BOB: f32 = 2.0;
const BOB_FRAMES: f32 = 36.0;
/// A Clawd beside a floater floats along this long, then drops: well
/// inside three seconds with the fall and the landing hop.
pub const PASSENGER_FRAMES: usize = 20;

/// How a Clawd feels after meeting another. It leans his next act, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feel {
    /// After being shoved.
    Annoyed,
    /// After company: a chat, a ride, working side by side.
    Happy,
}

/// What a feeling leans toward.
pub const ANNOYED_ACTS: [&str; 4] = ["watch", "turning", "coffee", "meditating"];
pub const HAPPY_ACTS: [&str; 5] = ["dancing", "dancingHappy", "heart", "jumpingHappy", "rose"];
/// A feeling is gone after this long, used or not.
pub const FEEL_SECS: f32 = 60.0;

/// One multiplier per act for a feeling.
pub fn feel_leans(f: Feel) -> Vec<f32> {
    crate::leans::from_names(match f {
        Feel::Annoyed => &ANNOYED_ACTS,
        Feel::Happy => &HAPPY_ACTS,
    })
}

/// What a leap ends in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    /// Walks on.
    Walk,
    /// Stands with a shake and a "?!", then walks on.
    Startled,
    /// On another Clawd's head, riding him.
    Perch,
}

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
    /// Stands still facing `dir` (meeting another Clawd), then walks on,
    /// turning around first when `then_turn`. `startled` adds the "?!".
    Stand {
        left: usize,
        then_turn: bool,
        startled: bool,
        t: usize,
    },
    /// A hop from `from_x` to `to_x` over `steps` frames. Lifts are sprite
    /// pixels above the surface: a leap can start or end on a Clawd's head.
    Leap {
        from_x: f32,
        to_x: f32,
        from_lift: f32,
        to_lift: f32,
        peak: f32,
        step: usize,
        steps: usize,
        then: Landing,
    },
    /// Riding on another Clawd's head; the crowd moves him along.
    Perch {
        left: usize,
    },
    /// Zero g: drifting through the air, surfaces ignored.
    Float(Float),
}

/// A float: the space sheet played in the air, or a short float along
/// beside someone who is playing it (`passenger`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Float {
    frame: usize,
    cruise: usize,
    /// Seconds of drifting left before he heads into the outro.
    left: f32,
    /// Frames since he lifted off.
    t: usize,
    /// Drift now and the drift he is easing toward, sprite pixels per second.
    v: (f32, f32),
    target: (f32, f32),
    /// Seconds until the drift changes.
    next: f32,
    /// The work area of the monitor he is on: he stays inside it.
    area: Rect,
    passenger: bool,
}

/// The work area that (`x`, `y`) is in, or the one under `x`.
fn area_at(areas: &[Rect], x: i32, y: i32) -> Option<Rect> {
    let under = |r: &&Rect| x >= r.left && x < r.right;
    areas
        .iter()
        .filter(under)
        .find(|r| y >= r.top && y <= r.bottom)
        .or_else(|| areas.iter().find(under))
        .copied()
}

/// Keeps one coordinate inside `lo..=hi`: hitting a wall turns the drift
/// back inward, and from outside (lifting off near an edge) he glides back
/// in `edge` screen pixels a frame.
fn wall(p: &mut f32, old: f32, v: &mut f32, target: &mut f32, lo: f32, hi: f32, edge: f32) {
    if *p < lo {
        *p = if old < lo { (old + edge).min(lo) } else { lo };
        *v = v.abs();
        *target = target.abs();
    } else if *p > hi {
        *p = if old > hi { (old - edge).max(hi) } else { hi };
        *v = -v.abs();
        *target = -target.abs();
    }
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
    /// A "?!" bubble over his head.
    pub mark: bool,
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
    space: usize,
    turning: usize,
    weights: Vec<f32>,
    sleepy: Vec<f32>,
    /// Seconds until the desktop may start a reaction.
    rest: f32,
    /// Seconds since he last set off walking.
    walking_for: f32,
    /// How he feels after meeting another Clawd, and for how much longer.
    feel: Option<(Feel, f32)>,
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
            space: sheet_index("space"),
            turning: act_index("turning"),
            weights: act_weights(),
            sleepy: sleepy_weights(),
            rest: 0.0,
            walking_for: 0.0,
            feel: None,
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

    /// One of the site's acts, its weights times `leans` and his feeling,
    /// which this pick uses up.
    pub fn pick(&mut self, leans: &[f32], rng: &mut Rng) -> usize {
        let mut w = leaned(&self.weights, leans);
        if let Some((f, _)) = self.feel.take() {
            w = leaned(&w, &feel_leans(f));
        }
        pick_weighted(&w, rng)
    }

    #[cfg(test)]
    pub fn feel(&self) -> Option<Feel> {
        self.feel.map(|(f, _)| f)
    }

    pub fn set_feel(&mut self, f: Feel) {
        self.feel = Some((f, FEEL_SECS));
    }

    /// Seconds since he last set off walking; zero when not walking.
    pub fn walking_for(&self) -> f32 {
        if self.is_walking() { self.walking_for } else { 0.0 }
    }

    /// Floating in zero g, on his own space act or along beside one.
    pub fn is_floating(&self) -> bool {
        matches!(self.mode, Mode::Float(_))
    }

    /// Starts the space act with gravity off: he lifts off gently, drifts
    /// about inside the work area he is in (one of `areas`), then drops
    /// back down when it ends. False when he is held, falling, already
    /// floating, or not over any work area.
    pub fn float(&mut self, areas: &[Rect], rng: &mut Rng) -> bool {
        self.lift_off(areas, false, rng)
    }

    /// Floats along for a moment beside one who started the space act,
    /// then drops. Only from his feet: not while held, falling, leaping or
    /// riding on someone.
    pub fn float_along(&mut self, areas: &[Rect], rng: &mut Rng) -> bool {
        if self.is_social() {
            return false;
        }
        self.lift_off(areas, true, rng)
    }

    fn lift_off(&mut self, areas: &[Rect], passenger: bool, rng: &mut Rng) -> bool {
        if matches!(self.mode, Mode::Fall { .. } | Mode::Drag { .. } | Mode::Float(_)) {
            return false;
        }
        // From where he is seen: up on a leap or a head.
        let y = self.y - self.lift() * self.scale as f32;
        let Some(area) = area_at(areas, self.x.round() as i32, y.round() as i32) else {
            return false;
        };
        self.y = y;
        self.owner = GROUND;
        self.mode = Mode::Float(Float {
            frame: 0,
            cruise: 0,
            left: rng.range(FLOAT_CRUISE.0, FLOAT_CRUISE.1),
            t: 0,
            v: (0.0, 0.0),
            target: (0.0, -LIFT_SPEED),
            next: 0.0,
            area,
            passenger,
        });
        true
    }

    /// Screen pixels the bob lifts him at frame `t` of a float.
    fn bob(&self, f: &Float) -> f32 {
        let amp = if f.passenger { BOB / 2.0 } else { BOB };
        let ramp = (f.t as f32 / BOB_FRAMES).min(1.0);
        let wave = (1.0 - (std::f32::consts::TAU * f.t as f32 / BOB_FRAMES).cos()) / 2.0;
        amp * ramp * wave * self.scale as f32
    }

    /// The act he is playing, if any.
    pub fn act_name(&self) -> Option<&'static str> {
        match self.mode {
            Mode::Act { act, .. } => Some(ACTS[act].name),
            _ => None,
        }
    }

    /// Frames left of the act he is playing, if any.
    pub fn act_frames_left(&self) -> Option<usize> {
        match self.mode {
            Mode::Act { act, frame, .. } => Some(ACTS[act].frames.len().saturating_sub(frame + 1)),
            _ => None,
        }
    }

    /// Busy with another Clawd: standing for a chat or a startle, leaping,
    /// or riding. Each of these is over within about three seconds.
    pub fn is_social(&self) -> bool {
        matches!(self.mode, Mode::Stand { .. } | Mode::Leap { .. } | Mode::Perch { .. })
    }

    #[cfg(test)]
    pub fn is_startled(&self) -> bool {
        matches!(self.mode, Mode::Stand { startled: true, .. })
    }

    /// Frames left riding, while he is on another Clawd's head.
    pub fn perch_left(&self) -> Option<usize> {
        match self.mode {
            Mode::Perch { left } => Some(left),
            _ => None,
        }
    }

    /// What the leap he is in ends in, if he is leaping.
    pub fn leap_landing(&self) -> Option<Landing> {
        match self.mode {
            Mode::Leap { then, .. } => Some(then),
            _ => None,
        }
    }

    /// Stops for `frames`, facing the way he is facing, then walks on,
    /// turning around first when `then_turn`.
    pub fn stand(&mut self, frames: usize, then_turn: bool) {
        self.mode = Mode::Stand {
            left: frames.max(1),
            then_turn,
            startled: false,
            t: 0,
        };
    }

    /// Hops to `to_x` over `steps` frames, from wherever he is now (his feet
    /// or another Clawd's head) to `to_lift` sprite pixels above the surface.
    pub fn leap(&mut self, to_x: f32, to_lift: f32, peak: f32, steps: usize, then: Landing) {
        self.mode = Mode::Leap {
            from_x: self.x,
            to_x,
            from_lift: self.lift(),
            to_lift,
            peak,
            step: 0,
            steps: steps.max(1),
            then,
        };
    }

    /// Moves the end of the leap he is in (the Clawd he is climbing walks on).
    pub fn retarget(&mut self, x: f32) {
        if let Mode::Leap { ref mut to_x, .. } = self.mode {
            *to_x = x;
        }
    }

    /// Plays `act` now as an ordinary act from the walk timer, with its
    /// normal length and the usual chance of a second.
    pub fn sit_down(&mut self, act: usize) {
        self.mode = Mode::Act {
            act,
            frame: 0,
            kind: ActKind::Timer,
            played: 0,
        };
    }

    /// Puts off his next act by at least `secs`.
    pub fn postpone_act(&mut self, secs: f32) {
        self.next_act_in = self.next_act_in.max(secs);
    }

    /// Sprite pixels his feet are above the surface: on a leap or a head.
    fn lift(&self) -> f32 {
        match self.mode {
            Mode::Leap {
                from_lift,
                to_lift,
                peak,
                step,
                steps,
                ..
            } => {
                let t = step as f32 / steps as f32;
                from_lift + (to_lift - from_lift) * t + peak * 4.0 * t * (1.0 - t)
            }
            Mode::Perch { .. } => HEAD,
            _ => 0.0,
        }
    }

    /// Whether a reaction from `trigger` may start now.
    pub fn can_react(&self, trigger: Trigger) -> bool {
        match self.mode {
            Mode::Fall { .. } | Mode::Drag { .. } | Mode::Leap { .. } | Mode::Perch { .. } | Mode::Float(_) => false,
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
            Mode::Stand { startled: true, .. } => "startled".into(),
            Mode::Stand { .. } => "stand".into(),
            Mode::Leap { .. } => "leap".into(),
            Mode::Perch { .. } => "perch".into(),
            Mode::Float(Float { passenger: true, .. }) => "float along".into(),
            Mode::Float(_) => "float".into(),
        }
    }

    pub fn pose(&self) -> Pose {
        let mirrored = self.dir < 0.0;
        let lift = (self.lift() * self.scale as f32).round() as i32;
        let (mut dx, mut mark) = (0, false);
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
            Mode::Stand { startled, t, .. } => {
                mark = startled;
                if startled && t < SHAKE_FRAMES {
                    dx = if t % 2 == 0 { self.scale } else { -self.scale };
                }
                (SpriteRef::Sheet(self.walk), 0, 0)
            }
            Mode::Leap { .. } | Mode::Perch { .. } => (SpriteRef::Sheet(self.walk), 0, -lift),
            Mode::Float(f) => {
                let dy = -self.bob(&f).round() as i32;
                if f.passenger {
                    let n = frame_count(SpriteRef::Act(self.turning));
                    (SpriteRef::Act(self.turning), f.t % n, dy)
                } else {
                    (SpriteRef::Sheet(self.space), f.frame, dy)
                }
            }
        };
        Pose {
            sprite,
            frame,
            mirrored,
            x: self.x.round() as i32 + dx,
            y: self.y.round() as i32 + dy,
            mark,
        }
    }

    fn start_walk(&mut self, rng: &mut Rng) {
        self.mode = Mode::Walk;
        self.walk_frame = 0;
        self.walking_for = 0.0;
        self.next_act_in = rng.range(ACT_GAP.0, ACT_GAP.1);
    }

    /// Walks on after meeting another Clawd, keeping his act timer.
    fn resume_walk(&mut self) {
        self.mode = Mode::Walk;
        self.walk_frame = 0;
        self.walking_for = 0.0;
        self.next_act_in = self.next_act_in.max(1.0);
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
        if let Some((_, left)) = self.feel.as_mut() {
            *left -= DT;
            if *left <= 0.0 {
                self.feel = None;
            }
        }
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
        if let Mode::Float(f) = self.mode {
            self.float_step(f, rng);
            return;
        }

        let Some((si, owner)) = support(world.surfaces, xi, yi, self.owner) else {
            // Falls from where he is seen: up on a leap or a head.
            self.y -= self.lift() * self.scale as f32;
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
            Mode::Stand {
                left,
                then_turn,
                startled,
                t,
            } => {
                if left > 1 {
                    self.mode = Mode::Stand {
                        left: left - 1,
                        then_turn,
                        startled,
                        t: t + 1,
                    };
                } else {
                    if then_turn {
                        self.turn_around();
                    }
                    self.resume_walk();
                }
            }
            Mode::Leap {
                from_x,
                to_x,
                from_lift,
                to_lift,
                peak,
                step,
                steps,
                then,
            } => {
                let step = step + 1;
                let t = step as f32 / steps as f32;
                self.x = from_x + (to_x - from_x) * t;
                let (lo, hi) = self.bounds(s);
                if hi > lo {
                    self.x = self.x.clamp(lo, hi);
                }
                self.mode = if step < steps {
                    Mode::Leap {
                        from_x,
                        to_x,
                        from_lift,
                        to_lift,
                        peak,
                        step,
                        steps,
                        then,
                    }
                } else {
                    match then {
                        Landing::Walk => {
                            self.resume_walk();
                            return;
                        }
                        Landing::Startled => Mode::Stand {
                            left: STARTLE_FRAMES,
                            then_turn: false,
                            startled: true,
                            t: 0,
                        },
                        Landing::Perch => Mode::Perch { left: PERCH_FRAMES },
                    }
                };
            }
            Mode::Perch { left } => {
                if left == 0 {
                    // Nobody got him down: hop off forward on his own.
                    let to = self.x + self.dir * 2.0 * HALF_BODY * self.scale as f32;
                    self.leap(to, 0.0, 4.0, 6, Landing::Walk);
                } else {
                    self.mode = Mode::Perch { left: left - 1 };
                }
            }
            Mode::Fall { .. } | Mode::Drag { .. } | Mode::Float(_) => unreachable!(),
        }
    }

    /// One frame of zero g. Surfaces do not matter up here; only the work
    /// area's edges, which he bounces softly off. When the act is over he
    /// falls like any other fall onto whatever is below.
    fn float_step(&mut self, mut f: Float, rng: &mut Rng) {
        let sh = &SHEETS[self.space];
        f.t += 1;
        let over = if f.passenger {
            f.t >= PASSENGER_FRAMES
        } else if f.frame >= sh.outro {
            f.frame += 1;
            f.frame >= sh.frames.len()
        } else if f.frame + 1 < sh.launch {
            f.frame += 1;
            false
        } else if f.left <= 0.0 || sh.cruise.is_empty() {
            f.frame = sh.outro;
            false
        } else {
            f.cruise = if f.frame < sh.launch {
                0
            } else {
                (f.cruise + 1) % sh.cruise.len()
            };
            f.frame = sh.cruise[f.cruise];
            f.left -= DT;
            false
        };
        if over {
            // Down from where he is seen, bob and all.
            self.y -= self.bob(&f);
            self.owner = GROUND;
            self.start_fall();
            return;
        }

        let lifting = if f.passenger {
            f.t < PASSENGER_FRAMES * 2 / 3
        } else {
            f.frame < sh.launch
        };
        if lifting {
            f.target = (0.0, -LIFT_SPEED);
        } else if f.passenger || f.frame >= sh.outro {
            f.target = (0.0, 0.0);
        } else {
            f.next -= DT;
            if f.next <= 0.0 {
                // A new drift; now and then he turns the other way.
                if rng.f32() < 0.4 {
                    self.dir = -self.dir;
                }
                f.target = (
                    self.dir * rng.range(1.5, 0.75 * FLOAT_CAP),
                    rng.range(-0.7, 0.35) * FLOAT_CAP,
                );
                f.next = rng.range(DRIFT_EVERY.0, DRIFT_EVERY.1);
            }
        }
        let ease = if f.passenger { PASSENGER_EASE } else { DRIFT_EASE };
        f.v.0 += (f.target.0 - f.v.0) * ease;
        f.v.1 += (f.target.1 - f.v.1) * ease;
        let speed = (f.v.0 * f.v.0 + f.v.1 * f.v.1).sqrt();
        if speed > FLOAT_CAP {
            f.v.0 *= FLOAT_CAP / speed;
            f.v.1 *= FLOAT_CAP / speed;
        }

        let scale = self.scale as f32;
        let (ox, oy) = (self.x, self.y);
        self.x += f.v.0 * scale * DT;
        self.y += f.v.1 * scale * DT;
        let (w, h) = if f.passenger {
            (SHEETS[self.walk].w, SHEETS[self.walk].h)
        } else {
            (sh.w, sh.h)
        };
        let half = (w / 2) as f32 * scale;
        let a = f.area;
        let edge = 0.4 * FLOAT_CAP * scale * DT;
        let was = f.target.0;
        wall(
            &mut self.x,
            ox,
            &mut f.v.0,
            &mut f.target.0,
            a.left as f32 + half,
            a.right as f32 - half,
            edge,
        );
        if f.target.0 * was < 0.0 {
            // Bounced off a side: he turns to face the new way.
            self.dir = -self.dir;
        }
        wall(
            &mut self.y,
            oy,
            &mut f.v.1,
            &mut f.target.1,
            a.top as f32 + (h as f32 + BOB) * scale,
            a.bottom as f32,
            edge,
        );
        self.mode = Mode::Float(f);
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
        self.walking_for += DT;
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
    fn a_feeling_leans_the_next_pick_once_then_fades() {
        let s = build(&[], &[mon()], 72);
        let mut rng = Rng::new(12);
        let mut a = walker(&mut rng, &s);
        for (feel, names) in [(Feel::Happy, &HAPPY_ACTS[..]), (Feel::Annoyed, &ANNOYED_ACTS[..])] {
            let (mut with, mut without) = (0, 0);
            for _ in 0..20_000 {
                a.set_feel(feel);
                with += names.contains(&ACTS[a.pick(&[], &mut rng)].name) as u32;
                assert_eq!(a.feel(), None, "used up by one pick");
                without += names.contains(&ACTS[a.pick(&[], &mut rng)].name) as u32;
            }
            assert!(with as f32 > 2.0 * without as f32, "{feel:?}: {with} vs {without}");
        }
        // Unused, it fades after a minute.
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
            leans: &[],
        };
        a.set_feel(Feel::Annoyed);
        a.next_act_in = 1000.0;
        for _ in 0..(FEEL_SECS * FPS as f32) as usize - 2 {
            a.step(&world, &mut rng);
        }
        assert_eq!(a.feel(), Some(Feel::Annoyed));
        for _ in 0..3 {
            a.step(&world, &mut rng);
        }
        assert_eq!(a.feel(), None);
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

    // ---- zero g ----

    /// Frames from lift-off to the end of the longest space act, and the
    /// frames a fall from anywhere on the monitor takes to land.
    fn longest_float() -> usize {
        let sh = &SHEETS[sheet_index("space")];
        sh.launch + (FLOAT_CRUISE.1 * FPS as f32) as usize + (sh.frames.len() - sh.outro) + 2
    }
    const LAND_FRAMES: usize = 24;

    /// The sprite's box on screen for `p`: left, top, right, bottom.
    fn sprite_box(p: Pose) -> (i32, i32, i32, i32) {
        let f = crate::sprites::frame(p.sprite, p.frame);
        let s = 4;
        let left = f.left(p.x, s, p.mirrored);
        (left, p.y - f.h as i32 * s, left + f.w as i32 * s, p.y)
    }

    #[test]
    fn in_zero_g_he_floats_inside_the_work_area_then_lands() {
        let ws = [
            WindowInfo {
                hwnd: 5,
                rect: Rect::new(200, 600, 1200, 900),
            },
            WindowInfo {
                hwnd: 6,
                rect: Rect::new(1300, 400, 1900, 700),
            },
        ];
        let s = build(&ws, &[mon()], 72);
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
            leans: &[],
        };
        let area = mon().work;
        let cap = FLOAT_CAP * 4.0 / FPS as f32 + 0.01;
        let mut turned = 0;
        for seed in 0..40 {
            let mut rng = Rng::new(seed);
            let mut a = walker(&mut rng, &s);
            // Start anywhere along the ground, edges included.
            a.x = 60.0 + (seed as f32 * 457.0) % 1800.0;
            let start = a.y;
            assert!(a.float(&[area], &mut rng));
            assert!(!a.float(&[area], &mut rng), "already floating");
            assert!(!a.can_react(Trigger::User), "no reaction in the air");
            let (mut frames, mut top, mut flips) = (0, start, 0);
            let mut dir = a.dir;
            let mut last_out = i32::MAX;
            while a.is_floating() {
                let (x, y) = (a.x, a.y);
                a.step(&world, &mut rng);
                frames += 1;
                assert!(frames <= longest_float(), "seed {seed}: still floating after {frames}");
                let moved = ((a.x - x).powi(2) + (a.y - y).powi(2)).sqrt();
                if a.is_floating() {
                    assert!(moved <= cap, "seed {seed}: {moved} px in a frame");
                    let (l, t, r, b) = sprite_box(a.pose());
                    let out = (area.left - l)
                        .max(r - area.right)
                        .max(area.top - t)
                        .max(b - area.bottom);
                    // Lifting off right by an edge, the wider suit may start
                    // a little past it; he glides back in during the lead in.
                    assert!(
                        out <= 0 || (frames < SHEETS[sheet_index("space")].launch && out < last_out),
                        "seed {seed}: out of the work area at {:?} after {frames} frames",
                        (l, t, r, b)
                    );
                    assert!(
                        a.y <= area.bottom as f32 && a.y >= area.top as f32,
                        "seed {seed}: feet left the area"
                    );
                    last_out = out;
                    top = top.min(a.y);
                    flips += (a.dir != dir) as u32;
                    dir = a.dir;
                }
            }
            assert!(
                frames >= SHEETS[sheet_index("space")].frames.len(),
                "seed {seed}: {frames}"
            );
            assert!(
                start - top >= 10.0 * 4.0,
                "seed {seed}: barely left the ground ({})",
                start - top
            );
            turned += (flips >= 1) as u32;
            // Gravity is back: he falls onto whatever is below and walks on.
            assert!(a.is_falling(), "{}", a.describe());
            let mut landed = None;
            for k in 0..LAND_FRAMES {
                a.step(&world, &mut rng);
                if !a.is_falling() {
                    landed = Some(k);
                    break;
                }
            }
            assert!(landed.is_some(), "seed {seed}: no landing within {LAND_FRAMES} frames");
            let y = a.y.round() as i32;
            assert!(
                s.iter().any(|s| s.y == y && s.contains_x(a.x as i32)),
                "seed {seed}: landed on nothing at {y}"
            );
            for _ in 0..12 {
                a.step(&world, &mut rng);
            }
            assert!(a.is_walking(), "seed {seed}: {}", a.describe());
        }
        // Now and then he turns the other way while drifting.
        assert!(turned >= 30, "only {turned} of 40 floats turned");
    }

    #[test]
    fn picked_up_in_zero_g_the_float_is_over() {
        let s = build(&[], &[mon()], 72);
        let world = World {
            surfaces: &s,
            idle_secs: 0.0,
            leans: &[],
        };
        let mut rng = Rng::new(31);
        let mut a = walker(&mut rng, &s);
        assert!(a.float(&[mon().work], &mut rng));
        for _ in 0..40 {
            a.step(&world, &mut rng);
        }
        assert!(a.is_floating());
        assert_eq!(a.pose().sprite, SpriteRef::Sheet(sheet_index("space")));
        a.grab();
        assert!(a.is_dragged());
        assert!(!a.float(&[mon().work], &mut rng), "not while held");
        a.drag_to(700, 300);
        a.release();
        assert!(!a.float(&[mon().work], &mut rng), "not while falling");
        for _ in 0..LAND_FRAMES {
            a.step(&world, &mut rng);
        }
        assert_eq!(a.pose().y, 1032);
        // Off every work area: no float.
        let mut b = walker(&mut rng, &s);
        assert!(!b.float(&[Rect::new(3000, 0, 4000, 1000)], &mut rng));
        assert!(b.is_walking());
    }
}
