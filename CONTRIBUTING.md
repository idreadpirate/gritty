# Contributing

## Build & run

```sh
cargo build            # debug
cargo run              # debug, launches the app
cargo build --release  # optimized single exe
```

## Test

```sh
cargo test
```

All pure logic must be covered by unit tests. When adding a feature, put the
logic in a pure function (geometry, parsing, scoring, serialization) and test it
directly; keep the GUI glue in `main.rs` thin. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#testing).

## Style

- Small, focused modules (one concern each).
- Immutable/borrow-friendly patterns; avoid needless `clone`.
- No `unsafe` outside the OS-boundary modules (`proc.rs`'s snapshot).
- Run `cargo fmt` and `cargo clippy` before submitting; no warnings.

## Commits

Conventional commits: `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`,
`perf:`. Keep each commit a coherent, building, tested change.

## Adding a command-palette action

1. Add a variant to `palette::Cmd` and an entry to `palette::COMMANDS`.
2. Handle it in `Gritty::run_cmd`.
3. Add a test in `palette.rs` if it affects filtering.
