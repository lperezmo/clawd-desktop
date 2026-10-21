//! Several Clawds at once. Each has his own actor, RNG stream, pace and
//! timers. What they know about each other lives here, in the shared crowd
//! state (each Clawd is a separate window, so nothing goes through window
//! order): two walking into each other on the same surface never pass
//! through, and with antics on they react to each other first.
//!
//! The antics, decided once per meeting by `meeting` and `plan`:
//! - Head on: they stop and face each other for a beat, then one turns (a
//!   chat). Rarely the one already walking longer shoves and the other hops
//!   over him, lands behind and shows a "?!". Sometimes one climbs on the
//!   other and rides him for a moment.
//! - Catching up from behind: he may climb on and ride.
//! - Both carrying laptops (Claude Code locked in), or walking up to one at
//!   his laptop: they sit down back to back or facing each other and work,
//!   as ordinary laptop acts.
//! - Afterwards each feels something (happy after company, annoyed after a
//!   shove) that leans his next act.
//!
//! - Zero g: when one starts the space act, gravity turns off for him and
//!   whoever stands within a body width of him floats along for a moment.
//!   A rider gets down first; nobody stacks in zero g.
//!
//! Nothing that happens because of another Clawd lasts longer than
//! `ENCOUNTER_CAP` frames, apart from the laptop acts, which are ordinary
//! acts with their ordinary length.

use crate::actor::{Actor, Feel, HALF_BODY, HEAD, Landing, PASSENGER_FRAMES, PERCH_FRAMES, Rng, STARTLE_FRAMES, World};
use crate::sprites::{FPS, SHEETS, act_index, sheet_index};
use crate::surfaces::{Rect, Surface};

pub const MAX_COUNT: usize = 5;

/// Anything one Clawd does because of another is over within this many
/// frames (about three seconds), apart from working side by side.
#[cfg(test)]
pub const ENCOUNTER_CAP: usize = 3 * FPS as usize;

// Distances are sprite pixels between two Clawds' centre lines.
/// Head on: they stop here, a hair apart (the walk is 24 wide).
pub const MEET_GAP: f32 = 25.0;
/// Head on with laptops, or walking up to the front of one at his laptop:
/// they sit down facing each other from here, so the laptops (22 pixels in
/// front of each) do not overlap.
pub const FACE_GAP: f32 = 46.0;
/// One right behind another, going the same way: he may climb on.
pub const CLIMB_GAP: f32 = 16.0;
/// A shoved Clawd lands this far behind the one who shoved him.
pub const SHOVE_LAND: f32 = 26.0;
/// A rider gets down this far in front of the one he rode.
pub const DISMOUNT_GAP: f32 = 26.0;

/// Leaps, in frames and sprite pixels of height. A shove clears the other
/// Clawd's head at the top of the hop.
pub const SHOVE_HOP: (usize, f32) = (10, 20.0);
pub const CLIMB_HOP: (usize, f32) = (6, 6.0);
pub const DISMOUNT_HOP: (usize, f32) = (6, 4.0);

/// The beat two Clawds face each other before one turns, in frames.
pub const CHAT_FRAMES: (usize, usize) = (12, 18);
/// The one who does not turn waits this much longer, so they part.
pub const PART_FRAMES: usize = 6;

/// Head on, how often one shoves the other; kept rare so it stays funny.
pub const SHOVE_CHANCE: f32 = 0.1;
/// And no second shove anywhere in the crowd for this long, in seconds.
pub const SHOVE_REST: f32 = 60.0;
/// Head on, how often one climbs on the other instead of a chat.
pub const CLIMB_CHANCE: f32 = 0.2;
/// Catching up from behind, how often he climbs on.
pub const CATCH_UP_CLIMB_CHANCE: f32 = 0.3;
/// Walking up to one at his laptop, how often he sits down with his own
/// when Claude Code is not locked in (always when it is).
pub const JOIN_CHANCE: f32 = 0.5;
/// Only while the other has this many frames of his laptop act left, so
/// they work side by side rather than one just packing up.
pub const JOIN_MIN_FRAMES: usize = 2 * FPS as usize;
/// After two Clawds meet, they leave each other be for this long (seconds),
/// apart from never walking through each other head on.
pub const PAIR_REST: f32 = 6.0;
/// And each of them leaves everyone be for this long (seconds), so one
/// meeting never runs straight into the next.
pub const CALM: f32 = 2.0;

pub struct Member {
    pub actor: Actor,
    pub rng: Rng,
}

/// A Clawd riding on another's head. Never more than two high: neither may
/// be part of another stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stack {
    pub top: usize,
    pub bottom: usize,
}

pub struct Crowd {
    pub members: Vec<Member>,
    scale: i32,
    /// Off: only the plain rule (the lower index turns around).
    pub antics: bool,
    /// Claude Code is locked in: every Clawd carries a laptop.
    pub lockin: bool,
    rng: Rng,
    stacks: Vec<Stack>,
    /// Pairs leaving each other be, with the frames left.
    rests: Vec<(usize, usize, u32)>,
    /// Frames until the next shove may happen.
    shove_rest: u32,
    /// Per Clawd, frames until he may meet anyone again.
    calm: Vec<u32>,
    /// Whether the next laptop pair met head on sits facing each other
    /// (else back to back).
    facing_next: bool,
}

/// What one Clawd is doing, as far as meeting another goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Doing {
    Walking,
    /// In a laptop or desktop act, with a while of it left.
    Working,
    Other,
}

/// One Clawd as another sees him.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    /// Centre line, screen pixels.
    pub x: f32,
    /// Feet row, screen pixels.
    pub y: i32,
    pub dir: f32,
    pub doing: Doing,
}

/// How two Clawds on the same surface come together. `0` and `1` name the
/// first and second of the pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meeting {
    /// Walking toward each other.
    HeadOn,
    /// Walking the same way, `climber` right behind the other.
    CatchUp { climber: usize },
    /// `walker` walking up to the other at his laptop: from behind his back,
    /// or from in front of his screen.
    AtDesk { walker: usize, behind: bool },
}

