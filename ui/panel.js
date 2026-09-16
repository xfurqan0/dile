/*
  The review panel: five states, one card, and no framework.

  The window it runs in never takes the keyboard focus — that is the whole point of the panel
  (docs/PROJECT.md §3) — so this script is deliberately not where Enter, Esc and Ctrl+C are
  decided. Those three are read from the global hook by `dile-hotkey`, turned into actions by
  the session, and arrive here only as their consequences. What this file owns is drawing:
  the level meter, the elapsed clock, the countdown line, the pills, and the one editable
  region on the page.

  **Every visible word comes from locales/.** `data-i18n` attributes are resolved once at
  start-up out of the catalogue `get_strings` hands over — the same files the tray reads — and
  crates/dile-app/tests/i18n.rs fails the build on a sentence typed in here.

  **Nothing sets a style.** The content security policy is `style-src 'self'`, so the moving
  parts are SVG geometry attributes rather than CSS: twelve `<rect>` heights for the meter and
  one `<rect>` width for the countdown. Everything else is a class or the `hidden` attribute.

  **The only door out of this window is a command.** `capabilities/default.json` grants the
  page `core:window:allow-hide` and `core:window:allow-start-dragging` and nothing else that
  moves a window: the card is placed, resized and focused by Rust, which owns the
  non-activating window style the whole design rests on.
*/

const invoke = (command, args) => window.__TAURI_INTERNALS__.invoke(command, args || {});

/** Subscribe to one of the application's events. */
function listen(event, handler) {
  const id = window.__TAURI_INTERNALS__.transformCallback((message) => handler(message.payload));
  return invoke("plugin:event|listen", { event, target: { kind: "Any" }, handler: id });
}

/** How many bars the level meter has, and the geometry they are drawn in. */
const BARS = 12;
const BAR_PITCH = 5;
const BAR_WIDTH = 3;
const METER_HEIGHT = 22;

/** The quietest level the meter bothers to draw, in dBFS. */
const FLOOR_DBFS = -60;

/** How long the idle hint and the "nothing heard" line stay up, in milliseconds. */
const HINT_MS = 1000;
const NOTHING_MS = 1200;

/** How long "copied — press Ctrl+V" stays up before the card goes away, in milliseconds.
    Longer than the other two: it is an instruction rather than an acknowledgement, and a
    person who has just finished speaking has not been looking at the screen. */
const COPIED_MS = 2600;

/** How long the pointer has to rest on the text before the countdown gives up. */
const HOVER_MS = 300;

/** How long after the last measurement the card asks Rust to resize it. */
const RESIZE_DELAY = 60;

/** How long after the last keystroke the edited text is sent to Rust. */
const EDIT_DELAY = 120;

let strings = {};
let context = {
  hotkey: "",
  auto_transfer: true,
  auto_transfer_ms: 2500,
  cap_ms: 60000,
  strictness: "medium",
  target_label: null,
  preview: false,
};

let mode = "closed";
let meterLevels = [];
let capMs = 60000;
let audioMs = 0;
let workingSince = 0;
let workingTimer = null;
let closeTimer = null;
let hoverTimer = null;
let editTimer = null;
let resizeTimer = null;
let countdownFrame = 0;
let engine = { model: null, tier: null };

let rawText = "";
let cleanedText = "";
let showingRaw = false;

const $ = (id) => document.getElementById(id);
const card = $("card");
const title = $("title");
const sub = $("sub");
const stack = $("stack");
const text = $("text");
const meter = $("meter");
const elapsed = $("elapsed");
const target = $("target");
const cancelLive = $("cancel-live");
const foot = $("foot");
const levels = $("levels");
const rawPill = $("raw");
const countdownFill = $("countdown-fill");

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

function applyStrings() {
  for (const node of document.querySelectorAll("[data-i18n]")) {
    node.textContent = t(node.dataset.i18n);
  }
}

/* ---------------------------------------------------------------------------- drawing */

function buildMeter() {
  const ns = "http://www.w3.org/2000/svg";
  for (let index = 0; index < BARS; index += 1) {
    const bar = document.createElementNS(ns, "rect");
    bar.setAttribute("x", String(index * BAR_PITCH));
    bar.setAttribute("width", String(BAR_WIDTH));
    bar.setAttribute("rx", String(BAR_WIDTH / 2));
    bar.setAttribute("y", String(METER_HEIGHT - 2));
    bar.setAttribute("height", "2");
    meter.append(bar);
  }
}

