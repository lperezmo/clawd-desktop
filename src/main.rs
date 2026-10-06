//! Clawd walks along the edges of your windows and the taskbar.

#![cfg_attr(not(test), windows_subsystem = "windows")]

mod actor;
mod config;
mod install;
mod overlay;
mod sprites;
#[rustfmt::skip]
mod sprites_data;
mod surfaces;
mod tray;

use actor::{Actor, Pose, Rng, World};
use config::Config;
use log::{debug, info, warn};
use overlay::Overlay;
use sprites::{FPS, frame, max_size, sheet_index};
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
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EVENT_OBJECT_DESTROY,
    EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND, GA_ROOT, GetAncestor, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, OBJID_WINDOW, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
    RegisterWindowMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_CLOSE, WM_CONTEXTMENU,
    WM_DISPLAYCHANGE, WM_ENDSESSION, WM_LBUTTONUP, WM_QUIT, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::w;

const STEP: Duration = Duration::from_nanos(1_000_000_000 / FPS as u64);
const POLL: Duration = Duration::from_millis(500);
const MIN_REBUILD: Duration = Duration::from_millis(100);
const STATUS_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    TrayMenu,
    DisplayChange,
    TaskbarCreated,
    Quit,
}

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static DIRTY: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
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
    if event == EVENT_OBJECT_LOCATIONCHANGE && unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd {
        return;
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
    scale: Option<i32>,
    console: bool,
    run_for: Option<Duration>,
}