/// What happens when they meet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plan {
    /// Not close enough yet.
    Wait,
    /// Leave each other be (for `PAIR_REST`).
    Pass,
    /// The plain rule: the first of the pair turns around.
    Turn,
    /// Face each other for `frames`, then `turner` turns and both walk on.
    Chat { turner: usize, frames: usize },
    /// `pusher` walks on; the other hops over him, lands behind, "?!".
    Shove { pusher: usize },
    /// `climber` climbs on the other's head and rides him.
    Climb { climber: usize },
    /// Both sit down with laptops, facing each other or back to back.
    Laptops { facing: bool },
    /// `walker` sits down with his laptop by the one already working.
    Join { walker: usize, facing: bool },
}

/// The odds that are not in the pair itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Odds {
    pub lockin: bool,
    pub shove_ready: bool,
    pub facing_next: bool,
}

/// Two Clawds that met, and what came of it, for the log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Encounter {
    pub a: usize,
    pub b: usize,
    pub plan: Plan,
}

impl std::fmt::Display for Encounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (a, b) = (self.a, self.b);
        let who = |k: usize| if k == 0 { a } else { b };
        let other = |k: usize| if k == 0 { b } else { a };
        match self.plan {
            Plan::Chat { turner, .. } => write!(f, "Clawds {a} and {b} stop for a chat; {} turns", who(turner)),
            Plan::Shove { pusher } => write!(f, "Clawd {} shoves Clawd {}", who(pusher), other(pusher)),
            Plan::Climb { climber } => write!(f, "Clawd {} climbs on Clawd {}", who(climber), other(climber)),
            Plan::Laptops { facing } => write!(
                f,
                "Clawds {a} and {b} work {}",
                if facing { "facing each other" } else { "back to back" }
            ),
            Plan::Join { walker, facing } => write!(
                f,
                "Clawd {} sits down to work {} Clawd {}",
                who(walker),
                if facing { "facing" } else { "back to back with" },
                other(walker)
            ),
            p => write!(f, "Clawds {a} and {b}: {p:?}"),
        }
    }
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
/// The plain rule, used when antics are off or the pair is resting.
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

/// How `a` and `b` come together, and the sprite pixels between their
/// centre lines; None when they do not (different surfaces, apart, walking
/// away from each other, busy with something else).
pub fn meeting(a: &Seen, b: &Seen, scale: i32) -> Option<(Meeting, f32)> {
    if a.y != b.y {
        return None;
    }
    let dx = b.x - a.x;
    let gap = dx.abs() / scale.max(1) as f32;
    if gap >= FACE_GAP || dx == 0.0 {
        return None;
    }
    // Whether `s`, at `from`, faces toward `to`.
    let faces = |s: &Seen, to: f32| (to - s.x) * s.dir > 0.0;
    match (a.doing, b.doing) {
        (Doing::Walking, Doing::Walking) => {
            if faces(a, b.x) && faces(b, a.x) {
                Some((Meeting::HeadOn, gap))
            } else if a.dir == b.dir && gap < CLIMB_GAP {
                let climber = if faces(a, b.x) { 0 } else { 1 };
                Some((Meeting::CatchUp { climber }, gap))
            } else {
                None
            }
        }
        (Doing::Walking, Doing::Working) if faces(a, b.x) => Some((
            Meeting::AtDesk {
                walker: 0,
                behind: !faces(b, a.x),
            },
            gap,
        )),
        (Doing::Working, Doing::Walking) if faces(b, a.x) => Some((
            Meeting::AtDesk {
                walker: 1,
                behind: !faces(a, b.x),
            },
            gap,
        )),
        _ => None,
    }
}

/// What comes of a meeting at `gap` sprite pixels. `walked` is how long
/// each has been walking: the one walking longer is the one who shoves.
pub fn plan(m: Meeting, gap: f32, odds: &Odds, walked: [f32; 2], rng: &mut Rng) -> Plan {
    match m {
        Meeting::HeadOn if odds.lockin => {
            if odds.facing_next && gap < FACE_GAP {
                Plan::Laptops { facing: true }
            } else if !odds.facing_next && gap < MEET_GAP {
                Plan::Laptops { facing: false }
            } else {
                Plan::Wait
            }
        }
        Meeting::HeadOn if gap >= MEET_GAP => Plan::Wait,
        Meeting::HeadOn => {
            let r = rng.f32();
            if r < SHOVE_CHANCE {
                if odds.shove_ready {
                    Plan::Shove {
                        pusher: if walked[1] > walked[0] { 1 } else { 0 },
                    }
                } else {
                    chat(rng)
                }
            } else if r < SHOVE_CHANCE + CLIMB_CHANCE {
                Plan::Climb {
                    climber: (rng.next_u64() % 2) as usize,
                }
            } else {
                chat(rng)
            }
        }
        Meeting::CatchUp { climber } => {
            if rng.f32() < CATCH_UP_CLIMB_CHANCE {
                Plan::Climb { climber }
            } else {
                Plan::Pass
            }
        }
        Meeting::AtDesk { walker, behind } => {
            let close = if behind { MEET_GAP } else { FACE_GAP };
            if gap >= close {
                Plan::Wait
            } else if odds.lockin || rng.f32() < JOIN_CHANCE {
                Plan::Join {
                    walker,
                    facing: !behind,
                }
            } else {
                Plan::Pass
            }
        }
    }
}

fn chat(rng: &mut Rng) -> Plan {
    let span = (CHAT_FRAMES.1 - CHAT_FRAMES.0 + 1) as u64;
    Plan::Chat {
        turner: 0,
        frames: CHAT_FRAMES.0 + (rng.next_u64() % span) as usize,
    }
}

