# rs-sensors

A terminal UI (TUI) sensor monitor in Rust, built on top of [libsensors](https://github.com/meitamas/libsensors) — the same library used by the `sensors` utility from the `lm-sensors` package.

It shows temperatures, fans, voltages, power, current, energy, and humidity in real time (refreshed every second), with color-coded status indicators (OK / WARN / ALARM) and a graphical bar for temperatures.

## Features

- Live polling of all available sensors once per second
- Filtering by type: All / Temp / Fan / Power / Voltage / Current / Energy (`f` key)
- Sorting: by value (descending) or by chip order (`s` key)
- Status coloring: green OK, yellow WARN (beyond min/max), red ALARM (beyond crit threshold or alarm flag)
- Text mode for scripting: `--list` / `-l` prints a single snapshot as aligned columns and exits

## Dependencies

- Rust (edition 2024)
- A C compiler (gcc/clang) — for building the FFI wrapper
- `libsensors` with headers (`liblsensors-dev` / `libsensors` providing `include/sensors`)

Key crates: `ratatui` (TUI), `crossterm` (input/terminal), `anyhow` (error handling), `cc` (compiling the C wrapper in the build script).

## Building

```sh
cargo build
```

## Running

TUI mode:

```sh
cargo run
```

One-shot text output:

```sh
cargo run -- --list
```

## Controls

| Key | Action |
|---|---|
| `q` | quit |
| `↑` / `↓` (also PgUp/PgDn, Home/End) | move selection |
| `f` | cycle filter |
| `s` | toggle sort |
| `r` | force refresh now |
| `?` or `h` | toggle help |

## Project layout

- `src/main.rs` — TUI, sensor data model, FFI, status logic
- `src/c/sensors_wrapper.c` — thin C wrapper over `libsensors`
- `build.rs` — compiles the wrapper and links `-lsensors`

## License

[MIT](LICENSE.md) — Copyright (c) 2026 Sergey Kruglov
