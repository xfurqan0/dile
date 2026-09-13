/*
  The settings window: read the document, draw it, and write it back 400 ms after the last
  keystroke.

  No framework and no build step, which is the same decision the panel's markup carries. The
  window is five groups of form controls over one JSON document; a bundler would be a
  lockfile and two minutes of CI for a file that is easier to read without one.

  **There is no Save button**, and that is the design. A settings window whose changes are
  already in force cannot have an unsaved state, so a button would only be a thing to forget
  to press. Every control writes the whole document, the Rust side works out what actually
  moved, and the "Saved" line is the receipt.

  **Every visible word comes from locales/.** `data-i18n` attributes are resolved once at
  start-up out of the catalogue the `get_strings` command hands over — the same files the
  tray reads — and crates/dile-app/tests/i18n.rs fails the build on a sentence typed in here.

  **The only door out of this window is a command.** There is no plugin API and no global
  Tauri object: `capabilities/settings.json` grants this window nothing beyond the core
  defaults, so everything it can do is one of the nine functions Rust exposes.
*/

const invoke = (command, args) => window.__TAURI_INTERNALS__.invoke(command, args || {});

/** How long after the last keystroke the document is written. */
const SAVE_DELAY = 400;

/** How long the "Saved" line stays up. */
const STATUS_DELAY = 1600;

/** How often the engine group re-reads the engine's state while it is on screen. */
const ENGINE_POLL = 2000;

let strings = {};
let settings = null;
let devices = [];
let section = "hotkey";
let saveTimer = null;
let statusTimer = null;
let enginePoll = null;

/** One string, with its `{placeholders}` filled. A missing key renders as the key. */
function t(key, params) {
  let value = Object.prototype.hasOwnProperty.call(strings, key) ? strings[key] : key;
  if (params) {
    for (const name of Object.keys(params)) {
      value = value.split("{" + name + "}").join(String(params[name]));
    }
  }
  return value;
}

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => Array.from(document.querySelectorAll(selector));
// Descendant selectors are written as a root and a child rather than as one string with a
// space in it: a literal with two words in it is what the hard-coded-text test of
// crates/dile-app/tests/i18n.rs looks for, and a CSS selector is not worth an exception.
const $$in = (root, selector) => (root ? Array.from(root.querySelectorAll(selector)) : []);

/** Every canonical cell of the dictionary table, in row order. */
const canonicalCells = () => $$in($("#dictionary-rows"), '.cell[data-part="canonical"]');

/* ------------------------------------------------------------------------ drawing */

function applyStrings() {
  for (const node of $$("[data-i18n]")) {
    node.textContent = t(node.dataset.i18n);
  }
  $("#dictionary-add").textContent = t("settings.dictionary.add");
}

function drawChord() {
  const keys = $("#chord-keys");
  keys.textContent = "";
  for (const part of settings.hotkey.chord.split("+")) {
    const cap = document.createElement("span");
    cap.className = "kbd";
    cap.textContent = part;
    keys.append(cap);
  }
}

function drawSegmented(id, value) {
  for (const button of $$in($("#" + id), "button")) {
    button.setAttribute("aria-checked", String(button.dataset.value === value));
  }
}

function drawSwitch(id, on) {
  $("#" + id).setAttribute("aria-checked", String(on));
}

function drawCap() {
  $("#cap").value = String(settings.capture.cap_secs);
  $("#cap-value").textContent = t("settings.recording.cap.value", {
    seconds: settings.capture.cap_secs,
  });
}

function drawDevices() {
  const select = $("#device");
  select.textContent = "";

  const fallback = document.createElement("option");
  fallback.value = "";
  fallback.textContent = t("settings.recording.device.default");
  select.append(fallback);

  for (const device of devices) {
    const option = document.createElement("option");
    option.value = device.id;
    option.textContent = device.name;
    select.append(option);
  }

  const wanted = settings.capture.device || "";
  if (wanted && !devices.some((device) => device.id === wanted)) {
    // A microphone that is not plugged in right now. Kept in the list rather than reset to
    // the default, because unplugging a headset must not silently change the setting.
    const missing = document.createElement("option");
    missing.value = wanted;
    missing.textContent = wanted;
    select.append(missing);
  }
  select.value = wanted;
}

