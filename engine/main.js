// figaro-engine: one hidden window running web/player.html (MusicKit JS).
//
// castlabs' Electron build, because Apple Music streams AAC under Widevine:
// it ships Chromium's AAC decoder and gets the Widevine CDM through
// castlabs' licensed component updater.
//
// Protocol: JSON lines, commands on stdin, events on stdout. See
// docs/ENGINE_PROTOCOL.md.

const { app, components, BrowserWindow } = require("electron");
const fs = require("fs");
const path = require("path");
const readline = require("readline");

// The web player token only works from apple.com origins. The page is a
// data URL with this base, so it gets that origin (a secure context, which
// EME needs) without intercepting any of Apple's real requests.
const PAGE_URL = "https://music.apple.com/figaro-engine";
const PAGE = fs.readFileSync(path.join(__dirname, "web", "player.html"));

const show = process.argv.includes("--show");
let main = null;
let pageReady = false;
let pending = [];
let quitting = false;

function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

function profileDir() {
  if (process.env.FIGARO_ENGINE_PROFILE) return process.env.FIGARO_ENGINE_PROFILE;
  const data = process.env.XDG_DATA_HOME || path.join(process.env.HOME || "/tmp", ".local", "share");
  return path.join(data, "figaro", "electron");
}

function runInPage(cmd) {
  // Re-serialised JSON is a valid JS expression: no injection.
  const js = `figaro.dispatch(${JSON.stringify(cmd)})`;
  if (!pageReady) pending.push(js);
  else main.webContents.executeJavaScript(js).catch((e) => emit({ event: "error", message: String(e) }));
}

function handleCommand(line) {
  let cmd;
  try {
    cmd = JSON.parse(line);
  } catch {
    return emit({ event: "error", message: `bad command: ${line}` });
  }
  switch (cmd.cmd) {
    case "quit": return quit();
    case "show": main.show(); main.focus(); return;
    case "hide": main.hide(); return;
    case undefined: return emit({ event: "error", message: `command without cmd: ${line}` });
    default: return runInPage(cmd);
  }
}

function onPageMessage(json) {
  if (json.includes('"event":"loaded"')) {
    pageReady = true;
    for (const js of pending) main.webContents.executeJavaScript(js);
    pending = [];
  }
  process.stdout.write(json + "\n");
}

function quit() {
  quitting = true;
  app.quit();
}

app.setPath("userData", profileDir());
// The window is hidden, so there is never a user gesture.
app.commandLine.appendSwitch("autoplay-policy", "no-user-gesture-required");
// No GPU needed for an audio page and a sign-in form. On the dev laptop
// (Fedora, GNOME) the GPU process segfaulted and windows stayed blank.
app.disableHardwareAcceleration();

app.on("window-all-closed", quit);

// With --show, log every page's loads and errors (the sign-in popup too) to
// stderr, which the app passes through to its terminal.
if (show) {
  app.on("web-contents-created", (_e, wc) => {
    const log = (...a) => console.error("[engine]", ...a);
    wc.on("did-navigate", (_e, url) => log("navigated", url.slice(0, 100)));
    wc.on("did-fail-load", (_e, code, text, url) => log("load failed", code, text, url.slice(0, 100)));
    wc.on("render-process-gone", (_e, d) => log("renderer gone", JSON.stringify(d)));
    wc.on("console-message", (e) => {
      if (!e.message.startsWith("FIGARO:") && e.level === "error") log("console", e.message.slice(0, 300));
    });
  });
}

app.whenReady().then(async () => {
  try {
    await components.whenReady();
  } catch (e) {
    emit({ event: "error", message: `Widevine CDM unavailable: ${e}` });
  }

  main = new BrowserWindow({
    width: 520,
    height: 640,
    show,
    title: "Figaro Engine",
    webPreferences: { backgroundThrottling: false },
  });
  main.setMenuBarVisibility(false);
  // The player window hides instead of closing.
  main.on("close", (e) => {
    if (!quitting) {
      e.preventDefault();
      main.hide();
    }
  });

  // MusicKit's `authorize()` opens a popup to Apple's sign-in page. That
  // one the user must see. Electron's default popup options: overriding
  // them (show/autoHideMenuBar) left the popup blank on GNOME Wayland.
  // Popups would inherit backgroundThrottling: false; keep the default.
  main.webContents.setWindowOpenHandler(() => ({
    action: "allow",
    overrideBrowserWindowOptions: { webPreferences: { backgroundThrottling: true } },
  }));

  main.webContents.on("console-message", (e) => {
    if (e.message.startsWith("FIGARO:")) onPageMessage(e.message.slice(7));
  });
  main.webContents.on("did-fail-load", (_e, code, text, url, isMainFrame) => {
    if (isMainFrame && code !== -3 /* ERR_ABORTED */) {
      emit({ event: "error", message: `could not load ${url}: ${text}` });
    }
  });

  main.loadURL(`data:text/html;base64,${PAGE.toString("base64")}`, { baseURLForDataURL: PAGE_URL });

  const rl = readline.createInterface({ input: process.stdin });
  rl.on("line", (line) => line.trim() && handleCommand(line));
  rl.on("close", quit);
});