function drawMeter() {
  const bars = meter.children;
  for (let index = 0; index < bars.length; index += 1) {
    // The ring is drawn oldest-first, so the bars read left to right the way a waveform does.
    const level = meterLevels[index] === undefined ? 0 : meterLevels[index];
    const height = Math.max(2, Math.round(level * METER_HEIGHT));
    bars[index].setAttribute("height", String(height));
    bars[index].setAttribute("y", String(METER_HEIGHT - height));
  }
}

function clock(ms) {
  const total = Math.max(0, Math.floor(ms / 1000));
  const minutes = String(Math.floor(total / 60)).padStart(2, "0");
  const seconds = String(total % 60).padStart(2, "0");
  return minutes + ":" + seconds;
}

function seconds(ms) {
  return String(Math.round(ms / 1000)) + " " + t("panel.unit.seconds");
}

/* ----------------------------------------------------------------------- the countdown */

function stopCountdown() {
  if (countdownFrame) {
    cancelAnimationFrame(countdownFrame);
    countdownFrame = 0;
  }
  countdownFill.setAttribute("width", "0");
}

function startCountdown() {
  stopCountdown();
  if (!context.auto_transfer) {
    return;
  }
  const total = Math.max(1, context.auto_transfer_ms);
  const from = performance.now();
  const step = (now) => {
    const left = Math.max(0, total - (now - from));
    countdownFill.setAttribute("width", String((left / total) * 100));
    if (left <= 0) {
      countdownFrame = 0;
      doTransfer();
      return;
    }
    countdownFrame = requestAnimationFrame(step);
  };
  countdownFrame = requestAnimationFrame(step);
}

/* --------------------------------------------------------------------------- the states */

function clearTimers() {
  stopCountdown();
  clearTimeout(closeTimer);
  clearTimeout(hoverTimer);
  clearInterval(workingTimer);
  workingTimer = null;
}

/** Show one state, with everything that does not belong to it hidden. */
function show(state) {
  mode = state;
  card.dataset.state = state;

  const live = state === "recording" || state === "working";
  const result = state === "result";

  stack.hidden = result;
  text.hidden = !result;
  meter.hidden = state !== "recording";
  elapsed.hidden = !live;
  target.hidden = !result;
  cancelLive.hidden = !live;
  foot.hidden = !result;
}

function notice(titleKey, hintKey, state) {
  clearTimers();
  show(state);
  title.textContent = t(titleKey);
  sub.textContent = hintKey ? t(hintKey) : "";
  sub.hidden = !hintKey;
  elapsed.textContent = "";
}

function renderIdle(hotkey) {
  notice("panel.hint.idle", null, "idle");
  title.textContent = t("panel.hint.idle", { hotkey: hotkey || context.hotkey });
  closeLater(HINT_MS);
}

/** Close the panel after a moment, unless this card is a preview somebody is looking at. */
function closeLater(delay) {
  if (context.preview) {
    return;
  }
  closeTimer = setTimeout(() => invoke("panel_close"), delay);
}

function renderRecording() {
  clearTimers();
  show("recording");
  title.textContent = t("panel.state.recording");
  sub.hidden = false;
  sub.textContent = t("panel.state.recording.hint");
  meterLevels = [];
  audioMs = 0;
  drawMeter();
  elapsed.textContent = t("panel.recording.elapsed", {
    elapsed: clock(0),
    cap: seconds(capMs),
  });
}

function renderWorking() {
  clearTimers();
  show("working");
  title.textContent = t("panel.state.processing");
  sub.hidden = false;
  sub.textContent = t("panel.processing.detail", {
    // An em dash rather than a label: until the engine has said which model it loaded there
    // is nothing honest to put here, and a word that means "unknown" would be read as one.
    model: engine.model || "—",
    tier: engine.tier || "—",
    audio: seconds(audioMs),
  });
  workingSince = performance.now();
  const tick = () => {
    elapsed.textContent = t("panel.processing.elapsed", {
      elapsed: ((performance.now() - workingSince) / 1000).toFixed(1),
    });
  };
  tick();
  workingTimer = setInterval(tick, 100);
}

