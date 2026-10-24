# sentinelle

A lightweight, good-looking system monitor for Linux (Rust + egui/glow).

- **Compute**: one card per CPU / GPU / memory. Each one has a load chart, its own temperature chart (no dual axes), and a footer: per-thread load, VRAM, or swap.
  GPUs: AMD (amdgpu sysfs), Intel (i915/xe clock), NVIDIA (NVML, loaded at runtime).
- **Storage**: each filesystem is a bar split into the folders that use the space. Folders are picked so each one is roughly 10–50% of used space.
  Click a segment or legend entry to open that folder in your file manager. Scans run in the background at idle CPU/IO priority, are cached in `~/.cache/sentinelle`, and repeat every 20 minutes (or when you click *Rescan*).
- **Network**: connection name (SSID / NetworkManager profile), IP, link rate, download/upload sparklines, and latency to a ping server (ICMP, with a TCP fallback).
  Click the host to change it.
- **Applications**: processes grouped into apps (systemd app scopes, then process tree). **Steam games get their own group, named from the Steam library**, instead of hiding inside "Steam".
  Expand an app to see its process tree. *End* asks for confirmation; right-click for *Force kill* and copying the PID or command. Rows stop reordering while the pointer is over the list.

## Build & run

```sh
cargo build --release
./target/release/sentinelle [--ping 9.9.9.9] [--interval 1000]
```

Optional runtime tools: `iw` / `nmcli` (network name and Wi-Fi rate), `fc-match` (uses Inter if installed), `xdg-open`.
`sentinelle --dump` prints what the collectors see, without a window. `--demo` shows synthetic data.

## Layout

- `src/core/`: data collection. It has no UI dependencies. The UI talks to it only through `Monitor` (`snapshot()`, `send(Command)`, `take_notices()`).
- `src/ui/`: rendering. `theme.rs` holds the design language: blue = load, orange = heat, aqua = memory, violet/magenta = traffic. Status colors are reserved for state and always come with a label.
