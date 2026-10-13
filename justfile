set dotenv-load

prefix := env_var_or_default("HOME", "/root") + "/.local"
share := prefix + "/share"
app_id := "io.github.mozartdiniz.Figaro"

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

# Install into ~/.local so it shows up in the app grid. The engine bundle
# (castlabs Electron) goes to libexec/figaro-engine/, where the app looks.
install: engine
    cargo install --path . --root {{prefix}}
    rm -rf {{prefix}}/libexec/figaro-engine
    mkdir -p {{prefix}}/libexec
    cp -a engine/bundle {{prefix}}/libexec/figaro-engine
    install -Dm644 data/{{app_id}}.desktop -t {{share}}/applications/
    install -Dm644 data/icons/hicolor/scalable/apps/{{app_id}}.svg -t {{share}}/icons/hicolor/scalable/apps/
    -update-desktop-database {{share}}/applications
    -gtk-update-icon-cache -f -t {{share}}/icons/hicolor

uninstall:
    -cargo uninstall figaro --root {{prefix}}
    rm -rf {{prefix}}/libexec/figaro-engine
    rm -f {{share}}/applications/{{app_id}}.desktop
    rm -f {{share}}/icons/hicolor/scalable/apps/{{app_id}}.svg
    -update-desktop-database {{share}}/applications