function renderResult(payload) {
  clearTimers();
  show("result");
  rawText = payload.raw || "";
  cleanedText = payload.cleaned || "";
  showingRaw = false;
  rawPill.setAttribute("aria-pressed", "false");
  text.setAttribute("contenteditable", "plaintext-only");
  text.textContent = cleanedText;
  target.textContent = payload.target_label ? t("panel.target", { app: payload.target_label }) : "";
  target.hidden = !payload.target_label;
  // The corner is where "copied" and "the clipboard refused" land too, so a new dictation
  // takes the colour off it rather than inheriting the last one's verdict.
  delete target.dataset.state;
  card.removeAttribute("title");
  drawLevels(payload.strictness || context.strictness);
  startCountdown();
}

function drawLevels(level) {
  for (const pill of levels.children) {
    pill.setAttribute("aria-checked", String(pill.dataset.level === level));
  }
}

/* ------------------------------------------------------------------------- the actions */

/** What a transfer or a copy would actually send. */
function body() {
  return showingRaw ? cleanedText : text.textContent;
}

function doTransfer() {
  stopCountdown();
  invoke("panel_transfer", { text: body() });
}

function doCopy() {
  stopCountdown();
  invoke("panel_copy", { text: body() });
}

function reclean(level) {
  stopCountdown();
  invoke("panel_reclean", { raw: rawText, level }).then((cleaned) => {
    cleanedText = cleaned;
    drawLevels(level);
    if (!showingRaw) {
      text.textContent = cleaned;
    }
    // The user has just been handed a different sentence to read, so the window they have to
    // read it in starts again rather than finishing from where it was.
    startCountdown();
  });
}

function toggleRaw() {
  if (!showingRaw) {
    // Whatever is in the box is the answer that gets transferred, edits included, so it is
    // kept before the raw transcript takes its place.
    cleanedText = text.textContent;
  }
  showingRaw = !showingRaw;
  rawPill.setAttribute("aria-pressed", String(showingRaw));
  text.setAttribute("contenteditable", showingRaw ? "false" : "plaintext-only");
  text.textContent = showingRaw ? rawText : cleanedText;
}

/* ---------------------------------------------------------------------------- the wiring */

function wire() {
  $("transfer").addEventListener("click", doTransfer);
  $("copy").addEventListener("click", doCopy);
  $("cancel").addEventListener("click", () => invoke("panel_cancel"));
  cancelLive.addEventListener("click", () => invoke("panel_cancel"));
  $("rerecord").addEventListener("click", () => invoke("panel_rerecord"));
  rawPill.addEventListener("click", toggleRaw);

  for (const pill of levels.children) {
    pill.addEventListener("click", () => reclean(pill.dataset.level));
  }

  // Clicking into the text is the one action that activates this window.
  text.addEventListener("mousedown", () => {
    invoke("panel_take_focus").then(() => text.focus());
  });
  text.addEventListener("blur", () => invoke("panel_release_focus"));

  // Any edit stops the countdown: a person who has started fixing a word has not finished
  // reading the sentence.
  text.addEventListener("input", () => {
    stopCountdown();
    clearTimeout(editTimer);
    editTimer = setTimeout(() => invoke("panel_edited", { text: text.textContent }), EDIT_DELAY);
  });

  // And so does resting the pointer on it, which is what somebody does while they read.
  text.addEventListener("mouseenter", () => {
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(stopCountdown, HOVER_MS);
  });
  text.addEventListener("mouseleave", () => clearTimeout(hoverTimer));

  // The three keys are swallowed by the global hook and come back as actions, so these
  // handlers only ever fire on a machine where the hook could not be installed blocking.
  // Shift+Enter is never swallowed, and it is the newline.
  text.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      doTransfer();
    } else if (event.key === "Escape") {
      event.preventDefault();
      invoke("panel_cancel");
    }
  });

  // The card is draggable by its background. Not by its buttons, and not by the transcript,
  // where a drag is a text selection.
  card.addEventListener("mousedown", (event) => {
    // One selector per call rather than one string with commas in it: a literal with two
    // words in it is what the hard-coded-text test of crates/dile-app/tests/i18n.rs looks
    // for, and a CSS selector is not worth an exception to that rule.
    const hit = event.target;
    const onControl = hit.closest(".action") || hit.closest(".pill") || hit.closest(".text");
    if (event.button !== 0 || onControl) {
      return;
    }
    invoke("plugin:window|start_dragging");
  });

  // The height the result state needs is measured here and applied there: the panel's
  // capability deliberately does not include `core:window:allow-set-size`.
  const observer = new ResizeObserver(() => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => invoke("panel_resize", { height: card.offsetHeight }), RESIZE_DELAY);
  });
  observer.observe(card);
}

