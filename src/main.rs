//! Clawd walks along the edges of your windows and the taskbar.

#![cfg_attr(not(test), windows_subsystem = "windows")]

mod actor;
mod config;
mod crowd;
mod hit;
mod install;
mod leans;
mod mood;
mod overlay;
mod react;
mod signals;
mod sprites;
#[rustfmt::skip]
mod sprites_data;
mod surfaces;
mod tray;

use actor::{Pose, Rng, Trigger, World};
use config::Config;
use crowd::Crowd;
use log::{debug, info, warn};
use overlay::{Mouse, Overlay};
use sprites::{ACTS, FPS, Rot, act_index, frame, max_size, sheet_index};
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::time::{Duration, Instant};
use surfaces::{MonitorInfo, Surface};
use tray::{Command, Tray, WM_TRAY};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, GetStdHandle, STD_ERROR_HANDLE,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetLastInputInfo, LASTINPUTINFO, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey,
    UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EVENT_OBJECT_CLOAKED,
    EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND,
    FindWindowW, GA_ROOT, GetAncestor, GetSystemMetrics, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
    OBJID_WINDOW, PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT, RegisterClassW, RegisterWindowMessageW,
    SM_CXDRAG, TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_CLOSE, WM_CONTEXTMENU,
    WM_DISPLAYCHANGE, WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONUP, WM_QUIT, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

const STEP: Duration = Duration::from_nanos(1_000_000_000 / FPS as u64);
const POLL: Duration = Duration::from_millis(500);
const MIN_REBUILD: Duration = Duration::from_millis(100);
const STATUS_EVERY: Duration = Duration::from_secs(5);
/// Ctrl+Alt+C: Perform.
const HOTKEY_ID: i32 = 1;
const CONTROL_CLASS: PCWSTR = w!("ClawdDesktopControl");
/// Posted to the running copy's control window by `--perform`.
const PERFORM_MESSAGE: PCWSTR = w!("ClawdDesktopPerform");
/// How often the clock and the battery are read for the leans.
const SIGNALS_EVERY: Duration = Duration::from_secs(60);
/// Rest between foreground-change reactions, over all Clawds.
const FOREGROUND_REST: Duration = Duration::from_secs(8);
/// How long a create or show waits for its other half, and how many wait.
const PAIR_MS: u32 = 5000;
const MAX_RECENT: usize = 512;
/// A foreground change reacts this much later, so the new window that
/// caused it reacts first.
const FOREGROUND_DELAY: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    TrayMenu,
    DisplayChange,
    TaskbarCreated,
    /// Every Clawd plays an act now: tray, hotkey or `--perform`.
    Perform,
    /// A top-level window was created and then shown.
    NewWindow(isize),
    Foreground(isize),
    Quit,
}

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static DIRTY: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static PERFORM_MSG: Cell<u32> = const { Cell::new(0) };
    static RECENT: RefCell<Vec<(isize, Seen, u32)>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Seen {
    Created,
    Shown,
}

/// Notes that `h` was created or shown. True when the other half arrived
/// within `PAIR_MS`: a new top-level window. The two can come in either
/// order (Explorer's create event lands after its show).
fn pair(h: isize, seen: Seen) -> bool {
    let now = unsafe { GetTickCount() };
    RECENT.with(|r| {
        let mut r = r.borrow_mut();
        r.retain(|&(_, _, t)| now.wrapping_sub(t) < PAIR_MS);
        if let Some(i) = r.iter().position(|&(x, s, _)| x == h && s != seen) {
            r.swap_remove(i);
            return true;
        }
        if seen == Seen::Shown && r.iter().any(|&(x, s, _)| x == h && s == seen) {
            return false;
        }
        if r.len() >= MAX_RECENT {
            r.remove(0);
        }
        r.push((h, seen, now));
        false
    })
}

fn push(e: Event) {
    EVENTS.with(|q| q.borrow_mut().push(e));
}

