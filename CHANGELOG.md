# CHANGELOG


## v0.3.0 (2026-10-06)

### Features

- Crowd antics: conversations, laptop pairs, shoves, stacks, moods
  ([#2](https://github.com/lperezmo/clawd-desktop/pull/2),
  [`f421157`](https://github.com/lperezmo/clawd-desktop/commit/f421157bd2a920f4ab9681c64bcc83024fb952a6))

Two Clawds that meet now stop and face each other for a beat before one turns, sometimes ride on
  each other (never three high), rarely shove (the other hops over and lands behind with a startled
  ?!), and sit down to work side by side with laptops while Claude Code is locked in or when one
  walks up to another at his laptop. Each comes away happy or annoyed, which leans his next act.
  Everything one Clawd does because of another is over within about three seconds, apart from the
  laptop acts, which are ordinary acts. The encounter state machine lives in crowd.rs and is unit
  tested. New tray toggle Crowd antics (crowd_antics in config.toml).


## v0.2.0 (2026-10-06)

### Chores

- Keep only the exe on a GitHub release
  ([`bb53dec`](https://github.com/lperezmo/clawd-desktop/commit/bb53dec4a3d685c66156776c3da18cf50d4fda08))

### Features

- Variety pack: shorter acts, clickable Clawd, desktop reactions, Claude Code mood, Perform
  ([#1](https://github.com/lperezmo/clawd-desktop/pull/1),
  [`0a410e5`](https://github.com/lperezmo/clawd-desktop/commit/0a410e553ed6b383a8aad3694646e8d0c71d6f87))

* feat: an act every 8 to 20 seconds, and one in four chains into a second

* feat: Clawd reacts to new terminal, browser and Explorer windows and to focus changes, and leans
  on the clock, battery and media

* feat: a clickable Clawd: click for an act, double click to celebrate, drag to carry him, right
  click for the menu

* feat: follow Claude Code's mood from clawd-buddy's session files: lock in and chill lean the
  picks, a celebration makes every Clawd celebrate

* feat: Perform from the tray, Ctrl+Alt+C or --perform makes every Clawd play an act now

* docs: README for what makes him do things, Clickable, Follow Claude Code and Perform


## v0.1.0 (2026-10-06)

### Chores

- Ci and semantic-release workflows
  ([`533a170`](https://github.com/lperezmo/clawd-desktop/commit/533a1703086233adc4dcc4e3a8cec0528ec6a63b))

CI runs fmt, clippy with -D warnings, tests and a release build on windows-latest. Release cuts a
  version from Conventional Commits with python-semantic-release, bumps Cargo.toml and the crate's
  Cargo.lock entry, tags vX.Y.Z and attaches the built exe to the GitHub release.

- Scaffold the crate, license and notice
  ([`fae5e8d`](https://github.com/lperezmo/clawd-desktop/commit/fae5e8dc27e0303df232509c2be7e05c4f8cf62c))

### Documentation

- Readme
  ([`40defe9`](https://github.com/lperezmo/clawd-desktop/commit/40defe9e752cd132a0a102de2007aa0f0a6cd3b8))

- Readme for releases, install, flags and several Clawds
  ([`746c2e1`](https://github.com/lperezmo/clawd-desktop/commit/746c2e17805bb09c2f1197a8f6f83f99b64ded5a))

### Features

- App icon, version info and --install / --uninstall
  ([`eba2512`](https://github.com/lperezmo/clawd-desktop/commit/eba25128bd33b4ddd37402d2d016324c1b9b7f52))

tools/make-icon.rs (cargo run --example make-icon) renders the walk sheet's frame 0 into
  assets/clawd.ico (16 to 256 px, outlined up to 48) and assets/clawd-256.png. build.rs embeds the
  icon as group 1 with winresource along with the file description and version; the tray loads that
  icon.

--install copies the exe to %LOCALAPPDATA%\Programs\clawd-desktop and makes Desktop\Clawd.lnk
  through IShellLinkW; --uninstall removes both and the Startup shortcut. Start with Windows points
  at the installed copy when there is one.

- Generate Clawd's sprite tables from clawd-buddy's sprites.ts
  ([`ed7cc97`](https://github.com/lperezmo/clawd-desktop/commit/ed7cc976869994c28e928b0b0844b05f14247d7e))

- Layered overlays, tray menu, settings and the main loop
  ([`0aac091`](https://github.com/lperezmo/clawd-desktop/commit/0aac0912cefd41be2330e35e8ff7f9d7f53afc58))

- Up to five Clawds at once
  ([`0807a92`](https://github.com/lperezmo/clawd-desktop/commit/0807a929d06a48248a9ff53f4a3346de90264413))

A count setting (1 to 5, tray menu Clawds) adds or removes Clawds live. Each has his own RNG stream,
  a pace within 15 percent of the sheet speed, act timers and a start spot spread over the walkable
  surfaces. Two walking into each other on the same surface do not pass through: the lower index
  turns around. Each Clawd keeps his own small overlay per monitor.

- Walkable surfaces from window edges and Clawd's behaviour
  ([`f411bc7`](https://github.com/lperezmo/clawd-desktop/commit/f411bc71ee87ab1b5acc5236cd7bc68f6e96b707))