/* ----------------------------------------------------------------------------- the feed */

function onState(payload) {
  const state = payload.state;
  if (state === "recording") {
    renderRecording();
    return;
  }
  // Everything below only means something while the panel is already up. The resting state
  // is emitted every time a dictation ends, and a card that reappeared for it would be a card
  // that never went away.
  if (mode === "closed" || mode === "result" || mode === "gone" || mode === "copied") {
    return;
  }
  // "live" is the panel saying it is open before any state has arrived; everything below is
  // a state that only means something once it is.

  if (state === "working") {
    renderWorking();
  } else if (state === "nothing-heard") {
    notice("panel.state.nothing", null, "stalled");
    closeLater(NOTHING_MS);
  } else if (state === "no-microphone") {
    notice("panel.state.nomicrophone", null, "stalled");
  } else if (state === "no-model") {
    notice("panel.state.nomodel", "panel.hint.engine", "stalled");
  } else if (state === "engine-failed") {
    notice("panel.state.noengine", "panel.hint.engine", "stalled");
  }
}

function onLevel(payload) {
  capMs = payload.cap_ms || capMs;
  if (mode !== "recording") {
    return;
  }
  audioMs = payload.elapsed_ms;
  const level = Math.min(1, Math.max(0, (payload.rms_dbfs - FLOOR_DBFS) / -FLOOR_DBFS));
  meterLevels.push(level);
  while (meterLevels.length > BARS) {
    meterLevels.shift();
  }
  drawMeter();
  elapsed.textContent = t("panel.recording.elapsed", {
    elapsed: clock(payload.elapsed_ms),
    cap: seconds(capMs),
  });
}

function onEngine(payload) {
  engine = { model: payload.model || null, tier: payload.tier || null };
}

function onPanel(payload) {
  if (payload.mode === "live") {
    refresh();
    if (mode === "closed") {
      mode = "live";
    }
  } else if (payload.mode === "idle") {
    refresh().then(() => renderIdle(payload.hotkey));
  } else if (payload.mode === "closed") {
    clearTimers();
    mode = "closed";
  } else if (payload.mode === "copied") {
    // The dictation is on the clipboard, which on this platform is the whole hand-over. The
    // result layout does not move at all — the corner that would have said where the text was
    // going says where it is instead, and the text stays selectable underneath it while the
    // card is up, so a failure a moment later still leaves something to take by hand.
    stopCountdown();
    clearTimeout(closeTimer);
    mode = "copied";
    target.hidden = false;
    target.dataset.state = "copied";
    target.textContent = t("panel.state.copied");
    closeLater(COPIED_MS);
  } else if (payload.mode === "clipboard-failed") {
    // The one outcome that must never take the dictation with it: the countdown stops, the
    // card stays, and the hint says to take the text out by hand.
    stopCountdown();
    clearTimeout(closeTimer);
    mode = "gone";
    card.dataset.state = "gone";
    card.title = t("panel.hint.clipboardfailed");
    target.hidden = false;
    target.dataset.state = "failed";
    target.textContent = t("panel.state.clipboardfailed");
  } else if (payload.mode === "target-gone") {
    // The text is still there and still worth keeping, so the result layout stays exactly
    // as it was and only the label changes: the corner that said where this was going now
    // says that the place is gone, which is the one thing that moved. Copy and cancel are
    // both still under the pointer, and the hint for them is on the card.
    stopCountdown();
    clearTimeout(closeTimer);
    mode = "gone";
    card.dataset.state = "gone";
    card.title = t("panel.hint.gone");
    target.hidden = false;
    target.textContent = t("panel.state.gone");
  }
}

function onResult(payload) {
  refresh().then(() => renderResult(payload));
}

/** Re-read the settings the panel draws itself with. */
function refresh() {
  return invoke("panel_context").then((next) => {
    context = next;
    capMs = next.cap_ms || capMs;
  });
}

/* ------------------------------------------------------------------------------- start */

async function boot() {
  const catalogue = await invoke("get_strings");
  strings = catalogue.strings;
  document.documentElement.lang = catalogue.locale;
  applyStrings();

  await refresh();
  buildMeter();
  drawMeter();
  stopCountdown();
  wire();

  await listen("dile://state", onState);
  await listen("dile://level", onLevel);
  await listen("dile://engine", onEngine);
  await listen("dile://result", onResult);
  await listen("dile://panel", onPanel);
}

boot().catch((error) => console.error(error));
