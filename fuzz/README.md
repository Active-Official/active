# Cargo-fuzz scaffold for ACTIVE (C-012).
#
# The CI-enforced harness is the in-repo proptest suite
# (active-core/tests/fuzz.rs). These cargo-fuzz targets drive the same
# entry points with libFuzzer for deeper, longer-running fuzzing.
#
# Requirements to run:
#   rustup toolchain install nightly
#   clang on PATH (libfuzzer-sys builds the libFuzzer C++ runtime)
#   cargo install cargo-fuzz
#
# Usage:
#   cargo +nightly fuzz run lexer -- -max_total_time=60
#   cargo +nightly fuzz run parser -- -max_total_time=60
#   cargo +nightly fuzz run interpolation -- -max_total_time=60
#
# Crashes and reproducers land in fuzz/artifacts/ (gitignored).