/// Frames the longest part of `plan` keeps a Clawd busy because of the
/// other, at most; laptop acts are ordinary acts and count as none.
pub fn plan_frames(plan: Plan) -> usize {
    match plan {
        Plan::Chat { frames, .. } => frames + PART_FRAMES,
        Plan::Shove { .. } => SHOVE_HOP.0 + STARTLE_FRAMES,
        // Climb, ride (one frame of slack for the crowd to get him down), get down.
        Plan::Climb { .. } => CLIMB_HOP.0 + PERCH_FRAMES + 1 + DISMOUNT_HOP.0,
        _ => 0,
    }
}

/// Both of a pair, mutably, in the order asked.
fn two<T>(v: &mut [T], i: usize, j: usize) -> (&mut T, &mut T) {
    assert_ne!(i, j);
    if i < j {
        let (a, b) = v.split_at_mut(j);
        (&mut a[i], &mut b[0])
    } else {
        let (a, b) = v.split_at_mut(i);
        (&mut b[0], &mut a[j])
    }
}

impl Crowd {
    pub fn new(scale: i32) -> Crowd {
        Crowd {
            members: Vec::new(),
            scale,
            antics: true,
            lockin: false,
            rng: Rng::new(0x00c1_a3d0),
            stacks: Vec::new(),
            rests: Vec::new(),
            shove_rest: 0,
            calm: Vec::new(),
            facing_next: false,
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

    /// Who is riding on whom.
    #[cfg(test)]
    pub fn stacks(&self) -> &[Stack] {
        &self.stacks
    }

    /// Grows or shrinks to `n` (1 to `MAX_COUNT`). Survivors are untouched;
    /// newcomers each get their own RNG stream seeded from `seeds` and drop
    /// in a little above a spread-out spot. `fallback` is where to drop in
    /// when there are no surfaces yet.
    pub fn resize(&mut self, n: usize, surfaces: &[Surface], seeds: &mut Rng, fallback: (i32, i32)) {
        let n = n.clamp(1, MAX_COUNT);
        self.members.truncate(n);
        self.stacks.retain(|s| s.top < n && s.bottom < n);
        self.rests.retain(|&(a, b, _)| a < n && b < n);
        self.calm.resize(n, 0);
        if self.members.len() == n {
            return;
        }
        let have = self.members.len();
        if have == 0 {
            self.rng = Rng::new(seeds.next_u64());
        }
        let starts = spread_starts(surfaces, n, self.scale, seeds);
        let drop = SHEETS[sheet_index("walk")].h as i32 * self.scale;
        for k in have..n {
            let mut rng = Rng::new(seeds.next_u64());
            let (x, y) = starts.get(k).map_or(fallback, |&(x, y)| (x, y - drop));
            let actor = Actor::new(x, y, self.scale, &mut rng);
            self.members.push(Member { actor, rng });
        }
        self.calm.resize(n, 0);
    }

    /// Riding, carrying, still calming down after a meeting, or busy with
    /// another Clawd: only the plain rule applies to him.
    fn busy(&self, i: usize) -> bool {
        let a = &self.members[i].actor;
        self.in_stack(i) || self.calm[i] > 0 || a.is_social() || a.is_floating() || a.is_climbing()
    }

    fn in_stack(&self, i: usize) -> bool {
        self.stacks.iter().any(|s| s.top == i || s.bottom == i)
    }

    fn resting(&self, i: usize, j: usize) -> bool {
        self.rests.iter().any(|&(a, b, _)| (a, b) == (i, j))
    }

    fn seen(&self, i: usize) -> Seen {
        let a = &self.members[i].actor;
        let doing = if a.is_walking() {
            Doing::Walking
        } else if matches!(a.act_name(), Some("laptop" | "desktop"))
            && a.act_frames_left().is_some_and(|n| n >= JOIN_MIN_FRAMES)
        {
            Doing::Working
        } else {
            Doing::Other
        };
        Seen {
            x: a.x,
            y: a.y.round() as i32,
            dir: a.dir,
            doing,
        }
    }

    /// Advances everyone one frame. Returns the meetings that started
    /// something, for the log.
    pub fn step(&mut self, world: &World) -> Vec<Encounter> {
        for m in &mut self.members {
            m.actor.step(world, &mut m.rng);
        }
        self.shove_rest = self.shove_rest.saturating_sub(1);
        for c in &mut self.calm {
            *c = c.saturating_sub(1);
        }
        self.rests.retain_mut(|r| {
            r.2 = r.2.saturating_sub(1);
            r.2 > 0
        });
        if !self.antics {
            let mut actors: Vec<&mut Actor> = self.members.iter_mut().map(|m| &mut m.actor).collect();
            resolve_meetings(&mut actors);
            self.tend_stacks();
            return Vec::new();
        }
        let mut out = Vec::new();
        let reach = SHEETS[sheet_index("walk")].w as f32;
        for i in 0..self.members.len() {
            for j in i + 1..self.members.len() {
                let (a, b) = (self.seen(i), self.seen(j));
                let Some((m, gap)) = meeting(&a, &b, self.scale) else {
                    continue;
                };
                let busy = self.resting(i, j) || self.busy(i) || self.busy(j);
                let p = if busy {
                    if m == Meeting::HeadOn && gap < reach {
                        Plan::Turn
                    } else {
                        Plan::Wait
                    }
                } else {
                    let odds = Odds {
                        lockin: self.lockin,
                        shove_ready: self.shove_rest == 0,
                        facing_next: self.facing_next,
                    };
                    let walked = [self.members[i].actor.walking_for(), self.members[j].actor.walking_for()];
                    plan(m, gap, &odds, walked, &mut self.rng)
                };
                if self.apply(i, j, p) {
                    out.push(Encounter { a: i, b: j, plan: p });
                }
            }
        }
        // Last, so a rider sits on his carrier where the carrier ends up.
        self.tend_stacks();
        out
    }

    /// Carries out a plan for the pair `i`, `j`. True when it started an
    /// antic worth logging.
    pub fn apply(&mut self, i: usize, j: usize, p: Plan) -> bool {
        let scale = self.scale as f32;
        let ij = [i, j];
        match p {
            Plan::Wait => return false,
            Plan::Turn => {
                self.members[i].actor.turn_around();
                return false;
            }
            Plan::Pass => {}
            Plan::Chat { turner, frames } => {
                let (t, o) = two(&mut self.members, ij[turner], ij[1 - turner]);
                t.actor.stand(frames, true);
                o.actor.stand(frames + PART_FRAMES, false);
                t.actor.set_feel(Feel::Happy);
                o.actor.set_feel(Feel::Happy);
            }
            Plan::Shove { pusher } => {
                let (p, o) = two(&mut self.members, ij[pusher], ij[1 - pusher]);
                let to = p.actor.x - p.actor.dir * SHOVE_LAND * scale;
                o.actor.leap(to, 0.0, SHOVE_HOP.1, SHOVE_HOP.0, Landing::Startled);
                o.actor.set_feel(Feel::Annoyed);
                self.shove_rest = (SHOVE_REST * FPS as f32) as u32;
            }
            Plan::Climb { climber } => {
                let (c, b) = two(&mut self.members, ij[climber], ij[1 - climber]);
                c.actor.dir = b.actor.dir;
                c.actor.leap(b.actor.x, HEAD, CLIMB_HOP.1, CLIMB_HOP.0, Landing::Perch);
                c.actor.set_feel(Feel::Happy);
                // He keeps walking while carrying.
                b.actor.postpone_act(4.0);
                self.stacks.push(Stack {
                    top: ij[climber],
                    bottom: ij[1 - climber],
                });
            }
            Plan::Laptops { facing } => {
                let laptop = act_index("laptop");
                for k in ij {
                    let a = &mut self.members[k].actor;
                    if !facing {
                        a.turn_around();
                    }
                    a.sit_down(laptop);
                    a.set_feel(Feel::Happy);
                }
                self.facing_next = self.rng.f32() < 0.5;
            }
            Plan::Join { walker, facing } => {
                let (w, o) = two(&mut self.members, ij[walker], ij[1 - walker]);
                if !facing {
                    w.actor.turn_around();
                }
                w.actor.sit_down(act_index("laptop"));
                w.actor.set_feel(Feel::Happy);
                o.actor.set_feel(Feel::Happy);
            }
        }
        self.rests.retain(|&(a, b, _)| (a, b) != (i, j));
        self.rests.push((i, j, (PAIR_REST * FPS as f32) as u32));
        if p != Plan::Pass {
            let calm = ((plan_frames(p) as f32 / FPS as f32 + CALM) * FPS as f32) as u32;
            self.calm[i] = calm;
            self.calm[j] = calm;
        }
        p != Plan::Pass
    }

    /// Who wants the space act now that an act came due; each wish is
    /// cleared. The app grants them with `float`, which needs the work areas.
    pub fn space_wishes(&mut self) -> Vec<usize> {
        (0..self.members.len())
            .filter(|&i| self.members[i].actor.take_space_wish())
            .collect()
    }

    /// Clawd `i` plays the space act with gravity off, inside his work area
    /// (one of `areas`). A rider on his head gets down first, he floats off
    /// the head he is riding on, and with antics on anyone standing within a
    /// body width of him floats along for a moment (well under three
    /// seconds) and drops. False when he cannot float now.
    pub fn float(&mut self, i: usize, areas: &[Rect]) -> bool {
        let Some(m) = self.members.get_mut(i) else { return false };
        let (x, y) = (m.actor.x, m.actor.y.round());
        if !m.actor.float(areas, &mut m.rng) {
            return false;
        }
        // No stacks in zero g: his rider hops down in front, and the one he
        // was riding walks on.
        self.tend_stacks();
        if !self.antics {
            return true;
        }
        let reach = SHEETS[sheet_index("walk")].w as f32 * self.scale as f32;
        for j in 0..self.members.len() {
            if j == i || self.busy(j) {
                continue;
            }
            let a = &self.members[j].actor;
            let afoot = a.is_walking() || a.act_name().is_some();
            if !afoot || a.y.round() != y || (a.x - x).abs() >= reach {
                continue;
            }
            let m = &mut self.members[j];
            if m.actor.float_along(areas, &mut m.rng) {
                self.calm[j] = ((PASSENGER_FRAMES as f32 / FPS as f32 + CALM) * FPS as f32) as u32;
            }
        }
        true
    }

    /// Keeps each rider on his carrier's head while the carrier walks, and
    /// gets him down in front when his ride is up or the carrier stops.
    fn tend_stacks(&mut self) {
        let scale = self.scale as f32;
        let mut stacks = std::mem::take(&mut self.stacks);
        stacks.retain(|st| {
            let (top, bottom) = two(&mut self.members, st.top, st.bottom);
            let (top, bottom) = (&mut top.actor, &mut bottom.actor);
            if top.leap_landing() == Some(Landing::Perch) {
                top.retarget(bottom.x);
                return true;
            }
            let Some(left) = top.perch_left() else {
                // Picked up, fallen, or already getting down.
                return false;
            };
            if bottom.is_walking() && left > 0 {
                top.x = bottom.x;
                top.y = bottom.y;
                top.dir = bottom.dir;
                return true;
            }
            let ahead = if bottom.is_walking() { bottom.dir } else { top.dir };
            top.leap(
                bottom.x + ahead * DISMOUNT_GAP * scale,
                0.0,
                DISMOUNT_HOP.1,
                DISMOUNT_HOP.0,
                Landing::Walk,
            );
            false
        });
        self.stacks = stacks;
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
            leans: &[],
            areas: &[],
            roam: false,
            climb: false,
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
        c.antics = false;
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
        c.antics = false;
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

    // ---- antics ----

    const S: i32 = 4;

    fn ground() -> Vec<Surface> {
        build(&[], &[mon()], 72)
    }

    /// `n` Clawds walking on the ground at (x, dir), acts put off.
    fn walkers(s: &[Surface], at: &[(f32, f32)]) -> Crowd {
        let mut c = Crowd::new(S);
        c.antics = false;
        c.resize(at.len(), s, &mut Rng::new(31), (0, 0));
        for _ in 0..40 {
            c.step(&world(s));
        }
        for (m, &(x, dir)) in c.members.iter_mut().zip(at) {
            assert!(m.actor.is_walking(), "{}", m.actor.describe());
            m.actor.x = x;
            m.actor.dir = dir;
            m.actor.y = 1032.0;
            m.actor.postpone_act(1000.0);
        }
        c.antics = true;
        c
    }

    fn seen(x: f32, dir: f32, doing: Doing) -> Seen {
        Seen { x, y: 1032, dir, doing }
    }

    #[test]
    fn meetings_are_classified() {
        use Doing::*;
        let w = |x, d| seen(x, d, Walking);
        // Head on, 22.5 sprite pixels apart.
        assert_eq!(
            meeting(&w(1000.0, 1.0), &w(1090.0, -1.0), S),
            Some((Meeting::HeadOn, 22.5))
        );
        // Walking away from each other, or far apart: nothing.
        assert_eq!(meeting(&w(1000.0, -1.0), &w(1090.0, 1.0), S), None);
        assert_eq!(meeting(&w(1000.0, 1.0), &w(1200.0, -1.0), S), None);
        // Another surface: nothing.
        let mut up = w(1090.0, -1.0);
        up.y = 400;
        assert_eq!(meeting(&w(1000.0, 1.0), &up, S), None);
        // Same way: only right behind is a catch up, and the one behind climbs.
        assert_eq!(meeting(&w(1000.0, 1.0), &w(1090.0, 1.0), S), None);
        assert_eq!(
            meeting(&w(1000.0, 1.0), &w(1050.0, 1.0), S),
            Some((Meeting::CatchUp { climber: 0 }, 12.5))
        );
        assert_eq!(
            meeting(&w(1050.0, -1.0), &w(1000.0, -1.0), S),
            Some((Meeting::CatchUp { climber: 0 }, 12.5))
        );
        assert_eq!(
            meeting(&w(1050.0, 1.0), &w(1000.0, 1.0), S),
            Some((Meeting::CatchUp { climber: 1 }, 12.5))
        );
        // Walking up to one at his laptop: behind his back or at his screen.
        let at_desk = |dir| seen(1090.0, dir, Working);
        assert_eq!(
            meeting(&w(1000.0, 1.0), &at_desk(1.0), S),
            Some((
                Meeting::AtDesk {
                    walker: 0,
                    behind: true
                },
                22.5
            ))
        );
        assert_eq!(
            meeting(&at_desk(-1.0), &w(1000.0, 1.0), S),
            Some((
                Meeting::AtDesk {
                    walker: 1,
                    behind: false
                },
                22.5
            ))
        );
        assert_eq!(meeting(&w(1000.0, -1.0), &at_desk(1.0), S), None);
        // Busy with anything else: nothing.
        assert_eq!(meeting(&w(1000.0, 1.0), &seen(1090.0, -1.0, Other), S), None);
    }

    #[test]
    fn plans_follow_the_odds() {
        let odds = Odds {
            lockin: false,
            shove_ready: true,
            facing_next: false,
        };
        let mut rng = Rng::new(5);
        let n = 20_000;
        let (mut shoves, mut climbs, mut chats) = (0, 0, 0);
        for _ in 0..n {
            match plan(Meeting::HeadOn, 24.0, &odds, [5.0, 1.0], &mut rng) {
                Plan::Shove { pusher } => {
                    assert_eq!(pusher, 0, "the one walking longer shoves");
                    shoves += 1;
                }
                Plan::Climb { .. } => climbs += 1,
                Plan::Chat { turner, frames } => {
                    assert_eq!(turner, 0);
                    assert!((CHAT_FRAMES.0..=CHAT_FRAMES.1).contains(&frames));
                    // A beat: one to one and a half seconds.
                    assert!((12..=18).contains(&frames));
                    chats += 1;
                }
                p => panic!("{p:?}"),
            }
        }
        let share = |k: i32| k as f32 / n as f32;
        assert!((share(shoves) - SHOVE_CHANCE).abs() < 0.01, "{shoves}");
        assert!(share(shoves) <= 0.12, "shoves stay rare");
        assert!((share(climbs) - CLIMB_CHANCE).abs() < 0.015, "{climbs}");
        assert!(chats > shoves + climbs);
        assert_eq!(
            plan(Meeting::HeadOn, 24.0, &odds, [1.0, 5.0], &mut Rng::new(0)),
            plan(Meeting::HeadOn, 24.0, &odds, [1.0, 5.0], &mut Rng::new(0))
        );
        let mut r = Rng::new(9);
        let pushers: Vec<usize> = (0..2000)
            .filter_map(|_| match plan(Meeting::HeadOn, 24.0, &odds, [1.0, 5.0], &mut r) {
                Plan::Shove { pusher } => Some(pusher),
                _ => None,
            })
            .collect();
        assert!(!pushers.is_empty() && pushers.iter().all(|&p| p == 1));
        // A shove anywhere in the crowd recently: none.
        let rested = Odds {
            shove_ready: false,
            ..odds
        };
        assert!((0..5000).all(|_| !matches!(
            plan(Meeting::HeadOn, 24.0, &rested, [5.0, 1.0], &mut rng),
            Plan::Shove { .. }
        )));
        // Not close enough yet.
        assert_eq!(plan(Meeting::HeadOn, 30.0, &odds, [0.0; 2], &mut rng), Plan::Wait);

        // Laptops when Claude Code is locked in: back to back up close, or
        // facing each other from further off.
        let lock = Odds { lockin: true, ..odds };
        assert_eq!(plan(Meeting::HeadOn, 30.0, &lock, [0.0; 2], &mut rng), Plan::Wait);
        assert_eq!(
            plan(Meeting::HeadOn, 24.0, &lock, [0.0; 2], &mut rng),
            Plan::Laptops { facing: false }
        );
        let face = Odds {
            facing_next: true,
            ..lock
        };
        assert_eq!(
            plan(Meeting::HeadOn, 45.0, &face, [0.0; 2], &mut rng),
            Plan::Laptops { facing: true }
        );

        // Catching up: some of the time he climbs on.
        let climbs = (0..n)
            .filter(|_| {
                matches!(
                    plan(Meeting::CatchUp { climber: 1 }, 10.0, &odds, [0.0; 2], &mut rng),
                    Plan::Climb { climber: 1 }
                )
            })
            .count();
        assert!((climbs as f32 / n as f32 - CATCH_UP_CLIMB_CHANCE).abs() < 0.015);

        // At a desk: from behind up close, from the front further off.
        let behind = Meeting::AtDesk {
            walker: 0,
            behind: true,
        };
        assert_eq!(plan(behind, 30.0, &lock, [0.0; 2], &mut rng), Plan::Wait);
        assert_eq!(
            plan(behind, 24.0, &lock, [0.0; 2], &mut rng),
            Plan::Join {
                walker: 0,
                facing: false
            }
        );
        let front = Meeting::AtDesk {
            walker: 1,
            behind: false,
        };
        assert_eq!(
            plan(front, 45.0, &lock, [0.0; 2], &mut rng),
            Plan::Join {
                walker: 1,
                facing: true
            }
        );
        let joins = (0..n)
            .filter(|_| matches!(plan(behind, 24.0, &odds, [0.0; 2], &mut rng), Plan::Join { .. }))
            .count();
        assert!((joins as f32 / n as f32 - JOIN_CHANCE).abs() < 0.015);
    }

    #[test]
    fn no_plan_keeps_a_clawd_longer_than_three_seconds() {
        assert_eq!(ENCOUNTER_CAP, 36);
        for p in [
            Plan::Chat {
                turner: 0,
                frames: CHAT_FRAMES.1,
            },
            Plan::Shove { pusher: 0 },
            Plan::Climb { climber: 0 },
        ] {
            assert!(plan_frames(p) <= ENCOUNTER_CAP, "{p:?}: {}", plan_frames(p));
        }
    }

    #[test]
    fn a_chat_is_a_beat_facing_each_other_then_one_turns() {
        let s = ground();
        let mut c = walkers(&s, &[(1000.0, 1.0), (1096.0, -1.0)]);
        assert!(c.apply(0, 1, Plan::Chat { turner: 0, frames: 15 }));
        let mut beat = 0;
        loop {
            let (a, b) = (&c.members[0].actor, &c.members[1].actor);
            if a.describe() != "stand" {
                break;
            }
            assert_eq!((a.dir, b.dir), (1.0, -1.0), "facing each other");
            assert_eq!((a.x, b.x), (1000.0, 1096.0), "standing still");
            beat += 1;
            c.step(&world(&s));
        }
        assert_eq!(beat, 15, "a beat of a second and a quarter");
        let (a, b) = (&c.members[0].actor, &c.members[1].actor);
        assert!(a.is_walking() && a.dir == -1.0, "Clawd 0 turned and walks on");
        assert_eq!(b.describe(), "stand", "the other waits a moment so they part");
        for _ in 0..PART_FRAMES {
            c.step(&world(&s));
        }
        assert!(c.members[1].actor.is_walking());
        assert_eq!(c.members[1].actor.dir, -1.0);
        assert_eq!(c.members[0].actor.feel(), Some(Feel::Happy));
        assert_eq!(c.members[1].actor.feel(), Some(Feel::Happy));
    }

    #[test]
    fn walking_head_on_they_stop_before_touching_and_never_pass_through() {
        let s = ground();
        let mut kinds = std::collections::HashSet::new();
        for seed in 0..60 {
            let mut c = walkers(&s, &[(800.0, 1.0), (1100.0, -1.0)]);
            c.rng = Rng::new(seed);
            let mut met = None;
            for _ in 0..12 * 15 {
                let (a, b) = (&c.members[0].actor, &c.members[1].actor);
                if a.is_walking() && b.is_walking() && a.dir > 0.0 && b.dir < 0.0 {
                    assert!(b.x > a.x, "walked through each other");
                }
                let gap = b.x - a.x;
                if let Some(e) = c.step(&world(&s)).first() {
                    assert!(gap < MEET_GAP * S as f32 + 8.0, "{gap}");
                    met = Some(e.plan);
                    break;
                }
            }
            let p = met.expect("they never met");
            kinds.insert(std::mem::discriminant(&p));
            assert!(
                matches!(p, Plan::Chat { .. } | Plan::Shove { .. } | Plan::Climb { .. }),
                "{p:?}"
            );
        }
        assert!(kinds.len() >= 2, "always the same thing: {kinds:?}");
    }

    #[test]
    fn a_shove_hops_him_over_the_pusher_with_a_startle() {
        let s = ground();
        let mut c = walkers(&s, &[(1000.0, 1.0), (1096.0, -1.0)]);
        assert!(c.apply(0, 1, Plan::Shove { pusher: 0 }));
        assert_eq!(c.shove_rest, (SHOVE_REST * FPS as f32) as u32);
        assert_eq!(c.members[1].actor.feel(), Some(Feel::Annoyed));
        let (mut busy, mut top, mut marked, mut shook) = (0, 0, 0, false);
        let mut last_x = c.members[0].actor.x;
        while c.members[1].actor.is_social() {
            c.step(&world(&s));
            busy += 1;
            let (p, o) = (&c.members[0].actor, &c.members[1].actor);
            assert!(p.is_walking() && p.x >= last_x, "the pusher just walks on");
            last_x = p.x;
            let pose = o.pose();
            top = top.max(1032 - pose.y);
            if o.is_startled() {
                marked += 1;
                assert!(pose.mark);
                shook |= pose.x != o.x.round() as i32;
                assert!(o.x < p.x, "landed behind the pusher");
            }
            assert!(busy <= ENCOUNTER_CAP);
        }
        assert!(top > (HEAD * S as f32) as i32, "cleared his head: {top}");
        assert_eq!(marked, STARTLE_FRAMES);
        assert!(shook);
        let o = &c.members[1].actor;
        assert!(o.is_walking() && o.dir == -1.0, "carries on the way he was going");
        assert!(!o.pose().mark);
        // No second shove for a while: head on again straight away, they chat
        // or climb, never shove.
        let odds = Odds {
            lockin: false,
            shove_ready: c.shove_rest == 0,
            facing_next: false,
        };
        assert!(!odds.shove_ready);
    }

    #[test]
    fn a_stack_is_two_high_and_rides_along() {
        let s = ground();
        let mut c = walkers(&s, &[(1000.0, 1.0), (960.0, 1.0), (900.0, 1.0)]);
        assert!(c.apply(0, 1, Plan::Climb { climber: 1 }));
        assert_eq!(c.stacks(), &[Stack { top: 1, bottom: 0 }]);
        let mut frames = 0;
        let mut rode = 0;
        while c.members[1].actor.is_social() {
            // The third walks right behind the stack, then into it head on:
            // nobody climbs on either of them.
            let carrier_x = c.members[0].actor.x;
            let third = &mut c.members[2].actor;
            if frames < 10 {
                third.x = carrier_x - 40.0;
                third.dir = 1.0;
            } else {
                third.x = carrier_x + 90.0;
                third.dir = -1.0;
            }
            c.step(&world(&s));
            frames += 1;
            assert!(c.stacks().len() <= 1, "{:?}", c.stacks());
            assert!(c.members[2].actor.perch_left().is_none());
            assert!(c.members[0].actor.perch_left().is_none());
            let (b, t) = (&c.members[0].actor, &c.members[1].actor);
            if t.perch_left().is_some() {
                rode += 1;
                assert_eq!(t.pose().y, b.pose().y - (HEAD * S as f32) as i32, "on his head");
                assert_eq!((t.x, t.dir), (b.x, b.dir), "rides along");
            }
            assert!(frames <= ENCOUNTER_CAP, "still up after {frames} frames");
        }
        assert!(rode >= PERCH_FRAMES);
        let (b, t) = (&c.members[0].actor, &c.members[1].actor);
        assert!(t.is_walking());
        assert_eq!(t.pose().y, 1032);
        assert!((t.x - b.x) * b.dir > 0.0, "got down in front");
        assert_eq!(t.feel(), Some(Feel::Happy));
    }

    #[test]
    fn a_rider_gets_down_when_his_carrier_stops() {
        let s = ground();
        let mut c = walkers(&s, &[(1000.0, 1.0), (960.0, 1.0)]);
        c.apply(0, 1, Plan::Climb { climber: 1 });
        for _ in 0..CLIMB_HOP.0 + 2 {
            c.step(&world(&s));
        }
        assert!(c.members[1].actor.perch_left().is_some());
        c.members[0].actor.sit_down(act_index("coffee"));
        c.step(&world(&s));
        assert_eq!(c.members[1].actor.describe(), "leap");
        assert!(c.stacks().is_empty());
        for _ in 0..DISMOUNT_HOP.0 {
            c.step(&world(&s));
        }
        assert!(c.members[1].actor.is_walking());
        assert_eq!(c.members[1].actor.pose().y, 1032);
    }

    #[test]
    fn laptops_side_by_side_are_two_ordinary_laptop_acts() {
        let s = ground();
        let laptop_len = crate::sprites::ACTS[act_index("laptop")].frames.len();
        for facing in [false, true] {
            let mut c = walkers(&s, &[(800.0, 1.0), (1100.0, -1.0)]);
            c.lockin = true;
            c.facing_next = facing;
            let mut met = None;
            for _ in 0..12 * 10 {
                if let Some(e) = c.step(&world(&s)).first() {
                    let gap = (c.members[1].actor.x - c.members[0].actor.x) / S as f32;
                    met = Some((e.plan, gap));
                    break;
                }
            }
            let (p, gap) = met.expect("never met");
            assert_eq!(p, Plan::Laptops { facing });
            let (a, b) = (&c.members[0].actor, &c.members[1].actor);
            assert_eq!(a.describe(), "act laptop");
            assert_eq!(b.describe(), "act laptop");
            if facing {
                assert_eq!((a.dir, b.dir), (1.0, -1.0));
                assert!(gap >= 44.0, "laptops overlap at {gap}");
            } else {
                assert_eq!((a.dir, b.dir), (-1.0, 1.0), "back to back");
                assert!(gap < MEET_GAP);
            }
            // Ordinary act timing: the laptop act plays whole, once.
            let mut frames = 1;
            let mut last = c.members[0].actor.pose().frame;
            loop {
                c.step(&world(&s));
                let a = &c.members[0].actor;
                if a.describe() != "act laptop" || a.pose().frame <= last {
                    break;
                }
                last = a.pose().frame;
                frames += 1;
            }
            assert_eq!(frames, laptop_len);
        }

        // Walking up behind one already at his laptop: he sits down back to back.
        let mut c = walkers(&s, &[(900.0, 1.0), (1000.0, 1.0)]);
        c.lockin = true;
        c.members[1].actor.sit_down(act_index("laptop"));
        let mut met = None;
        for _ in 0..12 * 5 {
            if let Some(e) = c.step(&world(&s)).first() {
                met = Some(e.plan);
                break;
            }
        }
        assert_eq!(
            met,
            Some(Plan::Join {
                walker: 0,
                facing: false
            })
        );
        assert_eq!(c.members[0].actor.describe(), "act laptop");
        assert_eq!(c.members[0].actor.dir, -1.0, "his back to the other's back");
    }

    #[test]
    fn nobody_joins_one_who_is_packing_up() {
        let s = ground();
        let mut c = walkers(&s, &[(900.0, 1.0), (1000.0, 1.0)]);
        c.members[1].actor.sit_down(act_index("laptop"));
        assert_eq!(c.seen(1).doing, Doing::Working);
        while c.members[1]
            .actor
            .act_frames_left()
            .is_some_and(|n| n >= JOIN_MIN_FRAMES)
        {
            c.step(&world(&s));
        }
        assert_eq!(c.members[1].actor.describe(), "act laptop");
        assert_eq!(c.seen(1).doing, Doing::Other);
    }

    #[test]
    fn every_encounter_is_over_within_three_seconds() {
        // A narrow screen so they meet often.
        let narrow = MonitorInfo {
            rect: Rect::new(0, 0, 900, 1080),
            work: Rect::new(0, 0, 900, 1032),
            primary: true,
        };
        let ws = [WindowInfo {
            hwnd: 5,
            rect: Rect::new(100, 600, 800, 900),
        }];
        let s = build(&ws, &[narrow], 72);
        let mut seen_plans = std::collections::BTreeMap::new();
        let mut head_on = 0;
        let mut shoves = 0;
        for seed in 0..16 {
            for lockin in [false, true] {
                let mut c = Crowd::new(S);
                c.lockin = lockin;
                c.resize(MAX_COUNT, &s, &mut Rng::new(seed), (0, 0));
                let mut social = [0usize; MAX_COUNT];
                let mut last_shove: Option<usize> = None;
                for step in 0..12 * 180 {
                    for e in c.step(&world(&s)) {
                        let kind = format!("{:?}", e.plan);
                        let kind = kind.split([' ', '{']).next().unwrap_or("").to_string();
                        *seen_plans.entry(kind).or_insert(0) += 1;
                        if matches!(e.plan, Plan::Chat { .. } | Plan::Shove { .. }) {
                            head_on += 1;
                        }
                        if matches!(e.plan, Plan::Shove { .. }) {
                            shoves += 1;
                            if let Some(t) = last_shove {
                                assert!(step - t >= (SHOVE_REST * FPS as f32) as usize);
                            }
                            last_shove = Some(step);
                        }
                    }
                    for (k, m) in c.members.iter().enumerate() {
                        social[k] = if m.actor.is_social() { social[k] + 1 } else { 0 };
                        assert!(
                            social[k] <= ENCOUNTER_CAP,
                            "seed {seed}: Clawd {k} busy with another for {} frames ({})",
                            social[k],
                            m.actor.describe()
                        );
                    }
                    // Never more than two high, and nobody in two stacks.
                    let mut used = std::collections::BTreeSet::new();
                    for st in c.stacks() {
                        assert!(used.insert(st.top) && used.insert(st.bottom), "{:?}", c.stacks());
                        assert!(c.members[st.bottom].actor.perch_left().is_none());
                        assert!(c.members[st.bottom].actor.leap_landing() != Some(Landing::Perch));
                    }
                }
            }
        }
        for kind in ["Chat", "Climb", "Laptops"] {
            assert!(
                seen_plans.get(kind).is_some_and(|&n| n > 0),
                "no {kind}: {seen_plans:?}"
            );
        }
        assert!(shoves > 0, "{seen_plans:?}");
        assert!(shoves * 5 < head_on, "shoves are rare: {shoves} of {head_on}");
    }

    // ---- zero g ----

    fn area() -> Rect {
        mon().work
    }

    #[test]
    fn nobody_stacks_in_zero_g() {
        let s = ground();
        // The carrier floats: his rider hops down first.
        let mut c = walkers(&s, &[(1000.0, 1.0), (960.0, 1.0)]);
        c.apply(0, 1, Plan::Climb { climber: 1 });
        for _ in 0..CLIMB_HOP.0 + 2 {
            c.step(&world(&s));
        }
        assert!(c.members[1].actor.perch_left().is_some());
        assert!(c.float(0, &[area()]));
        assert!(c.stacks().is_empty());
        assert_eq!(c.members[1].actor.describe(), "leap");
        assert!(!c.members[1].actor.is_floating(), "the rider gets down, not up");
        for _ in 0..ENCOUNTER_CAP {
            c.step(&world(&s));
            assert!(c.stacks().is_empty());
        }
        assert!(c.members[1].actor.is_walking(), "{}", c.members[1].actor.describe());
        assert_eq!(c.members[1].actor.pose().y, 1032);
        assert!(c.members[0].actor.is_floating());

        // The rider floats: off the head he was on.
        let mut c = walkers(&s, &[(1000.0, 1.0), (960.0, 1.0)]);
        c.apply(0, 1, Plan::Climb { climber: 1 });
        for _ in 0..CLIMB_HOP.0 + 2 {
            c.step(&world(&s));
        }
        let head = c.members[1].actor.pose().y;
        assert!(c.float(1, &[area()]));
        assert!(c.stacks().is_empty());
        assert_eq!(c.members[1].actor.pose().y, head, "lifts off from where he was");
        assert!(
            !c.members[0].actor.is_floating(),
            "the carrier is not his neighbour any more"
        );
        while c.members[1].actor.is_floating() {
            c.step(&world(&s));
            assert!(c.stacks().is_empty());
        }
    }

    #[test]
    fn one_beside_him_floats_along_for_under_three_seconds() {
        let s = ground();
        let mut c = walkers(&s, &[(1000.0, 1.0), (1060.0, -1.0), (1400.0, 1.0)]);
        assert!(c.float(0, &[area()]));
        assert_eq!(c.members[1].actor.describe(), "float along");
        assert!(!c.members[2].actor.is_floating(), "too far away");
        let mut frames = 0;
        let mut rose = false;
        while !c.members[1].actor.is_walking() {
            c.step(&world(&s));
            frames += 1;
            rose |= c.members[1].actor.pose().y < 1032 - 4 * S;
            assert!(frames <= ENCOUNTER_CAP, "still up after {frames} frames");
        }
        assert!(rose);
        assert_eq!(c.members[1].actor.pose().y, 1032);
        assert!(c.members[0].actor.is_floating(), "the one in the suit is still up");

        // Antics off: only he floats.
        let mut c = walkers(&s, &[(1000.0, 1.0), (1060.0, -1.0)]);
        c.antics = false;
        assert!(c.float(0, &[area()]));
        assert!(!c.members[1].actor.is_floating());
    }
}
