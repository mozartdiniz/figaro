set dotenv-load

# Figaro — GTK app + castlabs Electron playback engine

# Run the app (debug)
run:
    cargo run

# Run with logs, and show the engine's hidden window for debugging
debug:
    RUST_LOG=figaro=debug FIGARO_ENGINE_SHOW=1 cargo run

test:
    cargo test

# Bundle the playback engine with castlabs Electron into engine/bundle/ (Linux).
# First run downloads castlabs Electron (~120 MB).
engine:
    engine/bundle.sh

# Probe the API with your key, outside the GTK app (see applemusic/examples/probe.rs)
probe term="mozart":
    cargo run -p applemusic --example probe -- "{{term}}"
