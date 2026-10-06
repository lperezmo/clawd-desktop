# CHANGELOG


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