unsafe extern "system" fn control_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let m = lp.0 as u32;
            if m == WM_RBUTTONUP || m == WM_LBUTTONUP || m == WM_CONTEXTMENU {
                push(Event::TrayMenu);
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            push(Event::DisplayChange);
            LRESULT(0)
        }
        WM_CLOSE | WM_ENDSESSION => {
            push(Event::Quit);
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.get() => {
            push(Event::TaskbarCreated);
            LRESULT(0)
        }
        m if m != 0 && m == PERFORM_MSG.get() => {
            push(Event::Perform);
            LRESULT(0)
        }
        WM_HOTKEY if wp.0 == HOTKEY_ID as usize => {
            push(Event::Perform);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

unsafe extern "system" fn win_event(
    _: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _: u32,
    _: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 || hwnd.0.is_null() {
        return;
    }
    let top = || unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd;
    match event {
        EVENT_OBJECT_LOCATIONCHANGE if !top() => return,
        EVENT_OBJECT_CREATE => {
            if top() && pair(hwnd.0 as isize, Seen::Created) {
                push(Event::NewWindow(hwnd.0 as isize));
            }
            return;
        }
        // Shown, or uncloaked: Explorer and other DWM-cloaked windows show
        // up cloaked and appear when uncloaked.
        EVENT_OBJECT_SHOW | EVENT_OBJECT_UNCLOAKED => {
            let h = hwnd.0 as isize;
            if surfaces::scan::cloaked(hwnd) {
                DIRTY.set(true);
                return;
            }
            if pair(h, Seen::Shown) {
                push(Event::NewWindow(h));
            }
        }
        EVENT_SYSTEM_FOREGROUND => push(Event::Foreground(hwnd.0 as isize)),
        _ => {}
    }
    DIRTY.set(true);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Run,
    Install,
    Uninstall,
}

struct Args {
    action: Action,
    perform: bool,
    scale: Option<i32>,
    console: bool,
    run_for: Option<Duration>,
}

fn parse_args() -> Args {
    let mut a = Args {
        action: Action::Run,
        perform: false,
        scale: None,
        console: false,
        run_for: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--scale" => a.scale = it.next().and_then(|v| v.parse().ok()).map(|s: i32| s.clamp(1, 12)),
            "--console" => a.console = true,
            "--perform" => a.perform = true,
            "--install" => a.action = Action::Install,
            "--uninstall" => a.action = Action::Uninstall,
            "--run-for" => {
                a.run_for = it
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(Duration::from_secs_f64)
            }
            _ => {}
        }
    }
    a
}

/// Writes log lines to the log file and, with --console, to stderr.
struct Tee {
    file: Option<std::fs::File>,
    console: bool,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(f) = self.file.as_mut() {
            let _ = f.write_all(buf);
        }
        if self.console {
            let _ = std::io::stderr().write_all(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = self.file.as_mut() {
            let _ = f.flush();
        }
        Ok(())
    }
}

fn init_logging(console: bool) {
    let rust_log = std::env::var("RUST_LOG").ok();
    if !console && rust_log.is_none() {
        return;
    }
    if console {
        unsafe {
            let has_stderr = GetStdHandle(STD_ERROR_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
            if !has_stderr && AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
                let _ = AllocConsole();
            }
        }
    }
    let path = config::dir().join("clawd-desktop.log");
    let file = std::fs::File::create(&path).ok();
    let mut b = env_logger::Builder::new();
    b.filter_level(log::LevelFilter::Info);
    if let Some(spec) = rust_log.as_deref() {
        b.parse_filters(spec);
    }
    b.target(env_logger::Target::Pipe(Box::new(Tee { file, console })));
    b.init();
    info!(
        "clawd-desktop {} logging to {}",
        env!("CARGO_PKG_VERSION"),
        path.display()
    );
}

fn idle_secs() -> f32 {
    let mut li = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    unsafe {
        if !GetLastInputInfo(&mut li).as_bool() {
            return 0.0;
        }
        GetTickCount().wrapping_sub(li.dwTime) as f32 / 1000.0
    }
}

/// One Clawd's overlays (one per monitor) and his last rendered frame.
#[derive(Default)]
struct View {
    overlays: Vec<Overlay>,
    buf: Vec<u32>,
    buf_key: Option<u64>,
    /// Where `buf` last went on screen: top-left, width and height.
    origin: (i32, i32),
    size: (i32, i32),
}

/// The left button went down on a Clawd and is not up yet.
#[derive(Clone, Copy, Debug)]
struct Press {
    clawd: usize,
    at: (i32, i32),
    /// From the cursor to his centre line and feet.
    offset: (i32, i32),
    dragging: bool,
    /// The second press of a double click: its release is not a click.
    double: bool,
}

struct App {
    cfg: Config,
    scale: i32,
    monitors: Vec<MonitorInfo>,
    surfaces: Vec<Surface>,
    crowd: Crowd,
    views: Vec<View>,
    seeds: Rng,
    fullscreen: bool,
    hidden: bool,
    /// What the leans are worked out from, and the leans themselves.
    context: leans::Context,
    /// Claude Code's sessions, while Follow Claude Code is on, and the mood
    /// they add up to.
    watch: Option<mood::watch::Watch>,
    mood: Option<mood::Mood>,
    /// The newest celebration already played.
    celebrated_at: u64,
    leans: Vec<f32>,
    last_foreground_react: Option<Instant>,
    /// A foreground change waiting for `FOREGROUND_DELAY`.
    pending_foreground: Option<Instant>,
    press: Option<Press>,
    /// A single click waiting out the double-click time: Clawd and when.
    pending_click: Option<(usize, Instant)>,
    /// The Clawd the open menu was opened on with a right click.
    menu_clawd: Option<usize>,
    last_rebuild: Instant,
    frames_drawn: u64,
    steps: u64,
    rebuilds: u64,
}

impl App {
    fn headroom(&self) -> i32 {
        SHEETS_WALK_H * self.scale
    }

    fn new_view(&self) -> View {
        let (mw, mh) = max_size();
        let (cw, ch) = (mw as i32 * self.scale, mh as i32 * self.scale);
        View {
            overlays: self
                .monitors
                .iter()
                .filter_map(|m| Overlay::new(m.rect, cw, ch))
                .collect(),
            ..View::default()
        }
    }

    /// One view per Clawd: drops views of removed Clawds, adds new ones.
    fn sync_views(&mut self) {
        self.views.truncate(self.crowd.len());
        while self.views.len() < self.crowd.len() {
            let v = self.new_view();
            self.views.push(v);
        }
    }

    fn rebuild_overlays(&mut self) {
        self.views.clear();
        self.monitors = surfaces::scan::monitors();
        self.sync_views();
        let (mw, mh) = max_size();
        let (cw, ch) = (mw as i32 * self.scale, mh as i32 * self.scale);
        for (i, m) in self.monitors.iter().enumerate() {
            info!(
                "monitor {i}: {}x{} at ({}, {}), work bottom {}{}",
                m.rect.width(),
                m.rect.height(),
                m.rect.left,
                m.rect.top,
                m.work.bottom,
                if m.primary { ", primary" } else { "" }
            );
        }
        info!(
            "overlays up to {}x{} px at scale {}, one per Clawd per monitor",
            cw, ch, self.scale
        );
    }

    /// Where a Clawd drops in when there is no surface to put him on: the
    /// top of the primary monitor.
    fn fallback_start(&mut self) -> (i32, i32) {
        let m = self
            .monitors
            .iter()
            .find(|m| m.primary)
            .or(self.monitors.first())
            .copied();
        let Some(m) = m else { return (0, 0) };
        let margin = 40 * self.scale;
        let lo = m.work.left + margin;
        let hi = (m.work.right - margin).max(lo + 1);
        let x = self.seeds.range(lo as f32, hi as f32) as i32;
        (x, m.rect.top + SHEETS_WALK_H * self.scale)
    }

    fn set_count(&mut self, n: usize) {
        let fallback = self.fallback_start();
        let had = self.crowd.len();
        self.crowd.resize(n, &self.surfaces, &mut self.seeds, fallback);
        self.sync_views();
        for (i, m) in self.crowd.members.iter().enumerate().skip(had) {
            info!(
                "Clawd {i} starts at ({}, {}), pace {:.2}",
                m.actor.x, m.actor.y, m.actor.speed_mul
            );
        }
        if self.crowd.len() != had {
            info!("{} Clawd(s)", self.crowd.len());
        }
    }

    fn rebuild_surfaces(&mut self) {
        let wins = surfaces::scan::windows();
        let lanes = self
            .cfg
            .free_roam
            .then(|| (actor::LANE_SPACING * (SHEETS_WALK_H * self.scale) as f32).round() as i32);
        self.surfaces = surfaces::build_with(&wins, &self.monitors, self.headroom(), lanes);
        self.fullscreen = surfaces::scan::foreground_is_fullscreen();
        self.last_rebuild = Instant::now();
        self.rebuilds += 1;
        if self.rebuilds == 1 {
            info!("{} windows enumerated, {} surfaces:", wins.len(), self.surfaces.len());
            for s in &self.surfaces {
                info!(
                    "  y {:5}  x {:5}..{:5}  {}",
                    s.y,
                    s.x0,
                    s.x1,
                    if s.ground {
                        "ground".to_string()
                    } else {
                        format!("{} window(s)", s.parts.len())
                    }
                );
            }
        } else {
            debug!(
                "rebuild: {} windows, {} surfaces, fullscreen {}",
                wins.len(),
                self.surfaces.len(),
                self.fullscreen
            );
        }
    }

    /// "walk at (x, y), act snooze at (x, y), ..." for the log.
    fn describe(&self) -> String {
        self.crowd
            .members
            .iter()
            .map(|m| {
                let p = m.actor.pose();
                format!("{} at ({}, {})", m.actor.describe(), p.x, p.y)
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn should_hide(&self) -> bool {
        self.cfg.paused || (self.cfg.hide_fullscreen && self.fullscreen)
    }

    fn hide_all(&mut self) {
        for v in &mut self.views {
            for o in &mut v.overlays {
                o.hide();
            }
        }
    }

    fn render(&mut self, i: usize, pose: Pose) {
        let s = self.scale;
        let v = &mut self.views[i];
        let f = frame(pose.sprite, pose.frame);
        let key = {
            let id = match pose.sprite {
                sprites::SpriteRef::Sheet(i) => i as u64,
                sprites::SpriteRef::Act(i) => 1000 + i as u64,
            };
            (id << 32) ^ ((pose.frame as u64) << 8) ^ ((pose.mirrored as u64) << 7) ^ (s as u64)
        };
        let key = key ^ ((pose.mark as u64) << 6) ^ ((pose.rot as u64) << 4);
        let left = f.left(pose.x, s, pose.mirrored);
        let (fw, fh) = (
            f.w as i32 * s,
            (f.h + if pose.mark { sprites::MARK_ROWS } else { 0 }) as i32 * s,
        );
        if v.buf_key != Some(key) {
            f.render(s as usize, pose.mirrored, &mut v.buf);
            if pose.mark {
                sprites::add_mark(&mut v.buf, fw as usize, s as usize, pose.x - left);
            }
            if pose.rot != Rot::Upright {
                let upright = std::mem::take(&mut v.buf);
                sprites::rotate(&upright, fw as usize, fh as usize, pose.rot, &mut v.buf);
            }
            v.buf_key = Some(key);
        }
        // On a wall `x` is the edge and `y` the middle of his body along it;
        // his centre column lands `anchor` rows down (left edge) or up
        // from the bottom (right edge).
        let anchor = pose.x - left;
        let ((sw, sh), pos) = match pose.rot {
            Rot::Upright => ((fw, fh), (left, pose.y - fh)),
            Rot::Left => ((fh, fw), (pose.x, pose.y - anchor)),
            Rot::Right => ((fh, fw), (pose.x - fh, pose.y - (fw - anchor))),
        };
        v.origin = pos;
        v.size = (sw, sh);
        let mut drew = false;
        for o in &mut v.overlays {
            drew |= o.show(&v.buf, sw, sh, pos, key);
        }
        if drew {
            self.frames_drawn += 1;
        }
    }

    /// Re-reads the clock, battery and media state; recomputes the leans
    /// when any changed. `full` also reads the clock and battery.
    fn refresh_leans(&mut self, full: bool) {
        let mut ctx = self.context;
        if full {
            ctx.hour = signals::local_hour();
            ctx.low_battery = signals::low_battery();
        }
        ctx.media = signals::media_playing();
        if ctx != self.context || self.leans.is_empty() {
            self.context = ctx;
            self.set_leans();
        }
    }

    fn set_leans(&mut self) {
        let names = mood::lean_names(self.mood);
        self.leans = leans::combine(&leans::leans(&self.context), &leans::from_names(names));
        info!(
            "leaning toward {:?}, Claude Code {:?}",
            leans::reasons(&self.context),
            self.mood
        );
    }

    fn set_follow(&mut self, on: bool) {
        self.watch = on.then(mood::watch::Watch::new);
        self.update_mood();
    }

    /// Works out Claude Code's mood again; a new celebration makes every
    /// Clawd celebrate at once and hop.
    fn update_mood(&mut self) {
        let now = mood::now_ms();
        let live = self
            .watch
            .as_ref()
            .map(|w| mood::live(&w.sessions, now, mood::watch::alive))
            .unwrap_or_default();
        let m = mood::derive(&live, now);
        self.crowd.lockin = m == Some(mood::Mood::Lockin);
        if m != self.mood {
            self.mood = m;
            self.set_leans();
        }
        if let Some(at) = mood::newest_celebration(&live, now)
            && at > self.celebrated_at
        {
            self.celebrated_at = at;
            info!("Claude Code celebrates");
            for i in 0..self.crowd.len() {
                self.play_for_user(i, &react::CELEBRATION_ACTS, true);
            }
        }
    }

    /// A new top-level window: a terminal, browser or Explorer window makes
    /// the nearest Clawd that is free react.
    fn new_window(&mut self, h: isize) {
        let Some((exe, class, rect)) = react::win::describe(HWND(h as *mut _)) else {
            return;
        };
        let Some(kind) = react::classify(&exe, &class) else {
            debug!("new window {class} of {exe}: no reaction");
            return;
        };
        let at = ((rect.left + rect.right) / 2, rect.top);
        let points: Vec<(i32, i32)> = self
            .crowd
            .members
            .iter()
            .map(|m| (m.actor.x as i32, m.actor.y as i32))
            .collect();
        for i in react::nearest_first(&points, at) {
            let m = &mut self.crowd.members[i];
            let acts = react::acts_for(kind);
            let act = act_index(acts[(m.rng.next_u64() % acts.len() as u64) as usize]);
            if m.actor.react(act, Trigger::Desktop, false) {
                info!("new {kind:?} window ({class}): Clawd {i} plays {}", ACTS[act].name);
                // The foreground change it caused is this reaction.
                self.pending_foreground = None;
                return;
            }
        }
        debug!("new {kind:?} window: every Clawd is busy");
    }

    /// The foreground window changed: one free Clawd turns or looks around.
    fn foreground(&mut self, now: Instant) {
        if self.last_foreground_react.is_some_and(|t| now - t < FOREGROUND_REST) {
            return;
        }
        let n = self.crowd.len();
        if n == 0 {
            return;
        }
        let start = (self.seeds.next_u64() % n as u64) as usize;
        for k in 0..n {
            let i = (start + k) % n;
            let m = &mut self.crowd.members[i];
            let name = react::FOREGROUND_ACTS[(m.rng.next_u64() % 2) as usize];
            if m.actor.react(act_index(name), Trigger::Desktop, false) {
                debug!("foreground change: Clawd {i} plays {name}");
                self.last_foreground_react = Some(now);
                return;
            }
        }
    }

    fn set_clickable(&mut self, on: bool) {
        overlay::set_clickable_default(on);
        for v in &mut self.views {
            for o in &mut v.overlays {
                o.set_clickable(on);
            }
        }
        if !on {
            self.drop_press();
        }
    }

    /// Which Clawd an overlay window belongs to.
    fn clawd_of(&self, id: isize) -> Option<usize> {
        self.views.iter().position(|v| v.overlays.iter().any(|o| o.id() == id))
    }

    /// Lets go of whatever the mouse was holding.
    fn drop_press(&mut self) {
        if let Some(p) = self.press.take()
            && p.dragging
            && let Some(m) = self.crowd.members.get_mut(p.clawd)
        {
            m.actor.release();
        }
    }

    /// One act for Clawd `i` because of the user: `names` to pick from, or
    /// the usual weighted pick with the leans when empty.
    fn play_for_user(&mut self, i: usize, names: &[&str], then_hop: bool) {
        let Some(m) = self.crowd.members.get_mut(i) else { return };
        let act = if names.is_empty() {
            m.actor.pick(&self.leans, &mut m.rng)
        } else {
            act_index(names[(m.rng.next_u64() % names.len() as u64) as usize])
        };
        if m.actor.react(act, Trigger::User, then_hop) {
            info!("Clawd {i} plays {}", ACTS[act].name);
        }
    }

    /// Clicks, double clicks, drags and right clicks on a Clawd.
    fn mouse(&mut self, m: Mouse, now: Instant) -> bool {
        match m {
            Mouse::Down(id, p) | Mouse::Double(id, p) => {
                let Some(i) = self.clawd_of(id) else { return false };
                let v = &self.views[i];
                if !hit::opaque_at(&v.buf, v.origin, v.size.0, v.size.1, p) {
                    debug!("press at {p:?} missed Clawd {i}'s pixels");
                    return false;
                }
                let a = self.crowd.members[i].actor.pose();
                let double = matches!(m, Mouse::Double(..));
                self.press = Some(Press {
                    clawd: i,
                    at: p,
                    offset: (a.x - p.0, a.y - p.1),
                    dragging: false,
                    double,
                });
                if double {
                    self.pending_click = None;
                    self.play_for_user(i, &react::CELEBRATION_ACTS, false);
                }
            }
            Mouse::Move(_, p) => {
                let Some(mut press) = self.press else { return false };
                let slop = unsafe { GetSystemMetrics(SM_CXDRAG) }.max(2);
                if !press.dragging && ((p.0 - press.at.0).abs() > slop || (p.1 - press.at.1).abs() > slop) {
                    press.dragging = true;
                    self.pending_click = None;
                    self.crowd.members[press.clawd].actor.grab();
                    info!("Clawd {} picked up", press.clawd);
                }
                if press.dragging {
                    let a = &mut self.crowd.members[press.clawd].actor;
                    a.drag_to(p.0 + press.offset.0, p.1 + press.offset.1);
                    let pose = a.pose();
                    self.render(press.clawd, pose);
                }
                self.press = Some(press);
            }
            Mouse::Up(..) => {
                let Some(press) = self.press.take() else { return false };
                if press.dragging {
                    self.crowd.members[press.clawd].actor.release();
                    info!("Clawd {} let go", press.clawd);
                } else if !press.double {
                    let wait = Duration::from_millis(unsafe { GetDoubleClickTime() } as u64);
                    self.pending_click = Some((press.clawd, now + wait));
                }
            }
            Mouse::Lost(_) => self.drop_press(),
            Mouse::Right(id, _) => {
                self.menu_clawd = self.clawd_of(id);
                return true;
            }
        }
        false
    }

    fn set_scale(&mut self, s: i32) {
        self.scale = s;
        self.crowd.set_scale(s);
        self.rebuild_overlays();
        self.rebuild_surfaces();
    }

    fn handle(&mut self, cmd: Command) -> bool {
        info!("menu: {cmd:?}");
        match cmd {
            Command::TogglePause => {
                self.cfg.paused = !self.cfg.paused;
                if !self.cfg.paused {
                    self.rebuild_surfaces();
                }
            }
            Command::Scale(s) => {
                self.cfg.scale = s;
                self.set_scale(s);
            }
            Command::Count(n) => {
                self.cfg.count = n;
                self.set_count(n);
            }
            Command::ToggleFullscreen => self.cfg.hide_fullscreen = !self.cfg.hide_fullscreen,
            Command::Perform => self.perform(),
            Command::Float => {
                if let Some(i) = self.menu_clawd.take() {
                    self.float(i);
                }
                return true;
            }
            Command::ToggleFollow => {
                self.cfg.follow_claude = !self.cfg.follow_claude;
                self.set_follow(self.cfg.follow_claude);
            }
            Command::ToggleClickable => {
                self.cfg.clickable = !self.cfg.clickable;
                self.set_clickable(self.cfg.clickable);
            }
            Command::ToggleAntics => {
                self.cfg.crowd_antics = !self.cfg.crowd_antics;
                self.crowd.antics = self.cfg.crowd_antics;
            }
            Command::ToggleRoam => {
                self.cfg.free_roam = !self.cfg.free_roam;
                self.rebuild_surfaces();
            }
            Command::ToggleClimb => self.cfg.climb_walls = !self.cfg.climb_walls,
            Command::ToggleStartup => {
                if let Err(e) = install::set_startup(!install::startup_enabled()) {
                    warn!("start with Windows: {e}");
                }
            }
            Command::Quit => return false,
        }
        self.cfg.save();
        true
    }
}

const SHEETS_WALK_H: i32 = 18;

/// `--install` and `--uninstall`: report to the parent console, if any.
fn install_action(action: Action) -> i32 {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    let r = if action == Action::Install {
        install::install()
    } else {
        install::uninstall()
    };
    match r {
        Ok(lines) => {
            for l in lines {
                println!("{l}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn main() {
    let args = parse_args();
    if args.action != Action::Run {
        std::process::exit(install_action(args.action));
    }
    init_logging(args.console);

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\clawd-desktop")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if args.perform {
            signal_perform();
        } else {
            info!("already running; exiting");
        }
        return;
    }
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();

    let code = run(&args);

    if com {
        unsafe { CoUninitialize() };
    }
    if let Ok(m) = mutex {
        unsafe {
            let _ = CloseHandle(m);
        }
    }
    info!("clean exit");
    log::logger().flush();
    std::process::exit(code);
}

/// `--perform` with a copy already running: tells it through its control
/// window, the same one the single-instance check protects.
fn signal_perform() {
    unsafe {
        let msg = RegisterWindowMessageW(PERFORM_MESSAGE);
        match FindWindowW(CONTROL_CLASS, PCWSTR::null()) {
            Ok(h) if msg != 0 => match PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0)) {
                Ok(()) => info!("asked the running copy to perform"),
                Err(e) => warn!("--perform: {e}"),
            },
            _ => warn!("--perform: the running copy has no control window"),
        }
    }
}

impl App {
    /// Clawd `i` plays the space act in zero g, inside the work area of
    /// the monitor he is on.
    fn float(&mut self, i: usize) {
        let areas: Vec<surfaces::Rect> = self.monitors.iter().map(|m| m.work).collect();
        if self.crowd.float(i, &areas) {
            info!("Clawd {i} floats: {}", self.describe());
        } else {
            info!("Clawd {i} cannot float now");
        }
    }

    /// Every Clawd plays an act now, with the leans.
    fn perform(&mut self) {
        info!("perform");
        for i in 0..self.crowd.len() {
            self.play_for_user(i, &[], false);
        }
    }
}

fn run(args: &Args) -> i32 {
    overlay::register_class();
    let hinst = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default() };
    let control = unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(control_proc),
            hInstance: hinst.into(),
            lpszClassName: CONTROL_CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        PERFORM_MSG.set(RegisterWindowMessageW(PERFORM_MESSAGE));
        match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CONTROL_CLASS,
            w!("Clawd Desktop"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                warn!("control window: {e}");
                return 1;
            }
        }
    };

    let hooks: Vec<HWINEVENTHOOK> = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
    ]
    .iter()
    .map(|&(a, b)| unsafe {
        SetWinEventHook(
            a,
            b,
            None,
            Some(win_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    })
    .filter(|h| !h.is_invalid())
    .collect();
    info!("{} window event hooks", hooks.len());

    let tray = Tray::new(control);
    let hotkey = unsafe { RegisterHotKey(Some(control), HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x43) };
    match &hotkey {
        Ok(()) => info!("Ctrl+Alt+C performs"),
        Err(e) => warn!("Ctrl+Alt+C is taken ({e}); Perform is still in the tray menu"),
    }

    let mut cfg = Config::load();
    let scale = args.scale.unwrap_or(cfg.scale);
    if args.scale.is_some() {
        info!("scale {scale} from --scale (not saved)");
    } else {
        cfg.scale = scale;
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64);
    let walk_h = sprites::SHEETS[sheet_index("walk")].h as i32;
    debug_assert_eq!(walk_h, SHEETS_WALK_H);
    let count = cfg.count;
    overlay::set_clickable_default(cfg.clickable);

    let mut app = App {
        cfg,
        scale,
        monitors: Vec::new(),
        surfaces: Vec::new(),
        crowd: Crowd::new(scale),
        views: Vec::new(),
        seeds: Rng::new(seed),
        fullscreen: false,
        hidden: false,
        context: leans::Context::default(),
        watch: None,
        mood: None,
        celebrated_at: 0,
        leans: Vec::new(),
        last_foreground_react: None,
        pending_foreground: None,
        press: None,
        pending_click: None,
        menu_clawd: None,
        last_rebuild: Instant::now(),
        frames_drawn: 0,
        steps: 0,
        rebuilds: 0,
    };
    app.crowd.antics = app.cfg.crowd_antics;
    app.rebuild_overlays();
    app.rebuild_surfaces();
    app.set_count(count);
    signals::start_media_watch();
    app.refresh_leans(true);
    let follow = app.cfg.follow_claude;
    app.set_follow(follow);
    if args.perform {
        push(Event::Perform);
    }

    let started = Instant::now();
    let deadline = args.run_for.map(|d| started + d);
    let mut next_step = Instant::now();
    let mut next_status = started + STATUS_EVERY;
    let mut next_signals = started + SIGNALS_EVERY;
    let mut running = true;

    while running {
        let now = Instant::now();
        let hide = app.should_hide();
        let mut wait = if app.cfg.paused {
            Duration::MAX
        } else if hide {
            (app.last_rebuild + POLL).saturating_duration_since(now)
        } else {
            next_step
                .saturating_duration_since(now)
                .min((app.last_rebuild + POLL).saturating_duration_since(now))
        };
        if let Some(d) = deadline {
            wait = wait.min(d.saturating_duration_since(now));
        }
        let ms = if wait == Duration::MAX {
            u32::MAX
        } else {
            wait.as_millis().min(u32::MAX as u128 - 1) as u32
        };
        if ms > 0 {
            // The sessions folder's change handle wakes the loop too.
            let handles: Vec<_> = app.watch.as_ref().and_then(|w| w.handle()).into_iter().collect();
            let handles = (!handles.is_empty()).then_some(&handles[..]);
            unsafe {
                MsgWaitForMultipleObjectsEx(handles, ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
        }

        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                running = false;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        for e in EVENTS.with(|q| std::mem::take(&mut *q.borrow_mut())) {
            match e {
                Event::Quit => running = false,
                Event::TaskbarCreated => tray.add(),
                Event::Perform if !app.hidden && !app.cfg.paused => app.perform(),
                Event::Perform => {}
                Event::DisplayChange => {
                    info!("display change");
                    app.rebuild_overlays();
                    app.rebuild_surfaces();
                }
                Event::NewWindow(h) if !app.hidden && !app.cfg.paused => app.new_window(h),
                Event::Foreground(_) if !app.hidden && !app.cfg.paused => {
                    app.pending_foreground = Some(Instant::now() + FOREGROUND_DELAY)
                }
                Event::NewWindow(_) | Event::Foreground(_) => {}
                Event::TrayMenu => {
                    app.menu_clawd = None;
                    if let Some(cmd) = tray.menu(&app.cfg, false) {
                        running &= app.handle(cmd);
                    }
                }
            }
        }

        let now = Instant::now();
        if let Some(w) = app.watch.as_mut()
            && w.poll(now)
        {
            app.update_mood();
        }
        let mut menu = false;
        for m in overlay::take_mouse() {
            if !app.hidden {
                menu |= app.mouse(m, now);
            }
        }
        if menu && let Some(cmd) = tray.menu(&app.cfg, app.menu_clawd.is_some()) {
            running &= app.handle(cmd);
        }
        app.menu_clawd = None;
        if let Some((i, at)) = app.pending_click
            && now >= at
        {
            app.pending_click = None;
            app.play_for_user(i, &[], false);
        }
        if deadline.is_some_and(|d| now >= d) {
            info!("--run-for elapsed after {:.1} s", (now - started).as_secs_f32());
            running = false;
        }
        if !running {
            break;
        }

        if !app.cfg.paused {
            let since = now - app.last_rebuild;
            if (DIRTY.get() && since >= MIN_REBUILD) || since >= POLL {
                DIRTY.set(false);
                app.rebuild_surfaces();
            }
        }

        let hide = app.should_hide();
        if hide != app.hidden {
            info!("{}", if hide { "hiding" } else { "showing" });
            app.hidden = hide;
            if hide {
                app.hide_all();
            }
        }
        if !hide && now >= next_step {
            let areas: Vec<surfaces::Rect> = app.monitors.iter().map(|m| m.work).collect();
            let world = World {
                surfaces: &app.surfaces,
                idle_secs: idle_secs(),
                leans: &app.leans,
                areas: &areas,
                roam: app.cfg.free_roam,
                climb: app.cfg.climb_walls,
            };
            for e in app.crowd.step(&world) {
                info!("{e}");
            }
            for i in app.crowd.space_wishes() {
                app.float(i);
            }
            app.steps += 1;
            for i in 0..app.crowd.len() {
                let pose = app.crowd.members[i].actor.pose();
                app.render(i, pose);
            }
            next_step += STEP;
            if next_step < now {
                next_step = now + STEP;
            }
        }

        if app.pending_foreground.is_some_and(|t| now >= t) {
            app.pending_foreground = None;
            app.foreground(now);
        }

        if now >= next_signals {
            next_signals = now + SIGNALS_EVERY;
            app.refresh_leans(true);
        } else if !hide && app.steps.is_multiple_of(FPS as u64) {
            app.refresh_leans(false);
        }

        if now >= next_status {
            next_status = now + STATUS_EVERY;
            info!(
                "status: {}; {} surfaces, {} steps, {} frames drawn, {} rebuilds{}",
                app.describe(),
                app.surfaces.len(),
                app.steps,
                app.frames_drawn,
                app.rebuilds,
                if app.hidden { ", hidden" } else { "" }
            );
        }
    }

    info!(
        "exiting: {} steps, {} frames drawn, {} surface rebuilds; {}",
        app.steps,
        app.frames_drawn,
        app.rebuilds,
        app.describe()
    );
    app.cfg.save();
    for h in hooks {
        unsafe {
            let _ = UnhookWinEvent(h);
        }
    }
    if hotkey.is_ok() {
        unsafe {
            let _ = UnregisterHotKey(Some(control), HOTKEY_ID);
        }
    }
    drop(tray);
    drop(app);
    unsafe {
        let _ = DestroyWindow(control);
    }
    0
}
