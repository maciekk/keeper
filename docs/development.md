# Development

For end-user setup and examples, start with the [README](../README.md).

## Build and check

Requires Linux and a recent stable Rust toolchain (edition 2024):

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
python -B -m unittest discover -s tools -p 'test_*.py'
cargo build --release --locked
```

The runnable binary is `target/release/keeper`. Tests use disposable data; testing
on an actual target filesystem needs the precautions in
[filesystem qualification](qualification.md) and its
[repeatable runner](qualification-runner.md). Local tests and process-kill tests
do not establish power-loss durability or whole-drive readiness.

## Why Rust?

Keeper is both a useful integrity tool and a learn-Rust project. Rust also offers
practical benefits for this work:

- Systems-level control, strong filesystem APIs, and low runtime overhead.
- Excellent BLAKE3 support for large-data hashing.
- Explicit control over concurrency and different HDD/SSD needs.
- Direct subprocess support for future semantic validators.
- Distribution as a single executable rather than an application runtime stack.

Prefer a straightforward synchronous pipeline and, when justified by measurements,
controlled worker pools. Do not introduce async or other machinery just to learn
it. Rust is not a substitute for correct filesystem protocols, failure testing, or
independent backups. Follow the [design principles](principles.md).

## Project history and compatibility

This is the Rust rewrite of `maciekk/keeper`. The unused Go experiment remains in
Git history; its last commit is `5d021b2e818bda9e260432b67ac323a22844064e`.

Unscoped roots still write format 1; roots with `.keeperignore` write
[format 2](catalog-v2.md), recording their exact exclusion policy. Both remain
readable without conversion or reindexing. Tests read a golden format-1 catalog
from the foundation release and check unchanged unscoped serialization. See
[catalog format 1](catalog-v1.md) and the
[compatibility commitments](principles.md#5-metadata-evolution-should-preserve-expensive-knowledge).

For implementation status, deferred features, and whole-drive readiness gates,
consult the [roadmap](roadmap.md). The [long-form design](design.md) describes
architectural direction, not a claim that all proposed features exist.
