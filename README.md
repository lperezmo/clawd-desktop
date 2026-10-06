# clawd-desktop

Clawd, the pixel-art lobster from claude.dev, walks along the top edges of your windows and the taskbar on Windows. He is drawn in transparent, click-through, topmost overlay windows, so he never takes focus or input and does not show up in Alt-Tab.

He walks, turns at the ends, stops now and then for one of claude.dev's acts, sometimes rides a skateboard, car, scooter or bike along the taskbar, and falls to the next edge below when the window under him closes or moves away. After two minutes without input he naps until you are back. He hides while a fullscreen app is in front.

## Build

Needs the stable Rust toolchain (MSVC).

```
cargo build --release
cargo test
```

The binary is `target\release\clawd-desktop.exe`.

## Run

```
target\release\clawd-desktop.exe [--scale N] [--console] [--run-for SECS]
```

- `--scale N`: screen pixels per sprite pixel (default 4, or the saved setting). Not saved.
- `--console`: log to the parent console (or a new one) and to `%APPDATA%\clawd-desktop\clawd-desktop.log`.
- `--run-for SECS`: quit cleanly after that many seconds.
- `RUST_LOG=debug` (or any env_logger filter) also turns on the log file.

Only one copy runs at a time.

## Tray menu

- Pause / Resume
- Scale: 2x, 3x, 4x, 6x
- Hide while fullscreen (on by default)
- Start with Windows (adds or removes a shortcut in the Startup folder)
- Quit

Settings are saved in `%APPDATA%\clawd-desktop\config.toml`.

## Sprites

`src/sprites_data.rs` is generated from clawd-buddy's `plugins/clawd-buddy/hooks/sprites.ts` and committed, so building does not need Bun. To regenerate:

```
bun tools/gen-sprites.ts                      # git show lottie-acts:... from D:/Python/clawd-buddy
bun tools/gen-sprites.ts --from path/to/sprites.ts
bun tools/gen-sprites.ts --repo <clawd-buddy path> --ref <branch>
```

The generator reads the file as text and never runs it.

## License

MIT for the code. Clawd and his sprites are Anthropic's, from https://claude.dev; see NOTICE.