function drawDictionary() {
  const body = $("#dictionary-rows");
  body.textContent = "";

  for (const [index, entry] of settings.dictionary.entries()) {
    const row = document.createElement("tr");

    const canonical = document.createElement("td");
    const canonicalInput = document.createElement("input");
    canonicalInput.className = "cell";
    canonicalInput.value = entry.canonical;
    canonicalInput.dataset.index = String(index);
    canonicalInput.dataset.part = "canonical";
    canonicalInput.setAttribute("aria-label", t("settings.dictionary.canonical"));
    canonical.append(canonicalInput);

    const variants = document.createElement("td");
    const variantsInput = document.createElement("input");
    // Two `add` calls rather than one space-separated string: a literal with two words in it
    // is what the hard-coded-text test looks for, and a class list is not worth an exception.
    variantsInput.classList.add("cell", "variants");
    variantsInput.value = entry.variants.join(", ");
    variantsInput.dataset.index = String(index);
    variantsInput.dataset.part = "variants";
    variantsInput.placeholder = t("settings.dictionary.variants.placeholder");
    variantsInput.setAttribute("aria-label", t("settings.dictionary.variants"));
    variants.append(variantsInput);

    const pinned = document.createElement("td");
    const pinnedInput = document.createElement("input");
    pinnedInput.type = "checkbox";
    pinnedInput.checked = Boolean(entry.pinned);
    pinnedInput.dataset.index = String(index);
    pinnedInput.dataset.part = "pinned";
    pinnedInput.setAttribute("aria-label", t("settings.dictionary.pinned"));
    pinned.append(pinnedInput);

    const drop = document.createElement("td");
    const dropButton = document.createElement("button");
    dropButton.type = "button";
    dropButton.className = "drop-row";
    dropButton.textContent = "✕";
    dropButton.dataset.index = String(index);
    dropButton.setAttribute("aria-label", t("settings.dictionary.remove"));
    drop.append(dropButton);

    row.append(canonical, variants, pinned, drop);
    body.append(row);
  }

  const empty = settings.dictionary.length === 0;
  $("#dictionary-empty").hidden = !empty;
  $("#dictionary").hidden = empty;
}

function draw() {
  drawChord();
  drawSegmented("mode", settings.hotkey.mode);
  drawSwitch("second-key", settings.hotkey.second_key);
  drawCap();
  drawDevices();
  drawSegmented("strictness", settings.cleanup.strictness);
  drawSwitch("auto-transfer", settings.cleanup.auto_transfer);
  $("#auto-transfer-ms").value = String(settings.cleanup.auto_transfer_ms);
  $("#tier").value = settings.engine.tier_override || "auto";
  drawSegmented("language", settings.ui.language);
  drawSwitch("autostart", settings.ui.autostart);
  drawDictionary();
}

/* ------------------------------------------------------------------------- saving */

function showStatus(text, bad) {
  const status = $("#status");
  status.textContent = text;
  status.classList.toggle("bad", Boolean(bad));
  status.classList.add("showing");
  window.clearTimeout(statusTimer);
  statusTimer = window.setTimeout(() => status.classList.remove("showing"), STATUS_DELAY);
}

/** Two rows claiming the same term, found where they were typed. */
function markDuplicates() {
  const seen = new Map();
  let duplicate = false;
  for (const input of canonicalCells()) {
    const key = input.value.trim().toLocaleLowerCase();
    const clash = key.length > 0 && seen.has(key);
    input.classList.toggle("bad", clash);
    if (clash) {
      duplicate = true;
    } else if (key.length > 0) {
      seen.set(key, true);
    }
  }
  return duplicate;
}

function schedule() {
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(save, SAVE_DELAY);
}

