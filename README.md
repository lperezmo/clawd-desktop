# Clawd Desktop

Clawd, the pixel lobster from claude.dev, walks along the top edges of your windows and the taskbar on Windows. He lives in transparent, click-through, topmost overlay windows, so he never takes focus or input and does not show up in Alt-Tab. Up to five of him can walk at once.

<p align="center"><img src="assets/clawd-256.png" width="128" alt="Clawd"></p>

## What it looks like

- **Surfaces**: he walks on the visible top edge of every window and on the taskbar, turns at the ends, and rides along when you drag the window he is standing on.
- **Falling**: when that window closes, minimizes or moves out from under him, he falls to the next edge below.
- **Acts**: every 20 to 60 seconds he stops for one of claude.dev's acts, or rides a skateboard, car, scooter or bike along the taskbar.
- **Naps**: after two minutes without input he picks a rest act and keeps at it until you are back.
- **Company**: with more than one Clawd, each keeps his own pace and timers; two walking into each other turn apart instead of passing through.

He hides while a fullscreen app is in front.

## Install

Download `Clawd-Desktop-X.Y.Z.exe` from Releases. Either just run it, or run it once with `--install`:

```
Clawd-Desktop-X.Y.Z.exe --install
```

That copies it to `%LOCALAPPDATA%\Programs\clawd-desktop\clawd-desktop.exe` and puts a Clawd shortcut on the desktop. Start with Windows in the tray menu then launches the installed copy. `--uninstall` removes the copy and both shortcuts.

## Tray menu

- Pause / Resume
- Clawds: 1 to 5
- Scale: 2x, 3x, 4x, 6x
- Hide while fullscreen (on by default)
- Start with Windows
- Quit

Settings are saved in `%APPDATA%\clawd-desktop\config.toml`.

## Flags

- `--install`: copy the exe to the install folder and make the desktop shortcut.
- `--uninstall`: remove the installed copy, the desktop shortcut and the Startup shortcut.
- `--scale N`: screen pixels per sprite pixel (default 4, or the saved setting). Not saved.
- `--console`: log to the parent console (or a new one) and to `%APPDATA%\clawd-desktop\clawd-desktop.log`.
- `--run-for SECS`: quit cleanly after that many seconds.

Only one copy runs at a time.

## Build from source

Needs the stable Rust toolchain (MSVC).

```
cargo build --release
cargo test
```

The sprite tables in `src/sprites_data.rs` are generated from clawd-buddy's `hooks/sprites.ts` and committed. To regenerate them, and the icon from them:

```
bun tools/gen-sprites.ts --repo <clawd-buddy path> --ref <branch>
cargo run --example make-icon
```

## How it works

- One small layered window per Clawd per monitor, sized to his current frame and moved with each update.
- Surfaces come from `DWMWA_EXTENDED_FRAME_BOUNDS`, so edges match what you see, not the invisible resize borders.
- Frames are pushed with `UpdateLayeredWindow` at 12 fps, and only when the frame or position changed.
- `GetLastInputInfo` tells him when you have been away for two minutes.
- Settings live in `%APPDATA%\clawd-desktop\config.toml`.

## License and credits

The code is MIT, see [LICENSE](LICENSE). Clawd and his sprites are Anthropic's, from https://claude.dev, and are not covered by that license; see [NOTICE](NOTICE).