fn parse_args() -> Args {
    let mut a = Args {
        action: Action::Run,
        scale: None,
        console: false,
        run_for: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--scale" => a.scale = it.next().and_then(|v| v.parse().ok()).map(|s: i32| s.clamp(1, 12)),
            "--console" => a.console = true,
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

struct App {
    cfg: Config,
    scale: i32,
    monitors: Vec<MonitorInfo>,
    overlays: Vec<Overlay>,
    surfaces: Vec<Surface>,
    actor: Actor,
    rng: Rng,
    fullscreen: bool,
    hidden: bool,
    buf: Vec<u32>,
    buf_key: Option<u64>,
    last_rebuild: Instant,
    frames_drawn: u64,
    steps: u64,
    rebuilds: u64,
}

impl App {
    fn headroom(&self) -> i32 {
        SHEETS_WALK_H * self.scale
    }

    fn rebuild_overlays(&mut self) {
        self.overlays.clear();
        self.monitors = surfaces::scan::monitors();
        let (mw, mh) = max_size();
        let (cw, ch) = (mw as i32 * self.scale, mh as i32 * self.scale);
        for m in &self.monitors {
            if let Some(o) = Overlay::new(m.rect, cw, ch) {
                self.overlays.push(o);
            }
        }
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
            "{} overlays, {}x{} px each at scale {}",
            self.overlays.len(),
            cw,
            ch,
            self.scale
        );
        self.buf_key = None;
    }

    fn rebuild_surfaces(&mut self) {
        let wins = surfaces::scan::windows();
        self.surfaces = surfaces::build(&wins, &self.monitors, self.headroom());
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

    fn should_hide(&self) -> bool {
        self.cfg.paused || (self.cfg.hide_fullscreen && self.fullscreen)
    }

    fn hide_all(&mut self) {
        for o in &mut self.overlays {
            o.hide();
        }
    }

    fn render(&mut self, pose: Pose) {
        let f = frame(pose.sprite, pose.frame);
        let s = self.scale;
        let key = {
            let id = match pose.sprite {
                sprites::SpriteRef::Sheet(i) => i as u64,
                sprites::SpriteRef::Act(i) => 1000 + i as u64,
            };
            (id << 32) ^ ((pose.frame as u64) << 8) ^ ((pose.mirrored as u64) << 7) ^ (s as u64)
        };
        if self.buf_key != Some(key) {
            f.render(s as usize, pose.mirrored, &mut self.buf);
            self.buf_key = Some(key);
        }
        let (sw, sh) = (f.w as i32 * s, f.h as i32 * s);
        let pos = (f.left(pose.x, s, pose.mirrored), pose.y - sh);
        let mut drew = false;
        for o in &mut self.overlays {
            drew |= o.show(&self.buf, sw, sh, pos, key);
        }
        if drew {
            self.frames_drawn += 1;
        }
    }

    fn set_scale(&mut self, s: i32) {
        self.scale = s;
        self.actor.scale = s;
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
            Command::ToggleFullscreen => self.cfg.hide_fullscreen = !self.cfg.hide_fullscreen,
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
        info!("already running; exiting");
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

fn run(args: &Args) -> i32 {
    overlay::register_class();
    let hinst = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default() };
    let control = unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(control_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("ClawdDesktopControl"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("ClawdDesktopControl"),
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
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
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
    let mut rng = Rng::new(seed);
    let walk_h = sprites::SHEETS[sheet_index("walk")].h as i32;
    debug_assert_eq!(walk_h, SHEETS_WALK_H);

    let mut app = App {
        cfg,
        scale,
        monitors: Vec::new(),
        overlays: Vec::new(),
        surfaces: Vec::new(),
        actor: Actor::new(0, 0, scale, &mut rng),
        rng,
        fullscreen: false,
        hidden: false,
        buf: Vec::new(),
        buf_key: None,
        last_rebuild: Instant::now(),
        frames_drawn: 0,
        steps: 0,
        rebuilds: 0,
    };
    app.rebuild_overlays();
    app.rebuild_surfaces();

    // Drop in from the top of the primary monitor.
    let m = app
        .monitors
        .iter()
        .find(|m| m.primary)
        .or(app.monitors.first())
        .copied();
    if let Some(m) = m {
        let margin = 40 * scale;
        let lo = m.work.left + margin;
        let hi = (m.work.right - margin).max(lo + 1);
        let x = app.rng.range(lo as f32, hi as f32) as i32;
        app.actor = Actor::new(x, m.rect.top + walk_h * scale, scale, &mut app.rng);
        info!("Clawd starts at x {x}, falling from y {}", m.rect.top + walk_h * scale);
    }

    let started = Instant::now();
    let deadline = args.run_for.map(|d| started + d);
    let mut next_step = Instant::now();
    let mut next_status = started + STATUS_EVERY;
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
            unsafe {
                MsgWaitForMultipleObjectsEx(None, ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
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
                Event::DisplayChange => {
                    info!("display change");
                    app.rebuild_overlays();
                    app.rebuild_surfaces();
                }
                Event::TrayMenu => {
                    if let Some(cmd) = tray.menu(&app.cfg) {
                        running &= app.handle(cmd);
                    }
                }
            }
        }

        let now = Instant::now();
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
            let world = World {
                surfaces: &app.surfaces,
                idle_secs: idle_secs(),
            };
            app.actor.step(&world, &mut app.rng);
            app.steps += 1;
            let pose = app.actor.pose();
            app.render(pose);
            next_step += STEP;
            if next_step < now {
                next_step = now + STEP;
            }
        }

        if now >= next_status {
            next_status = now + STATUS_EVERY;
            let p = app.actor.pose();
            info!(
                "status: {} at ({}, {}), {} surfaces, {} steps, {} frames drawn, {} rebuilds{}",
                app.actor.describe(),
                p.x,
                p.y,
                app.surfaces.len(),
                app.steps,
                app.frames_drawn,
                app.rebuilds,
                if app.hidden { ", hidden" } else { "" }
            );
        }
    }

    info!(
        "exiting: {} steps, {} frames drawn, {} surface rebuilds, last pose {} at ({}, {})",
        app.steps,
        app.frames_drawn,
        app.rebuilds,
        app.actor.describe(),
        app.actor.pose().x,
        app.actor.pose().y
    );
    app.cfg.save();
    for h in hooks {
        unsafe {
            let _ = UnhookWinEvent(h);
        }
    }
    drop(tray);
    drop(app);
    unsafe {
        let _ = DestroyWindow(control);
    }
    0
}