async function save() {
  if (markDuplicates()) {
    showStatus(t("settings.error.dictionary.duplicate"), true);
    return;
  }

  showStatus(t("settings.state.saving"));
  try {
    const stored = await invoke("set_settings", { settings });
    settings = stored;
    // Only the readouts a clamp could have moved are redrawn: a full redraw would take the
    // caret out of whichever dictionary cell is being typed into.
    drawCap();
    const delay = $("#auto-transfer-ms");
    if (document.activeElement !== delay) {
      delay.value = String(settings.cleanup.auto_transfer_ms);
    }
    showStatus(t("settings.state.saved"));
  } catch (error) {
    console.error(error);
    showStatus(t(error && error.key ? error.key : "settings.error.save"), true);
  }
}

/* ------------------------------------------------------------------------ the engine */

function drawEngine(status) {
  const tier = status.engine.tier || status.decided_tier || "vulkan";
  const model = status.models[tier];
  if (model) {
    let line = t("settings.engine.model.value", { file: model.file, size: model.size_mb });
    if (!model.present) {
      line += " · " + t("settings.engine.model.missing");
    }
    $("#model").textContent = line;
  }

  const state = status.engine.state;
  $("#engine-state").textContent = state
    ? t("settings.engine.state." + state, {
        tier: tier,
        percent: status.engine.progress === null ? 0 : status.engine.progress,
      })
    : "";

  const probe = status.probe;
  $("#probe-line").textContent = probe
    ? t("settings.engine.probe.line", {
        result: t(probe.passed ? "settings.engine.probe.passed" : "settings.engine.probe.failed"),
        device: probe.device || tier,
        load: probe.load_ms,
        probe: probe.probe_ms,
        words: probe.words,
        expected: probe.expected_words,
      })
    : t("settings.engine.probe.none");
}

async function refreshEngine() {
  try {
    drawEngine(await invoke("engine_status"));
  } catch (error) {
    console.error(error);
  }
}

function watchEngine() {
  window.clearInterval(enginePoll);
  if (section !== "engine") {
    return;
  }
  refreshEngine();
  enginePoll = window.setInterval(refreshEngine, ENGINE_POLL);
}

/* ------------------------------------------------------------------------- wiring */

function showSection(wanted) {
  section = wanted;
  for (const item of $$(".rail-item")) {
    item.classList.toggle("on", item.dataset.section === wanted);
  }
  for (const node of $$(".section")) {
    node.hidden = node.dataset.section !== wanted;
  }
  $("#pane").scrollTop = 0;
  watchEngine();
}

