# active-fmt

Formatter for the [Active](https://crates.io/crates/active) configuration and data language.

Parses Active documents and reprints them, which is what canonical layout and editor format-on-save both need.

```sh
cargo install active-fmt
active-fmt --help
```

Installing this crate provides the `active-fmt` binary; the same entry point is available as a library (`active_fmt::format`).

## Status

Stable 1.0 implementation.

## License

Licensed under either of the Apache License, Version 2.0 or the MIT license, at your option.
