set dotenv-load

# Figaro — GTK app + CEF playback engine

# Run the app (debug)
run:
    cargo run

# Run with logs, and show the engine's hidden window for debugging
debug:
    RUST_LOG=figaro=debug FIGARO_ENGINE_SHOW=1 cargo run

test:
    cargo test

# Build the playback engine and bundle it with CEF into engine/bundle/ (Linux).
# First run downloads the CEF binary distribution (~300 MB).
engine:
    cargo run -p figaro-engine --bin bundle-engine -- engine/bundle --release

# Probe the API with your key, outside the GTK app (see applemusic/examples/probe.rs)
probe term="mozart":
    cargo run -p applemusic --example probe -- "{{term}}"