function wire() {
  for (const item of $$(".rail-item")) {
    item.addEventListener("click", () => showSection(item.dataset.section));
  }

  $("#chord-change").addEventListener("click", changeChord);

  for (const button of $$in($("#mode"), "button")) {
    button.addEventListener("click", () => {
      settings.hotkey.mode = button.dataset.value;
      drawSegmented("mode", settings.hotkey.mode);
      schedule();
    });
  }

  $("#second-key").addEventListener("click", () => {
    settings.hotkey.second_key = !settings.hotkey.second_key;
    drawSwitch("second-key", settings.hotkey.second_key);
    schedule();
  });

  $("#cap").addEventListener("input", () => {
    settings.capture.cap_secs = Number($("#cap").value);
    drawCap();
    schedule();
  });

  $("#device").addEventListener("change", () => {
    settings.capture.device = $("#device").value || null;
    schedule();
  });

  for (const button of $$in($("#strictness"), "button")) {
    button.addEventListener("click", () => {
      settings.cleanup.strictness = button.dataset.value;
      drawSegmented("strictness", settings.cleanup.strictness);
      schedule();
    });
  }

  $("#auto-transfer").addEventListener("click", () => {
    settings.cleanup.auto_transfer = !settings.cleanup.auto_transfer;
    drawSwitch("auto-transfer", settings.cleanup.auto_transfer);
    schedule();
  });

  $("#auto-transfer-ms").addEventListener("input", () => {
    const typed = Number($("#auto-transfer-ms").value);
    if (Number.isFinite(typed) && typed > 0) {
      settings.cleanup.auto_transfer_ms = typed;
      schedule();
    }
  });

  $("#tier").addEventListener("change", () => {
    const chosen = $("#tier").value;
    settings.engine.tier_override = chosen === "auto" ? null : chosen;
    schedule();
  });

  $("#reprobe").addEventListener("click", async () => {
    try {
      await invoke("rerun_probe");
      refreshEngine();
    } catch (error) {
      console.error(error);
    }
  });

  $("#open-models").addEventListener("click", async () => {
    try {
      await invoke("open_models_dir");
    } catch (error) {
      console.error(error);
      showStatus(t(error && error.key ? error.key : "settings.error.models"), true);
    }
  });

  $("#dictionary-add").addEventListener("click", () => {
    settings.dictionary.push({ canonical: "", variants: [], pinned: false });
    drawDictionary();
    const rows = canonicalCells();
    if (rows.length > 0) {
      rows[rows.length - 1].focus();
    }
  });

  $("#dictionary-rows").addEventListener("input", (event) => {
    const target = event.target;
    const index = Number(target.dataset.index);
    const entry = settings.dictionary[index];
    if (!entry) {
      return;
    }
    if (target.dataset.part === "canonical") {
      entry.canonical = target.value;
    } else if (target.dataset.part === "variants") {
      entry.variants = target.value
        .split(",")
        .map((variant) => variant.trim())
        .filter((variant) => variant.length > 0);
    } else if (target.dataset.part === "pinned") {
      entry.pinned = target.checked;
    }
    // An entry with no term would be dropped on the way in, so it is not sent at all until
    // there is something to send: a row being typed into is not yet a dictionary entry.
    if (settings.dictionary.every((row) => row.canonical.trim().length > 0)) {
      schedule();
    }
  });

  $("#dictionary-rows").addEventListener("click", (event) => {
    const button = event.target.closest(".drop-row");
    if (!button) {
      return;
    }
    settings.dictionary.splice(Number(button.dataset.index), 1);
    drawDictionary();
    save();
  });

  for (const button of $$in($("#language"), "button")) {
    button.addEventListener("click", async () => {
      settings.ui.language = button.dataset.value;
      drawSegmented("language", settings.ui.language);
      await save();
      await reload();
    });
  }

  $("#autostart").addEventListener("click", () => {
    settings.ui.autostart = !settings.ui.autostart;
    drawSwitch("autostart", settings.ui.autostart);
    schedule();
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      invoke("close_settings").catch((error) => console.error(error));
    }
  });
}

async function changeChord() {
  const button = $("#chord-change");
  const label = button.textContent;
  button.textContent = t("settings.hotkey.listening");
  button.disabled = true;
  try {
    settings.hotkey.chord = await invoke("capture_hotkey");
    drawChord();
    await save();
  } catch (error) {
    console.error(error);
    showStatus(t(error && error.key ? error.key : "settings.error.chord.unreadable"), true);
  } finally {
    button.textContent = label;
    button.disabled = false;
  }
}

/* -------------------------------------------------------------------------- start */

/** Re-read the catalogue and repaint. What a language change does to this window. */
async function reload() {
  const catalogue = await invoke("get_strings");
  strings = catalogue.strings;
  document.documentElement.lang = catalogue.locale;
  applyStrings();
  draw();
  refreshEngine();
}

async function boot() {
  try {
    const catalogue = await invoke("get_strings");
    strings = catalogue.strings;
    document.documentElement.lang = catalogue.locale;
    applyStrings();

    settings = await invoke("get_settings");
    try {
      devices = await invoke("list_input_devices");
    } catch (error) {
      // A machine with no audio host still gets a settings window; it simply has one entry
      // in the microphone list.
      console.error(error);
      devices = [];
    }

    draw();
    wire();

    // The group to open on. The Rust side sets it only when something asked for one; every
    // other start lands on the first rail item.
    const wanted = window.__DILE_GROUP__;
    const known = $$(".rail-item").some((item) => item.dataset.section === wanted);
    showSection(known ? wanted : "hotkey");
  } catch (error) {
    console.error(error);
  }
}

boot();
