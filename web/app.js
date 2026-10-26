(() => {
  "use strict";

  // --- Helpers ---------------------------------------------------------------
  const $ = (id) => document.getElementById(id);

  /** A DOM element. `text` sets textContent; `on<event>` adds a listener. */
  function el(tag, attrs, ...kids) {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === "class") e.className = v;
      else if (k === "text") e.textContent = v;
      else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
      else e.setAttribute(k, v === true ? "" : String(v));
    }
    for (const k of kids.flat()) if (k != null && k !== false) e.append(k);
    return e;
  }

  // Static icon markup (paths from the design), never built from server data.
  const PATHS = {
    check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
    clock: '<circle cx="12" cy="13" r="8"/><path d="M12 9v4l2.5 2.5M9.5 2.5h5"/>',
    alert: '<circle cx="12" cy="12" r="9"/><path d="M12 7.5v5.5M12 16.5v.5"/>',
    edit: '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>',
    trash: '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6"/>',
    arrow: '<path d="M9 5l7 7-7 7"/>',
    cloud: '<path d="M7 18a4 4 0 0 1-.6-7.95A6 6 0 0 1 18 9.5a3.5 3.5 0 0 1-.5 7H7z"/>',
    back: '<path d="M15 6l-6 6 6 6"/>',
    cross: '<path d="M6 6l12 12M18 6L6 18"/>',
    warn: '<path d="M12 3.5L2.5 20h19L12 3.5z"/><path d="M12 10v4.5M12 17.4v.1"/>',
    stop: '<circle cx="12" cy="12" r="9"/><path d="M5.6 5.6l12.8 12.8"/>',
    retry: '<path d="M20 12a8 8 0 1 1-2.5-5.8M20 4v5h-5"/>',
    timer: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
    grinder: '<path d="M7 3h10l-1.5 5h-7z"/><rect x="6" y="8" width="12" height="9" rx="2"/><path d="M10 17v3h4v-3"/>',
    unlink: '<path d="M9.5 14.5l-2 2a3.5 3.5 0 0 1-5-5l2-2M14.5 9.5l2-2a3.5 3.5 0 0 1 5 5l-2 2M8 3v3M3 8h3M16 21v-3M21 16h-3"/>',
    down: '<path d="M6 9l6 6 6-6"/>',
  };
  function icon(name, size, width) {
    const t = document.createElement("template");
    t.innerHTML = '<svg width="' + size + '" height="' + size + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="'
      + (width || 2) + '" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + PATHS[name] + "</svg>";
    return t.content.firstChild;
  }
  function kebab() {
    const t = document.createElement("template");
    t.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><circle cx="12" cy="5.5" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="12" cy="18.5" r="1.8"/></svg>';
    return t.content.firstChild;
  }

  const store = {
    get(k) { try { return localStorage.getItem("gbs." + k); } catch (_) { return null; } },
    set(k, v) { try { localStorage.setItem("gbs." + k, v); } catch (_) {} },
  };

  // --- Texts ------------------------------------------------------------------
  // Every text the app writes itself, in English and German. Keys are stable
  // and dotted by place; {name} is filled in by t(). An object with one/other
  // is a plural, picked by the number `n`. Keys ending in .e64 / .wifi are
  // the wording for an E64 WS or an E65/E80 GbS (Local WiFi), picked by tv().
  // Every key is in `en`, and `de` has the same keys, with one exception:
  // texts an integration sends in English, its summary and form
  // (kind.<id>.summary, kind.<id>.field.<key>.label|help|placeholder) and its
  // keyed status lines (integ.detail.<detail_key>), are only in `de`. English,
  // and any key missing here (a new integration), shows the server's text
  // (serverText(), integDetail()). Other texts from the server (integration
  // names, errors, log lines) are shown as they come; the copied or
  // downloaded log stays English.
  //
  // The catalogs are web/strings/<lang>.json, one per language in LANGS in
  // control_api.rs. The server fills them in here when it starts, so they
  // come with the page.
  const STRINGS = {/* web/strings/<lang>.json */};

  // Language: the choice in Status → Settings (this device), else the
  // browser's: the first of its languages that is English or German.
  const LANGS = Object.keys(STRINGS);
  const langChoice = () => { const v = store.get("lang"); return LANGS.includes(v) ? v : "auto"; };
  function browserLangs() {
    const list = navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language || "en"];
    return list.map((l) => String(l));
  }
  function resolveLang() {
    const c = langChoice();
    if (c !== "auto") return c;
    for (const l of browserLangs()) {
      const base = l.toLowerCase().split("-")[0];
      if (LANGS.includes(base)) return base;
    }
    return "en";
  }
  let lang = "en", locale = "en-GB", fmt = null;
  /** Intl formatters for the language, in the browser's own variant of it
   *  when it has one (de-CH, en-US, ...). */
  function setLanguage() {
    lang = resolveLang();
    locale = browserLangs().find((l) => l.toLowerCase().split("-")[0] === lang) || (lang === "de" ? "de-DE" : "en-GB");
    try { new Intl.NumberFormat(locale); } catch (_) { locale = lang; }
    const nf = (o) => new Intl.NumberFormat(locale, o);
    const df = (o) => new Intl.DateTimeFormat(locale, o);
    // 09:47 in 24-hour locales, 9:47 AM (not 09:47 AM) in 12-hour ones.
    const h12 = /^h1[12]$/.test(df({ hour: "numeric" }).resolvedOptions().hourCycle || "");
    const hourStyle = h12 ? "numeric" : "2-digit";
    fmt = {
      one: nf({ minimumFractionDigits: 1, maximumFractionDigits: 1, useGrouping: false }),
      upToOne: nf({ maximumFractionDigits: 1, useGrouping: false }),
      int: nf({ maximumFractionDigits: 0 }),
      hm: df({ hour: hourStyle, minute: "2-digit" }),
      hms: df({ hour: hourStyle, minute: "2-digit", second: "2-digit" }),
      day: df({ day: "numeric", month: "short" }),
      dayYear: df({ day: "numeric", month: "short", year: "numeric" }),
      weekday: df({ weekday: "long" }),
      plural: new Intl.PluralRules(locale),
      list: Intl.ListFormat ? new Intl.ListFormat(locale, { style: "long", type: "conjunction" }) : null,
    };
    document.documentElement.lang = lang;
  }

  const warned = new Set();
  function warnOnce(msg) { if (!warned.has(msg)) { warned.add(msg); console.warn(msg); } }
  const hasText = (key) => Object.prototype.hasOwnProperty.call(STRINGS.en, key);
  /** The text for `key` in the current language, with {name} filled from
   *  `vars` (numbers in its number format). Falls back to English. */
  function t(key, vars) {
    let v = STRINGS[lang] && STRINGS[lang][key];
    if (v === undefined) {
      if (lang !== "en") warnOnce("gbs: no " + lang + " text for " + key + ", using English");
      v = STRINGS.en[key];
    }
    if (v === undefined) { warnOnce("gbs: no text for " + key); return key; }
    if (typeof v === "object") {
      const n = vars && typeof vars.n === "number" ? vars.n : 0;
      v = v[fmt.plural.select(n)] || v.other;
    }
    return fill(v, vars);
  }
  /** `text` with {name} filled from `vars`; unknown names stay as they are.
   *  A number in `vars` is a count (grouped: 1,000 / 1.000); format
   *  measurements such as 28.5 s with secs()/grams() before passing them. */
  function fill(text, vars) {
    return text.replace(/\{(\w+)\}/g, (all, name) => {
      if (!vars || !(name in vars)) return all;
      const x = vars[name];
      if (x == null) return "";
      return typeof x === "number" ? fmt.int.format(x) : String(x);
    });
  }
  /** A text the server sends in English (see STRINGS): the catalog's
   *  wording for `key` in the current language, else `english`. */
  function serverText(key, english, vars) {
    const v = STRINGS[lang] && STRINGS[lang][key];
    return typeof v === "string" ? fill(v, vars) : english;
  }
  /** The grinder-family wording: `key.e64` or `key.wifi` when the catalog has
   *  it, else `key`. */
  function tv(key, vars) {
    const k = key + (wifiSetup() ? ".wifi" : ".e64");
    return t(hasText(k) ? k : key, vars);
  }
  /** "A, B and C" in the current language. */
  function joinList(items) {
    if (fmt.list) return fmt.list.format(items);
    return items.length <= 1 ? items.join("") : items.slice(0, -1).join(", ") + " " + t("common.and") + " " + items[items.length - 1];
  }

  /** Static texts: data-i18n sets the text, data-i18n-attr="attr:key;…" attributes
   *  (only these, so a typo cannot set anything else). */
  const I18N_ATTRS = ["aria-label", "title", "placeholder"];
  function applyStatic(root) {
    for (const e of (root || document).querySelectorAll("[data-i18n]")) e.textContent = t(e.dataset.i18n);
    for (const e of (root || document).querySelectorAll("[data-st]")) e.textContent = serverText(e.dataset.st, e.dataset.stEn);
    for (const e of (root || document).querySelectorAll("[data-st-placeholder]")) e.placeholder = serverText(e.dataset.stPlaceholder, e.dataset.stPlaceholderEn);
    for (const e of (root || document).querySelectorAll("[data-i18n-attr]")) {
      for (const pair of e.dataset.i18nAttr.split(";")) {
        const [attr, key] = pair.split(":").map((x) => (x || "").trim());
        if (I18N_ATTRS.includes(attr) && key) e.setAttribute(attr, t(key));
      }
    }
  }
  /** An element whose text follows the language (it carries its key). */
  const tEl = (tag, attrs, key) => el(tag, Object.assign({ "data-i18n": key, text: t(key) }, attrs || {}));
  /** Sets a server text on `e` (see serverText); `e` keeps the key and the
   *  English, so a language switch redoes it. */
  function setServerText(e, key, english) {
    e.dataset.st = key;
    e.dataset.stEn = english || "";
    e.textContent = serverText(key, english || "");
    return e;
  }
  const stEl = (tag, attrs, key, english) => setServerText(el(tag, attrs), key, english);
  /** The catalog key of an integration kind's text: kindKey("la_marzocco",
   *  "field.username.label"). */
  const kindKey = (kindId, part) => "kind." + kindId + "." + part;

  const cap = (s) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : "");
  const sentence = (s) => { s = cap(String(s || "").trim()); return s && !/[.!?]$/.test(s) ? s + "." : s; };
  const pad2 = (n) => String(n).padStart(2, "0");
  const num = (v) => { const n = parseFloat(String(v).replace(",", ".")); return isFinite(n) ? n : NaN; };
  /** 28.5 (28,5 in German): one decimal. */
  const secs = (s) => fmt.one.format(Math.round(s * 10) / 10);
  /** A measured weight: 39.5. */
  const grams = (g) => fmt.one.format(Math.round(g * 10) / 10);
  /** A recipe weight: 40, 36.5. */
  const gramsShort = (g) => fmt.upToOne.format(Math.round(g * 10) / 10);
  /** m:ss from milliseconds. */
  function clock(ms) {
    const s = Math.max(0, Math.round(ms / 1000));
    return Math.floor(s / 60) + ":" + pad2(s % 60);
  }
  /** m:ss from milliseconds, rounding down (a running timer). */
  function clockDown(ms) {
    const s = Math.max(0, Math.floor(ms / 1000));
    return Math.floor(s / 60) + ":" + pad2(s % 60);
  }
  /** Clock times in the language's format. */
  const hhmm = (d) => fmt.hm.format(d);
  const hhmmss = (d) => fmt.hms.format(d);
  function ago(ms) {
    const s = Math.max(0, Math.round(ms / 1000));
    if (s < 60) return t("ago.s", { n: s });
    const m = Math.round(s / 60);
    if (m < 60) return t("ago.min", { n: m });
    const h = Math.round(m / 60);
    if (h < 48) return t("ago.h", { n: h });
    return t("ago.d", { n: Math.round(h / 24) });
  }
  /** An age in the steps `age_of` in la_marzocco/run.rs uses for its status
   *  line (whole seconds below 2 min, minutes below 2 h, hours below 48 h,
   *  then days, each rounded down), so the line shows the same number in
   *  every language. */
  function ageOf(ms) {
    const s = Math.floor(Math.max(0, ms) / 1000);
    if (s < 120) return t("ago.s", { n: s });
    if (s < 7200) return t("ago.min", { n: Math.floor(s / 60) });
    if (s < 48 * 3600) return t("ago.h", { n: Math.floor(s / 3600) });
    return t("ago.d", { n: Math.floor(s / 86400) });
  }
  const reducedMotion = () => !!(window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches);

  async function api(path, opts) {
    const init = Object.assign({ cache: "no-store" }, opts || {});
    if (init.body) init.headers = { "content-type": "application/json" };
    const r = await fetch(path, init);
    let body = null;
    try { body = await r.json(); } catch (_) {}
    if (!r.ok) {
      throw new Error(body && body.error ? sentence(body.error) : t("common.requestFailed", { status: String(r.status) }));
    }
    return body == null ? {} : body;
  }

  // --- App state ---------------------------------------------------------------
  const FRESH_MS = 10 * 60000;   // a finished shot stays on the Start tab this long
  const ON_TARGET_S = 1.0;       // within this of the recipe time counts as on target
  const KNOB_MS = 5 * 60000;     // E64: how long a grind shows "press the knob"

  let S = null;                  // GET /api/state
  let stateAt = 0;               // performance.now() when S arrived
  let online = true;
  let offlineSince = 0;          // Date.now() when gbs-anywhere stopped answering
  let events = [], lastSeq = 0, primed = false;
  const seenAt = new Map();      // event seq -> when it happened, on this device's clock
  let hist = null;               // GET /api/history, or null
  let histState = "unknown";     // "unknown" | "ok" | "failed"
  let kinds = null, kindsPersistent = false, kindsLoading = false;
  let tab = "start";
  let sseOpen = false;
  const ui = {
    open: 1, planKey: null, info: false, errInfo: false, dismissed: 0,
    // System status: open or closed (sysOpen), as last set outside a stop
    // (sysPref), and the stop it opened for by itself (sysStop, see STOPS).
    sysOpen: false, sysPref: false, sysStop: null,
    pill: null,                  // the pill's label and short label, for a resize
    // Shot events this page reported by hand, and the event numbers the one
    // just sent falls between until its event is in.
    lastScenario: null, mine: new Set(), mineRange: null,
    // The running brew (its start event), whether step 2 shows the fields
    // for it, and whether no integration reported it in time.
    brew: null, typeIn: false, missed: false,
  };

  const wifiOf = () => (S && S.grinder && S.grinder.wifi) || null;
  /** How long the link `w` has been in its state, from the server's
   *  state_age_ms, as of now. */
  const linkStateAge = (w) => (w.state_age_ms == null ? 0 : w.state_age_ms + (performance.now() - stateAt));
  /** How long the link leaves the grinder alone after an unanswered check
   *  (quiet_ms), in whole minutes, at least 1. */
  const quietMinutes = (w) => Math.max(1, Math.round(((w && w.quiet_ms) || 0) / 60000));
  function linkOn() {
    const age = S && S.grinder ? S.grinder.last_poll_age_ms : null;
    return age != null && age < 6000;
  }
  const windowS = () => (S && S.settings && S.settings.shot_window_s) || 0;
  const windowLabel = () => clock(windowS() * 1000);
  /** The shot times the grinder adjusts from, for its current or last
   *  request (`grinder.shot_range`: over min_ms, up to max_ms), or null
   *  before the state is in or while that is not known. */
  const grinderRange = () => (S && S.grinder.shot_range) || null;
  /** Whether the grinder adjusts from a shot of `ms`, or null when that is
   *  not known yet. */
  function inGrinderRange(ms) {
    const r = grinderRange();
    return r && ms != null ? ms > r.min_ms && ms <= r.max_ms : null;
  }
  function lastOf(kindsWanted) {
    for (let i = events.length - 1; i >= 0; i--) if (kindsWanted.includes(events[i].kind)) return events[i];
    return null;
  }
  // Why a brew ended without a result, by its `cause` (the server's
  // AbortCause): on purpose (history kind "aborted") or a failure, the text
  // on the Start tab and the history title. A timeout's texts say how long
  // it waited (errorTitle, shotTitle). A test in control_api.rs checks this
  // table against the server.
  const CAUSES = {
    shot_aborted_by_user: { onPurpose: true, text: "start.aborted.user", title: "history.abortedUser" },
    grinder_shot_replaced: { onPurpose: true, text: "start.aborted.replaced", title: "history.replaced" },
    grinder_address_changed: { onPurpose: true, text: "start.aborted.address", title: "history.address" },
    shot_timed_out: { onPurpose: false },
    grinder_shot_dropped: { onPurpose: false, text: "start.error.stoppedWaiting", title: "history.stoppedWaiting" },
    grinder_shot_refused: { onPurpose: false, text: "start.error.refused", title: "history.refused" },
    grinder_unpaired: { onPurpose: false, text: "start.error.unknownApp", title: "start.error.unknownApp" },
    shot_end_missed: { onPurpose: false, title: "history.endMissed" },
  };
  /** The cause of a shot_aborted event or a history entry, or null. */
  const causeOf = (x) => (x && x.cause) || null;
  const causeInfo = (cause) => CAUSES[cause] || {};
  const isAbort = (cause) => !!causeInfo(cause).onPurpose;
  /** An E65/E80 setup: its link, or an address it is starting with. */
  const wifiSetup = () => !!(wifiOf() || (S && S.settings && S.settings.grinder_host));
  /** The E65/E80 link is (re)starting and has not reached the grinder yet. */
  function linkStarting() {
    if (linkOn()) return false;
    const wifi = wifiOf();
    return wifi ? wifi.state === "connecting" : !!(S && S.settings && S.settings.grinder_host);
  }

  // Integrations that report shots by themselves. When every one has failed,
  // the shot can still be typed in by hand.
  function reporters() {
    const list = (S && S.integrations) || [];
    const live = list.filter((i) => i.health !== "failing" && i.health !== "stopped");
    return { list, live, manual: live.length === 0 };
  }
  /** Seconds into a brew after which a shot no integration reported counts
   *  as missed: a minute after it should have ended (the recipe time, else
   *  30 s), but early enough to type it in before the shot window ends, and
   *  not before half of it. */
  function missedAfterS() {
    const due = (targets().t || 30) + 60;
    const win = windowS();
    return win > 0 ? Math.min(due, Math.max(win - 30, win / 2)) : due;
  }
  // Integrations only report brews the grinder asked for.
  const shotMissed = (r) => !r.manual && !!S.shot.requested_by_grinder && elapsedMs() >= missedAfterS() * 1000;

  /** Recipe targets of the last grind: weight (g) and time (s). */
  function targets() {
    const grind = lastOf(["grinder_grind_finished"]);
    let w = grind && grind.result ? grind.result.target_weight_g : null;
    if (w == null && S && S.grinder.last_grind) w = S.grinder.last_grind.target_weight_g;
    // An E65/E80 sends its target time with the request after the grind.
    const req = lastOf(["grinder_shot_requested"]);
    const t = req && grind && req.seq > grind.seq && req.target_time_ms ? req.target_time_ms / 1000 : null;
    return { w: w > 0 ? w : null, t: t > 0 ? t : null };
  }

  /** When the brew ended with this event. */
  function endTime(ev) {
    return ev && seenAt.has(ev.seq) ? seenAt.get(ev.seq) : null;
  }
  /** When the current (or last) brew started: from its event, else (its
   *  event is not in yet) counted back from the running timer. */
  function brewStart(f) {
    if (f.started && seenAt.has(f.started.seq)) return seenAt.get(f.started.seq);
    return f.sc === "running" ? Date.now() - elapsedMs() : null;
  }
  /** How long the last brew ran until its outcome, from the ages of both
   *  events, or null when one is missing. */
  function brewMs(f) {
    if (!f.started || !f.outcome || f.outcome.seq < f.started.seq) return null;
    const start = brewStart(f), end = endTime(f.outcome);
    return start != null && end != null ? end - start : null;
  }
  const fresh = (ev) => { const t = endTime(ev); return t != null && Date.now() - t < FRESH_MS; };
  /** E64: a grind waits for the knob press only for a while. */
  const knobFresh = (ev) => seenAt.has(ev.seq) && Date.now() - seenAt.get(ev.seq) < Math.max(KNOB_MS, windowS() * 1000);

  /** Where the shot is: ready, knob (E64: ground, press the knob), running,
   *  checking (the grinder takes the result), done, error or aborted. */
  function flow() {
    const phase = S.shot.phase;
    const outcome = lastOf(["shot_reported", "shot_aborted"]);
    // A request after the grind (a knob press, or an E65/E80's job) means
    // the knob is no longer waited for.
    const last = lastOf(["grinder_grind_finished", "grinder_shot_requested", "shot_reported", "shot_aborted"]);
    // `reason` is the server's words for `cause`, shown for a cause this
    // app does not know.
    let sc = "ready", reason = null, cause = null;
    if (phase === "running") sc = "running";
    else if (phase === "finishing") {
      if (!S.shot.aborted) sc = "checking";
      else {
        const ended = outcome && outcome.kind === "shot_aborted" ? outcome : null;
        reason = ended ? ended.reason : null;
        cause = causeOf(ended);
        sc = isAbort(cause) ? "aborted" : "error";
      }
    } else if (last && last.kind === "grinder_grind_finished" && knobFresh(last)) sc = "knob";
    else if (last && last.kind === "shot_reported") sc = fresh(last) ? "done" : "ready";
    else if (last && last.kind === "shot_aborted" && !isAbort(causeOf(last)) && fresh(last)) {
      sc = "error"; reason = last.reason; cause = causeOf(last);
    }
    if (sc === "error" && outcome && outcome.seq <= ui.dismissed) sc = "ready";
    return { sc, reason, cause, outcome, started: lastOf(["shot_started"]), job: lastOf(["grinder_grind_finished"]) };
  }

  // Per state: the tone and badge (catalog key under badge.) of each step.
  const PLANS = {
    ready: [["start", "start"], ["none", ""], ["none", ""]],
    knob: [["run", "pressKnob"], ["wait", "waiting"], ["none", ""]],
    running: [["done", "done"], ["run", "running"], ["wait", "waiting"]],
    checking: [["done", "done"], ["done", "done"], ["run", "running"]],
    done: [["done", "done"], ["done", "done"], ["done", "done"]],
    error: [["done", "done"], ["error", "error"], ["skip", "skipped"]],
    aborted: [["done", "done"], ["abort", "aborted"], ["skip", "skipped"]],
  };

  // --- Tabs --------------------------------------------------------------------
  // "setup" is the grinder setup's own view, without the tabs (#setup, or
  // #repair to start an E65/E80 by clearing the old pairing).
  const TABS = ["start", "status", "history", "setup"];
  function showTab(name) {
    // The log is the last card on Status: #log opens it there.
    const toLog = name === "log";
    if (toLog) name = "status";
    if (name === "setup" || name === "repair") {
      const mode = name === "repair" ? "repair" : "setup";
      // Opened from the app (which forgot the last one), an address typed
      // or a reload, which goes on with the person's answers.
      if (tab !== "setup") wz = restoredSetup(mode) || freshSetup(mode);
      else if (wz.mode !== mode) wz = freshSetup(mode);
      name = "setup";
    }
    if (!TABS.includes(name)) name = "start";
    // Leaving the setup ends it: the next one starts afresh.
    if (tab === "setup" && name !== "setup") forgetSetup();
    const changed = name !== tab;
    tab = name;
    for (const t of TABS) $("tab-" + t).hidden = t !== name;
    $("app-head").hidden = $("tabs").hidden = name === "setup";
    $("setup-head").hidden = name !== "setup";
    for (const a of document.querySelectorAll("nav.tabs a")) {
      if (a.dataset.tab === name) a.setAttribute("aria-current", "page"); else a.removeAttribute("aria-current");
    }
    if (changed && !toLog) window.scrollTo(0, 0);
    renderAll();
    if (toLog) { log.open = true; scrollToLog(); openLog().then(scrollToLog); }
    else if (name === "status" && log.open) openLog();   // back after a restart: start over
    if (name === "history" && primed) loadHistory();   // else the first catch-up does
  }
  /** Opens a tab and puts it in the address, without a navigation: Safari on
   *  iPhone reloaded the whole page for a plain "#tab" link. */
  function goTab(name) {
    if ((location.hash.slice(1) || "start") !== name) history.pushState(null, "", "#" + name);
    showTab(name);
  }
  for (const a of document.querySelectorAll("nav.tabs a")) {
    a.addEventListener("click", (e) => {
      if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;   // new tab/window
      e.preventDefault();
      goTab(a.dataset.tab);
    });
  }
  // Back and forward between tabs, and links typed with a hash.
  window.addEventListener("popstate", () => showTab(location.hash.slice(1)));
  window.addEventListener("hashchange", () => showTab(location.hash.slice(1)));
  $("to-settings").addEventListener("click", (e) => {
    e.preventDefault();
    goTab("status");
    // To the + of Time to pull the shot (rendered now), or at the longest to
    // the field, next to No limit.
    const up = $("set-window-up");
    const b = up.getAttribute("aria-disabled") === "true" ? $("set-window") : up;
    b.scrollIntoView({ block: "center" });
    b.focus();
  });

  // --- Header and system status ---------------------------------------------------
  /** Why the grinder won't grind, unless it is only the shot in progress. */
  function grindBlocked() {
    const b = S && S.grinder.grind_blocked;
    return b && !(S.shot.phase !== "ready" && b.cause === "shot_running") ? b : null;
  }

  /** The last part of a server error ("cannot reach the grinder at …:
   *  operation timed out" gives "operation timed out"): the cause, without
   *  the addresses before it. */
  const lastCause = (text) => String(text || "").split(": ").pop().trim();

  /** Why there is no grinder, in plain words (`text`), and the server's
   *  error behind it (`detail`), which stays English like the log. */
  function grinderMissing(wifi) {
    if (!wifi) return { text: t("sys.missing.e64"), detail: "" };
    const error = wifi.state === "failing" && wifi.detail ? lastCause(wifi.detail) : "";
    // It answers, but not as it should: not a matter of switching it on.
    if (error && wifi.unreachable === false) return { text: t("sys.missing.linkError"), detail: error };
    return { text: t("sys.missing.wifi"), detail: error };
  }

  // System states in which no shot can start: System status opens by
  // itself, step 1 says it is blocked and the steps stay closed.
  const STOPS = ["offline", "noGrinder", "blocked", "setup", "pairing"];
  /** The stop System status `s` shows, or null. Not during a shot, so the
   *  step in use stays open, and not for a moment offline (as when the
   *  phone wakes up), so nothing jumps. */
  function stopOf(s, f) {
    if (!STOPS.includes(s.key) || ["running", "checking", "knob"].includes(f.sc)) return null;
    if (s.key === "offline" && Date.now() - offlineSince < 5000) return null;
    return s.key;
  }

  function sysInfo(f) {
    const wifi = wifiOf();
    // `big` is the System status line; the header pill shows `label`, or
    // `short` where that does not fit.
    const s = (tone, icon, label, text, detail) => ({
      key: label, tone, icon, text, detail: detail || "", label: t("sys.label." + label),
      short: hasText("sys.label." + label + "Short") ? t("sys.label." + label + "Short") : "",
      big: label === "ready" ? t("sys.label.readyLong") : t("sys.label." + label),
    });
    if (!online) return s("error", "alert", "offline", t("sys.text.offline"));
    if (!S) return s("none", "clock", "connecting", "");
    const blocked = grindBlocked();
    if (f.sc === "running") return s("run", "clock", "running", t("sys.text.running"));
    if (f.sc === "checking") return s("run", "clock", "running", t("sys.text.checking"));
    if (f.sc === "knob") return s("run", "clock", "knob", t("sys.text.knob"));
    // Not paired yet: the setup (System status leads there) says what to do.
    const phase = wifi && wifi.state === "pairing" ? wifi.phase : null;
    if (phase === "asking") return s("start", "alert", "pairing", t("sys.text.needsId"));
    if (phase === "quiet") return s("start", "clock", "pairing", t("sys.text.quiet", { n: quietMinutes(wifi) }));
    if (phase === "accepting") return s("start", "alert", "setup", t("sys.text.accept"));
    if (phase) return s("start", "clock", "pairing", t("sys.text.pairing"));
    if (linkStarting()) {
      const host = S.settings && S.settings.grinder_host;
      return s("start", "clock", "linking", host ? t("sys.text.linking", { host }) : t("sys.text.linkingNoHost"));
    }
    if (!linkOn()) {
      // It answers but turns pairing down: not set up, rather than not
      // connected; its answer stays at hand with the log. Any other error
      // shows as one.
      if (setupNeeded() && wifi && wifi.not_set_up) return s("start", "alert", "setup", t("sys.text.setup"), lastCause(wifi.detail));
      const missing = grinderMissing(wifi);
      return s("error", "alert", "noGrinder", missing.text, missing.detail);
    }
    if (blocked) return s("warn", "alert", "blocked", t("sys.text.blocked", { message: blocked.grinder_message }));
    if (f.sc === "error") return s("error", "alert", "error", t("sys.text.error"), errorTitle(f));
    // Shots can still be made, but something needs a look: not green.
    if (wifi && wifi.problem) return s("warn", "alert", "problem", t("sys.text.problem"), sentence(wifi.problem));
    const failing = (S.integrations || []).find((i) => i.health === "failing" || i.health === "stopped");
    if (failing) {
      const r = s("warn", "alert", "integError", t("sys.text.integError", { title: failing.title }), failing.last_error);
      r.integ = failing;
      return r;
    }
    if (f.sc === "done") {
      const at = endTime(f.outcome);
      return s("ok", "check", "ready", at ? t("sys.text.done", { ago: ago(Date.now() - at) }) : t("sys.text.doneNoTime"));
    }
    if (f.sc === "aborted") return s("ok", "check", "ready", t("sys.text.aborted"));
    return s("ok", "check", "ready", "");
  }

  /** The grinder and each integration, in one line. What System status
   *  says already (its title) is left out. */
  function renderChecks(s) {
    const box = $("sys-checks");
    const items = [];
    const wifi = wifiOf();
    if (S && online) {
      let grinder = null;
      if (["noGrinder", "setup", "pairing", "linking", "problem"].includes(s.key)) { /* said above */ }
      else if (wifi && wifi.state === "pairing") grinder = ["wait", "clock", t("check.grinderPairing")];
      else if (linkOn()) grinder = ["ok", "check", tv("check.grinderReady")];
      else if (linkStarting()) grinder = ["wait", "clock", t("check.grinderConnecting")];
      else grinder = ["bad", "alert", t("check.grinderOff")];
      if (grinder) items.push(grinder);
      const list = S.integrations || [];
      for (const i of list) {
        if (i === s.integ) continue;
        const title = { title: i.title };
        if (i.health === "connected" || i.health === "watching") items.push(["ok", "check", t("check.integReady", title)]);
        else if (i.health === "starting") items.push(["wait", "clock", t("check.integConnecting", title)]);
        else items.push(["bad", "alert", i.health === "stopped" ? t("check.integStopped", title) : t("check.integError", title)]);
      }
      if (list.length === 0) items.push(["muted", null, t("check.noIntegration")]);
      // The grinder alone on a line says nothing System status doesn't.
      if (items.length === 1 && items[0] === grinder) items.length = 0;
    }
    box.replaceChildren(...(items.length ? [el("div", { class: "checks-in" }, ...items.map(([cls, ic, text]) =>
      el("span", { class: cls }, ic ? icon(ic, 14, ic === "check" ? 2.6 : 2.2) : null, text)))] : []));
  }

  function setTone(node, tone) {
    node.className = node.className.replace(/\bt-[a-z]+\b/g, "").trim() + " t-" + tone;
  }

  /** Renders the pill and System status; returns what sysInfo() said. */
  function renderHeader(f) {
    const s = sysInfo(f);
    // A stop opens System status; a tap closes it until the next stop. When
    // the stop is over, it goes back to how it was left before.
    const stop = stopOf(s, f);
    if (stop !== ui.sysStop) {
      ui.sysStop = stop;
      ui.sysOpen = stop ? true : ui.sysPref;
    }
    const pill = $("sys-pill");
    setTone(pill, s.tone);
    ui.pill = [s.label, s.short];
    fitPill(s.label, s.short);
    pill.setAttribute("aria-label", t("sys.pill", { label: s.label }));
    pill.setAttribute("aria-expanded", String(ui.sysOpen && tab === "start"));
    setTone($("sys-card"), s.tone);
    $("sys-label").textContent = s.big || s.label;
    $("sys-icon").replaceChildren(icon(s.icon, 22, s.icon === "check" ? 2.2 : 2));
    $("sys-text").textContent = s.text;
    $("sys-text").hidden = !s.text;
    // The way into the setup: an E65/E80 that is not paired yet, or no
    // grinder at all yet (the setup then asks which one).
    $("sys-setup-link").hidden = !(S && online && setupOffered());
    // The last error, with the way to the log.
    $("sys-detail-text").textContent = s.detail;
    $("sys-detail").hidden = !s.detail;
    $("sys-toggle").setAttribute("aria-expanded", String(ui.sysOpen));
    $("sys-body").hidden = !ui.sysOpen;
    renderChecks(s);
    return s;
  }
  /** The pill's label, or its short form when the long one would be cut off. */
  function fitPill(label, short) {
    const text = $("sys-pill-text");
    text.textContent = label;
    if (short && text.scrollWidth > text.clientWidth + 1) text.textContent = short;
  }
  let resizing = false;
  window.addEventListener("resize", () => {
    if (resizing || !ui.pill) return;
    resizing = true;
    requestAnimationFrame(() => { resizing = false; fitPill(...ui.pill); });
  });

  function setSysOpen(open) {
    ui.sysOpen = open;
    if (!ui.sysStop) ui.sysPref = open;
  }
  function toggleSys() {
    setSysOpen(!ui.sysOpen);
    renderAll();
  }
  $("sys-toggle").addEventListener("click", toggleSys);
  $("sys-pill").addEventListener("click", () => {
    if (tab !== "start") {
      setSysOpen(true);
      goTab("start");
      return;
    }
    toggleSys();
  });

  // --- Start tab -----------------------------------------------------------------
  /** A banner above System status, only for what System status (`s`)
   *  does not say itself, such as a grinder that went away mid-shot. */
  function renderBanner(f, s) {
    const b = $("banner");
    const wifi = wifiOf();
    // A status region announces every change, so only real ones.
    const show = (cls, text) => {
      b.hidden = false;
      if (b.className !== "banner " + cls) b.className = "banner " + cls;
      if (b.textContent !== text) b.textContent = text;
    };
    if (!S || !online) { b.hidden = true; return; }
    const blocked = grindBlocked();
    if (blocked && s.key !== "blocked") show("warn", t("sys.text.blocked", { message: blocked.grinder_message }));
    else if (wifi && wifi.state === "pairing") b.hidden = true; // System status and the setup say it
    else if (linkStarting()) b.hidden = true; // the system status says it
    else if (!linkOn()) { if (s.key === "noGrinder" || s.key === "setup") b.hidden = true; else show("bad", grinderMissing(wifi).text); }
    else if (wifi && wifi.problem && s.key !== "problem" && !(f.sc === "error" && f.reason === wifi.problem)) show("warn", t("sys.problem", { problem: sentence(wifi.problem) }));
    else b.hidden = true;
  }

  function setStep(n, tone, badgeKey, open) {
    const label = badgeKey ? t("badge." + badgeKey) : "";
    const card = $("step-" + n);
    const head = card.querySelector(".card-head");
    const numEl = card.querySelector("[data-num]");
    setTone(numEl, tone);
    numEl.replaceChildren(tone === "done" ? icon("check", 16, 2.6) : String(n));
    const badge = card.querySelector("[data-badge]");
    badge.hidden = !label;
    badge.textContent = label;
    setTone(badge, tone);
    head.setAttribute("aria-expanded", String(open));
    $("step-" + n + "-body").hidden = !open;
  }
  for (const head of document.querySelectorAll("[data-step]")) {
    head.addEventListener("click", () => {
      const n = Number(head.dataset.step);
      ui.open = ui.open === n ? 0 : n;
      renderStart();
    });
  }

  /** "the La Marzocco cloud and the Half Decent Scale", from integrations. */
  const theTitles = (live) => joinList(live.map((i) => t("start.integrationName", { title: i.title })));

  function renderStart() {
    if (!S) return;
    const f = flow();
    const r = reporters();
    const tg = targets();
    const sys = renderHeader(f);
    renderBanner(f, sys);

    // Step 1 says when no shot can start: blocked while there is no grinder
    // (or no gbs-anywhere, or the grinder won't grind), waiting while it
    // connects or pairs.
    const stop = !!stopOf(sys, f);
    // Step 2 shows the fields to type the shot in when no integration can
    // report it: none is working, the brew was started without the grinder,
    // or none reported it in time. Once shown, they stay for this brew.
    const brew = f.sc === "running" ? (f.started ? f.started.seq : -1) : null;
    if (brew !== ui.brew) { ui.brew = brew; ui.typeIn = ui.missed = false; }
    if (brew !== null) {
      ui.missed = ui.missed || shotMissed(r);
      ui.typeIn = ui.typeIn || r.manual || !S.shot.requested_by_grinder || ui.missed;
    }
    let plan = PLANS[f.sc];
    // Also after a shot, whose result stays in steps 2 and 3.
    if (stop && (sys.key === "setup" || sys.key === "pairing")) plan = [["wait", "waiting"], ...plan.slice(1)];
    else if (stop && ["ready", "done", "error", "aborted"].includes(f.sc)) plan = [[sys.tone === "warn" ? "warn" : "error", "blocked"], ...plan.slice(1)];
    else if (f.sc === "ready" && sys.key === "linking") plan = [["wait", "waiting"], ["none", ""], ["none", ""]];

    // Which step is open: the running or failed one opens by itself when the
    // shot moves on, step 3 with the result once done; otherwise step 1, or
    // none during a stop (System status tells why). A tap overrides it until
    // then.
    const key = f.sc + ":" + (f.outcome ? f.outcome.seq : 0) + ":" + (f.job ? f.job.seq : 0) + ":" + (stop ? sys.key : "") + ":" + ui.typeIn;
    if (key !== ui.planKey) {
      ui.planKey = key;
      const auto = plan.findIndex(([tone]) => tone === "run" || tone === "error");
      ui.open = stop ? 0 : auto >= 0 ? auto + 1 : f.sc === "done" ? 3 : 1;
      ui.errInfo = false;
    }
    if (f.sc === "running" && ui.lastScenario !== "running") {
      // A typed weight stays for the next shot; left empty, the recipe's is sent.
      $("f-time").value = ""; setHint();
      if (ui.lastScenario !== null && navigator.vibrate) { try { navigator.vibrate([120, 80, 120]); } catch (_) {} }
    }
    ui.lastScenario = f.sc;
    plan.forEach(([tone, label], i) => setStep(i + 1, tone, label, ui.open === i + 1));

    // Step 1
    const recipe = [tg.t ? t("unit.seconds", { time: secs(tg.t) }) : null, tg.w ? t("unit.grams", { weight: gramsShort(tg.w) }) : null].filter(Boolean).join(" · ");
    const s1 = $("s1-text"), m1 = $("s1-meta");
    if (f.sc === "ready") {
      // A line break in the text becomes one on the page.
      const lines = tv("start.step1.ready").split("\n");
      s1.replaceChildren(...lines.flatMap((line, i) => (i ? [el("br"), line] : [line])));
      m1.textContent = recipe ? t("start.step1.lastRecipe", { recipe }) : "";
    } else if (f.sc === "knob") {
      s1.replaceChildren(el("strong", { text: t("start.step1.knob") }), t("start.step1.knobRest"));
      m1.textContent = tg.w ? t("start.step1.target", { weight: t("unit.grams", { weight: gramsShort(tg.w) }) }) : "";
    } else {
      const at = brewStart(f);
      s1.textContent = at ? tv("start.step1.at", { time: hhmmss(new Date(at)) }) : t("start.step1.ground");
      m1.textContent = recipe ? t("start.step1.recipeTarget", { recipe }) : "";
    }
    m1.hidden = !m1.textContent;

    // Step 2
    const win = windowS();
    $("s2-ready").hidden = !(f.sc === "ready" || f.sc === "knob");
    if (!$("s2-ready").hidden) {
      const tail = r.manual ? t("start.step2.tail.manual") : t("start.step2.tail.auto", { n: r.live.length, names: theTitles(r.live) });
      $("s2-ready").textContent = tv("start.step2.lead") + " "
        + (win > 0 ? t("start.step2.window", { window: windowLabel(), tail }) : t("start.step2.noWindow", { tail }));
    }
    $("s2-run").hidden = f.sc !== "running";
    if (f.sc === "running") {
      $("s2-timer").setAttribute("aria-label", tv("start.step2.timer"));
      $("s2-info-btn").setAttribute("aria-expanded", String(ui.info));
      $("s2-info").hidden = !ui.info;
      $("s2-info").textContent = [
        tv("start.step2.since"),
        r.manual || !S.shot.requested_by_grinder ? t("start.step2.info.manual") : cap(t("start.step2.info.auto", { n: r.live.length, names: theTitles(r.live) })),
        win > 0 ? t("start.step2.info.limit", { window: windowLabel() }) : "",
      ].filter(Boolean).join(" ");
      $("s2-flush").hidden = !!S.shot.requested_by_grinder;
      // Whichever comes first wins: the shot typed in or an integration's.
      const note = r.manual && r.list.length ? cap(t("start.step2.failed", { n: r.list.length, names: theTitles(r.list) }))
        : ui.missed ? t("start.step2.missed", { n: r.live.length, names: theTitles(r.live) }) : "";
      // A status region announces every change, so only real ones.
      if ($("s2-missed").textContent !== note) $("s2-missed").textContent = note;
      $("s2-missed").hidden = !note;
      $("s2-auto").hidden = ui.typeIn;
      $("shot-form").hidden = !ui.typeIn;
      $("s2-t-target").textContent = tg.t ? t("start.step2.tile.targetTime", { value: t("unit.seconds", { time: secs(tg.t) }) }) : "";
      $("s2-t-target").hidden = !tg.t;
      $("s2-w-target").textContent = tg.w ? t("start.step2.tile.targetWeight", { value: t("unit.grams", { weight: gramsShort(tg.w) }) }) : "";
      $("s2-w-target").hidden = !tg.w;
      $("f-time").placeholder = tg.t ? secs(tg.t) : "";
      $("f-weight").placeholder = tg.w ? gramsShort(tg.w) : "";
      renderClock();
    }
    const shown = f.sc === "checking" || f.sc === "done";
    $("s2-done").hidden = !shown;
    let res = null, shot = "", rating = "";
    if (shown) {
      res = f.sc === "checking" ? S.shot.result : (f.outcome && f.outcome.result);
      shot = res ? t("unit.shot", { time: secs(res.time_ms / 1000), weight: grams(res.weight_g) }) : "";
      $("s2-shot").textContent = shot;
      const at = endTime(f.outcome);
      const by = reportedBy(f.outcome, res);
      $("s2-source").textContent = [by, at ? hhmmss(new Date(at)) : null].filter(Boolean).join(" · ");
    }
    $("s2-error").hidden = f.sc !== "error";
    if (f.sc === "error") {
      const timeout = f.cause === "shot_timed_out";
      $("s2-err-title").textContent = errorTitle(f);
      $("s2-err-info-btn").hidden = !timeout;
      $("s2-err-info-btn").setAttribute("aria-expanded", String(ui.errInfo));
      $("s2-err-info").hidden = !(timeout && ui.errInfo);
    }
    $("s2-aborted").hidden = f.sc !== "aborted";
    if (f.sc === "aborted") $("s2-aborted").textContent = t(causeInfo(f.cause).text);

    // Step 3
    $("s3-wait").hidden = !(f.sc === "ready" || f.sc === "knob" || f.sc === "running");
    $("s3-outcome").hidden = !shown;
    if (shown && res) {
      $("s3-shot").textContent = shot;
      rating = renderOutcome(res, tg, f.outcome && f.outcome.kind === "shot_reported" ? f.outcome : null);
    }
    $("s3-skip").hidden = !(f.sc === "error" || f.sc === "aborted");

    // What each step came to, under its title (hidden while open): the
    // recipe, the shot (not everyone has a scale) and the grinder's rating.
    const ground = !(f.sc === "ready" || f.sc === "knob");
    [ground && recipe ? t("start.step1.recipeTarget", { recipe }) : "", shot, rating].forEach((text, i) => {
      const sum = $("step-" + (i + 1)).querySelector("[data-sum]");
      if (sum.textContent !== text) sum.textContent = text;
      sum.hidden = !text;
      const open = ui.open === i + 1;
      sum.classList.toggle("ghost", open);
      if (open) sum.setAttribute("aria-hidden", "true"); else sum.removeAttribute("aria-hidden");
    });
  }

  /** "Reported by La Marzocco cloud" or "Entered by hand". */
  function reportedBy(ev, res) {
    if (!res) return "";
    if (ev && ui.mine.has(ev.seq)) return t("start.step2.byHand");
    const i = ((S && S.integrations) || []).find((x) => x.last_report && x.last_report.time_ms === res.time_ms
      && Math.abs(x.last_report.weight_g - res.weight_g) < 0.05);
    return i ? t("start.step2.reportedBy", { title: i.title }) : t("start.step2.byHand");
  }

  /** Why the shot of flow() `f` failed. */
  function errorTitle(f) {
    if (f.cause === "shot_timed_out") {
      // How long it waited, from the server's events: not the setting now,
      // which may have changed since.
      const ms = brewMs(f);
      return ms > 0 ? t("start.error.timeout", { window: clock(ms) }) : t("start.error.timeoutNoLimit");
    }
    const text = causeInfo(f.cause).text;
    if (text) return t(text);
    // A cause this app does not know: as the server says it.
    return f.reason ? cap(f.reason) : t("start.error.generic");
  }

  /** Green when the grinder adjusts from the shot; grey when it ignores
   *  the shot or (as in the design) the shot was on target. Returns the
   *  rating in short with what to expect, for the closed step's line. */
  function renderOutcome(res, tg, ev) {
    const secsShot = res.time_ms / 1000;
    const box = $("s3-outcome");
    const range = grinderRange();
    const inRange = (ev ? ev.in_grinder_range : inGrinderRange(res.time_ms)) !== false;
    let result, change = "", good = false, sum;
    // in_grinder_range does not say on which side a shot missed, so the
    // time picks the wording: a shot too short also gets the hint to grind
    // finer by hand.
    if (range && res.time_ms <= range.min_ms) {
      result = t("outcome.tooShort", { time: secs(secsShot) });
      change = t("outcome.finerByHand");
      sum = t("start.step3.sum.tooShort");
    } else if (!inRange) {
      result = t("outcome.tooLong", { time: secs(secsShot) });
      sum = t("start.step3.sum.tooLong");
    } else if (!tg.t) {
      good = true;
      result = t("outcome.sent");
      change = t("outcome.compares");
      sum = cap(result);
    } else {
      const d = secsShot - tg.t;
      if (Math.abs(d) <= ON_TARGET_S) {
        result = t("outcome.onTarget", { time: secs(secsShot), target: secs(tg.t) });
        sum = t("start.step3.sum.onTarget");
      }
      else {
        good = true;
        const diff = { diff: secs(Math.abs(d)) };
        result = d < 0 ? t("outcome.fast", diff) : t("outcome.slow", diff);
        change = d < 0 ? t("outcome.finer") : t("outcome.coarser");
        sum = t(d < 0 ? "start.step3.sum.fast" : "start.step3.sum.slow", diff);
      }
    }
    box.classList.toggle("good", good);
    $("s3-result").textContent = result;
    $("s3-change-row").hidden = !change;
    $("s3-change").textContent = change;
    return sum;
  }

  function elapsedMs() {
    if (!S || S.shot.phase !== "running") return 0;
    return (S.machine.mako.REAL_EXTRACTION_TIME_MS || 0) + (performance.now() - stateAt);
  }
  function renderClock() {
    if (!S || S.shot.phase !== "running" || tab !== "start") return;
    // A shot counts as missed while the clock runs: show the fields then.
    if (!$("s2-run").hidden && !ui.missed && shotMissed(reporters())) { renderStart(); return; }
    const ms = elapsedMs();
    $("s2-timer").textContent = clockDown(ms);
    const win = windowS() * 1000;
    $("s2-left").hidden = !(win > 0);
    if (win > 0) {
      const left = Math.max(0, Math.ceil((win - ms) / 1000));
      $("s2-left").textContent = t("start.step2.left", { time: Math.floor(left / 60) + ":" + pad2(left % 60) });
    }
  }

  $("s2-info-btn").addEventListener("click", () => { ui.info = !ui.info; renderStart(); });
  $("s2-err-info-btn").addEventListener("click", () => { ui.errInfo = !ui.errInfo; renderStart(); });
  $("s2-dismiss").addEventListener("click", () => {
    const o = lastOf(["shot_aborted"]);
    if (o) ui.dismissed = o.seq;
    renderStart();
  });

  // Manual entry: the default flow without an integration.
  function setHint(message, warn) {
    const h = $("form-hint");
    const typed = num($("f-time").value);
    const ok = isFinite(typed) && typed > 0;
    $("send").disabled = !ok || busy;
    let text = message || "", isWarn = !!warn;
    if (!message && ok) {
      // No warning before the state (and with it the limits) is in.
      const range = grinderRange(), limit = (ms) => ({ limit: fmt.upToOne.format(ms / 1000) });
      if (range && typed * 1000 <= range.min_ms) { text = tv("start.form.tooShort", limit(range.min_ms)); isWarn = true; }
      else if (range && typed * 1000 > range.max_ms) { text = t("start.form.tooLong", limit(range.max_ms)); isWarn = true; }
    }
    h.textContent = text;
    h.className = "hint" + (isWarn ? " warn" : "");
    h.hidden = !text;
  }
  let busy = false;
  $("f-time").addEventListener("input", () => setHint());
  $("f-time").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); $("f-weight").focus(); } });
  /** Marks the shot sent from this page once its event is in: the newest
   *  shot in the range (after a gap, an integration's may come before it). */
  function markMine() {
    const r = ui.mineRange;
    if (!r) return;
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i];
      if (e.kind !== "shot_reported" || e.seq <= r.after || e.seq > r.upTo) continue;
      ui.mine.add(e.seq); ui.mineRange = null;
      return;
    }
  }
  $("shot-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    if (busy) return;
    const secsTyped = num($("f-time").value), w = num($("f-weight").value);
    if (!isFinite(secsTyped) || secsTyped <= 0) { setHint(t("start.form.enterTime"), true); $("f-time").focus(); return; }
    busy = true; $("send").disabled = true;
    const after = lastSeq;
    try {
      // Without a weight the server uses the recipe's; it never sends 0 g.
      const shot = w > 0 ? { time_s: secsTyped, weight_g: w } : { time_s: secsTyped };
      const st = await api("/api/shot/result", { method: "POST", body: JSON.stringify(shot) });
      // Only a shot the server took was typed here ("Entered by hand"): its
      // event falls between the last one seen before and the reply.
      ui.mineRange = { after, upTo: st.server.last_event_seq };
      markMine();
      if (document.activeElement) document.activeElement.blur();
      busy = false;
      setHint();
      await tick();
    } catch (err) {
      busy = false;
      setHint(t("start.form.notSent", { error: err.message }), true);
    }
  });
  for (const b of document.querySelectorAll("[data-abort]")) {
    b.addEventListener("click", () => {
      if (busy) return;
      openSheet({
        title: t("start.abort.title"),
        body: [el("p", { class: "body-text", text: t("start.abort.text") })],
        actions: [
          { label: t("start.abort.keep"), cls: "s-plain", run: closeSheet, autofocus: true },
          { label: t("start.step2.abort"), cls: "s-danger", run: async (btn) => {
            btn.disabled = true;
            try {
              await api("/api/shot/abort", { method: "POST" });
              closeSheet();
              tick();
            } catch (err) { sheetError(t("start.abort.failed", { error: err.message })); btn.disabled = false; }
          } },
        ],
        returnTo: b,
      });
    });
  }

  // --- Status tab ------------------------------------------------------------------
  // Integration health: tone of its pill (the label is integ.health.<health>).
  const HEALTH = { starting: "start", connected: "ok", watching: "run", failing: "error", stopped: "error" };
  const kindOf = (id) => (kinds || []).find((k) => k.id === id);
  const hasPassword = (k) => !!(k && k.fields.some((f) => f.input === "password"));

  // Icons from the --icons directory, probed once per kind
  // (undefined: not probed yet, null: none, string: URL).
  const customIcons = {};
  async function probeIcon(id) {
    customIcons[id] = null;
    for (const ext of ["svg", "png", "webp"]) {
      try {
        const r = await fetch("/icons/" + id + "." + ext, { method: "HEAD", cache: "no-store" });
        if (r.ok) { customIcons[id] = "/icons/" + id + "." + ext; invalidateIntegrations(); return; }
      } catch (_) {}
    }
  }
  function iconFor(kindId, node) {
    const box = node || el("span", { class: "integ-icon" });
    if (!/^[a-z0-9_-]+$/.test(kindId)) { box.replaceChildren(icon("cloud", 22, 1.8)); return box; }
    if (customIcons[kindId] === undefined) probeIcon(kindId);
    if (customIcons[kindId]) {
      const mark = el("span", { class: "mark" });
      mark.style.setProperty("--icon", "url('" + customIcons[kindId] + "')");
      box.replaceChildren(mark);
    } else {
      const k = kindOf(kindId);
      // k.icon is the integration's icon.svg, compiled into the server
      // (GET /api/integrations/kinds); markup must never come from anywhere else.
      if (k && k.icon) box.innerHTML = k.icon; else box.replaceChildren(icon("cloud", 22, 1.8));
    }
    return box;
  }

  async function loadKinds() {
    if (kindsLoading) return;
    kindsLoading = true;
    try {
      const r = await api("/api/integrations/kinds");
      kinds = Array.isArray(r.kinds) ? r.kinds : [];
      kindsPersistent = !!r.persistent;
      invalidateIntegrations();
    } catch (_) {}
    kindsLoading = false;
  }

  /** The host of the grinder URL in use (`https://192.168.1.30:30001`). */
  function urlHost(u) {
    if (!u) return null;
    try { return new URL(u).hostname.replace(/^\[|\]$/g, ""); } catch (_) { return u; }
  }
  function hostOf(peer) {
    if (!peer) return null;
    const m = /^\[([^\]]+)\]:\d+$/.exec(peer) || /^([^:]+):\d+$/.exec(peer);
    return m ? m[1] : peer;
  }

  function renderGrinder() {
    const wifi = wifiOf();
    const on = linkOn();
    const e65 = wifiSetup();
    $("g-model").textContent = tv("grinder.model");
    let tone = "error", state = "notConnected";
    if (wifi && wifi.phase === "asking") { tone = "start"; state = "needsId"; }
    else if (wifi && wifi.state === "pairing") { tone = "start"; state = "pairing"; }
    else if (on) { tone = "ok"; state = "ready"; }
    else if (linkStarting()) { tone = "start"; state = "connecting"; }
    else if (setupNeeded() && wifi && wifi.not_set_up) { tone = "start"; state = "notSetUp"; }
    setTone($("g-pill"), tone);
    // Set up while an E65/E80 is not paired, or no grinder is known yet;
    // clearing a pairing is in the menu.
    $("g-setup").hidden = !setupOffered() || !$("g-host-form").hidden;
    $("g-pill-text").textContent = t("grinder.state." + state);
    renderGrinderMenu();
    const age = S.grinder.last_poll_age_ms;
    const typed = S.settings && S.settings.grinder_host;
    const facts = [
      ["ip", e65 ? (typed || (wifi && urlHost(wifi.grinder)) || "–") : (hostOf(S.grinder.peer) || "–")],
    ];
    if (e65) facts.push(["pairingId", wifi && wifi.pairing_id != null ? String(wifi.pairing_id) : t("grinder.fact.notPaired")]);
    if (wifi && wifi.pairing_code != null && wifi.state === "pairing") facts.push(["pairingCode", String(wifi.pairing_code)]);
    facts.push(["lastContact", age == null ? t("grinder.fact.never") : ago(age)]);
    // Idle only with a grinder to brew for; a shot under way still shows.
    const phase = S.shot.phase;
    const inShot = phase === "running" || phase === "finishing";
    facts.push(["group", inShot ? t("grinder.group." + phase) : on ? t("grinder.group.idle") : "–"]);
    $("g-facts").replaceChildren(...facts.map(([k, v]) => el("div", null, el("dt", { text: t("grinder.fact." + k) }), el("dd", { text: v }))));
  }

  $("g-setup").addEventListener("click", () => openSetup("setup"));

  // The ⋮ menu of a card on the Status tab (the grinder's, an
  // integration's), one open at a time. `menuFor` is the open one's key:
  // "grinder" or "integ:<id>".
  let menuFor = null;
  /** The ⋮ button that opens the menu `key`. */
  function menuButton(key, label) {
    return el("button", {
      type: "button", class: "kebab", "aria-label": label, "data-menu": key,
      "aria-haspopup": "menu", "aria-expanded": String(menuFor === key),
      onclick: (e) => {
        e.stopPropagation();
        if (menuFor === key) { closeMenu(true); return; }
        menuFor = key; invalidateIntegrations(); focusMenu();
      },
    }, kebab());
  }
  /** The open menu: `actions` are [name, icon, class, run]. */
  function menuList(label, actions) {
    return el("div", { class: "menu", role: "menu", "aria-label": label },
      actions.map(([name, ic, cls, run]) => el("button", {
        type: "button", role: "menuitem", class: cls,
        onclick: () => { closeMenu(false); run(); },
      }, icon(ic, 18, 2), name)));
  }
  /** Closes the ⋮ menu; `refocus` puts focus back on its button. */
  function closeMenu(refocus) {
    const key = menuFor;
    menuFor = null;
    invalidateIntegrations();
    if (refocus && key) focusKebab(key);
  }
  function focusKebab(key) {
    const b = [...document.querySelectorAll(".kebab")].find((x) => x.dataset.menu === key);
    if (b) b.focus();
  }
  function focusMenu() {
    requestAnimationFrame(() => { const b = document.querySelector(".menu button"); if (b) b.focus(); });
  }
  document.addEventListener("click", (e) => {
    if (menuFor && !e.target.closest(".menu")) closeMenu(false);
  });
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (!$("sheet").hidden) closeSheet();
    else if (menuFor) closeMenu(true);
  });

  // The grinder's menu: its address (in the form on the card), and while
  // an E65/E80 is paired, clearing the pairing (the setup's re-pair mode,
  // which starts on the grinder). Rebuilt only when it changes, so the
  // focus stays in it between renders.
  const gMore = menuButton("grinder", "");
  gMore.hidden = true;   // until the state is in
  $("g-end").append(gMore);
  let gMenuSig = "";
  function grinderActions() {
    const acts = [[S.settings && S.settings.grinder_host ? t("grinder.host.change") : t("grinder.host.enter"), "edit", "", () => hostEditor(true)]];
    if (wifiPaired()) acts.push([t("grinder.clear"), "trash", "danger", () => openSetup("repair")]);
    return acts;
  }
  function renderGrinderMenu() {
    const open = menuFor === "grinder";
    const acts = grinderActions();
    const sig = JSON.stringify([lang, open, acts.map((a) => a[0])]);
    if (sig === gMenuSig) return;
    gMenuSig = sig;
    gMore.hidden = false;
    gMore.setAttribute("aria-label", t("grinder.more"));
    gMore.setAttribute("aria-expanded", String(open));
    const old = $("g-card").querySelector(":scope > .menu");
    if (old) old.remove();
    if (open) $("g-card").append(menuList(t("grinder.menu"), acts));
  }

  // Integrations list. Rebuilt only when something shown changes; an open
  // edit form stays in place while the other cards change around it.
  let integSig = "", editing = null, editForm = null, integOpen = store.get("integOpen") !== "0";
  /** Rebuilds the integrations list on the next render, and renders. */
  function invalidateIntegrations() { integSig = ""; renderStatus(); }
  function renderIntegrations() {
    const list = S.integrations || [];
    $("integ-count").textContent = String(list.length);
    $("integ-toggle").setAttribute("aria-expanded", String(integOpen));
    $("integ-body").hidden = !integOpen;
    $("add-integration").hidden = !(kinds && kinds.length);
    const adding = !$("add-panel").hidden;
    $("integ-empty").hidden = list.length > 0 || adding;
    // Deleted from another phone while being edited here: the form goes.
    if (editing && !list.some((i) => i.id === editing)) { editing = null; editForm = null; }
    const sig = JSON.stringify([list.map((i) => [i.id, i.title, i.subject, i.health, i.detail, integDetail(i), i.last_error, i.reports, i.source, i.adoptable, i.kind]),
      !!kinds, menuFor, editing]);
    if (sig === integSig) return;
    integSig = sig;
    const box = $("integ-list");
    const nodes = list.map((i) => (i.id === editing && editForm) || integItem(i));
    if (editForm && editForm.parentNode === box) {
      // The other cards change around the open form, which keeps its focus
      // and what was typed.
      const at = nodes.indexOf(editForm);
      for (const c of [...box.children]) if (c !== editForm) c.remove();
      editForm.before(...nodes.slice(0, at));
      editForm.after(...nodes.slice(at + 1));
    } else box.replaceChildren(...nodes);
  }

  function integItem(i) {
    const tone = HEALTH[i.health] || "none";
    const label = HEALTH[i.health] ? t("integ.health." + i.health) : i.health;
    const key = "integ:" + i.id;
    const more = menuButton(key, t("integ.more", { title: i.title }));
    const actions = integActions(i);
    const head = el("div", { class: "integ-row" },
      iconFor(i.kind),
      el("div", { class: "integ-name" },
        el("p", { class: "integ-title", text: i.title }),
        i.subject ? el("p", { class: "integ-subject", text: i.subject }) : null),
      el("div", { class: "head-end" },
        el("span", { class: "pill-sm t-" + tone }, el("span", { class: "dot" }), label),
        actions.length ? more : null));
    const bad = i.health === "failing" || i.health === "stopped";
    const detailText = i.health === "failing" ? (i.last_error || i.detail || t("integ.errorFallback"))
      : i.health === "stopped" ? t("integ.stopped") : integDetail(i);
    const foot = [];
    if (i.reports > 0) foot.push(t("integ.reports", { n: i.reports }));
    if (i.source === "command_line") foot.push(t("integ.fromStart"));
    const item = el("div", { class: "integ" }, head,
      detailText ? el("p", { class: "integ-detail" + (bad ? " bad" : ""), text: sentence(detailText).replace(/\.$/, "") }) : null,
      foot.length ? el("p", { class: "foot", text: foot.join(" · ") }) : null);
    if (menuFor === key) item.append(menuList(t("integ.menu", { title: i.title }), actions));
    return item;
  }

  /** An integration's status line in the current language: the catalog's
   *  wording for its detail_key (integ.detail.<key>), filled from
   *  detail_vars, else the server's English `detail`. A value's name ends in
   *  its unit, which the wording leaves out: age_ms is {age} ("5 h ago", in
   *  ageOf's steps), time_ms {time} in seconds and weight_g {weight} with
   *  one decimal,
   *  other numbers ({every}) with up to one, text ({host}) as it is. */
  function integDetail(i) {
    if (!i.detail_key) return i.detail;
    const vars = {};
    for (const [name, x] of Object.entries(i.detail_vars || {})) {
      if (x == null || typeof x === "object") continue;   // only numbers and text are filled in
      vars[name.replace(/_(ms|s|g)$/, "")] = typeof x !== "number" ? String(x)
        : name === "age_ms" ? ageOf(x) : name === "time_ms" ? secs(x / 1000) : name === "weight_g" ? grams(x)
        : fmt.upToOne.format(x);
    }
    const text = serverText("integ.detail." + i.detail_key, i.detail, vars);
    // A value the wording needs but the server did not send: English.
    return /\{\w+\}/.test(text) ? i.detail : text;
  }

  /** The ⋮ menu: only integrations in the app can be deleted there. */
  function integActions(i) {
    const acts = [];
    if (kindOf(i.kind)) acts.push([t("common.edit"), "edit", "", () => (i.source === "command_line" ? moveToEdit(i) : startEdit(i))]);
    if (i.source === "app") acts.push([t("common.delete"), "trash", "danger", () => confirmDelete(i)]);
    return acts;
  }

  $("integ-toggle").addEventListener("click", () => {
    integOpen = !integOpen; store.set("integOpen", integOpen ? "1" : "0"); menuFor = null; invalidateIntegrations();
  });

  /** Edit one from the start settings: it is moved into the app first. */
  function moveToEdit(i) {
    const k = kindOf(i.kind);
    if (!i.adoptable) {
      const noFile = S.settings && S.settings.persistent === false;
      return openSheet({
        returnTo: () => focusKebab("integ:" + i.id),
        title: t("integ.cmdOnly.title", { title: i.title }),
        body: [el("p", { class: "body-text", text: t("integ.cmdOnly.text") + (noFile ? " " + t("integ.cmdOnly.noFile") : "") })],
        actions: [{ label: t("common.ok"), cls: "s-plain", run: closeSheet }],
      });
    }
    openSheet({
      returnTo: () => focusKebab("integ:" + i.id),
      title: t("integ.adopt.title", { title: i.title }),
      body: [
        el("p", { class: "body-text", text: hasPassword(k) ? t("integ.adopt.textPassword") : t("integ.adopt.text") }),
        el("p", { class: "body-text", text: t("integ.adopt.after") }),
      ],
      actions: [
        { label: t("common.cancel"), cls: "s-plain", run: closeSheet },
        { label: t("integ.adopt.confirm"), cls: "s-primary", run: async (btn) => {
          btn.disabled = true;
          try {
            const view = await api("/api/integrations/" + encodeURIComponent(i.id) + "/adopt", { method: "POST" });
            closeSheet();
            await tick();
            startEdit(((S && S.integrations) || []).find((x) => x.id === i.id) || Object.assign({}, i, view));
          } catch (err) { sheetError(err.message); btn.disabled = false; }
        } },
      ],
    });
  }

  /** The word typed to confirm a delete: DELETE or LÖSCHEN, in any case
   *  (LOESCHEN for keyboards without umlauts). */
  const DELETE_WORDS = ["DELETE", "LÖSCHEN", "LOESCHEN"];
  const deleteTyped = (v) => DELETE_WORDS.includes(v.trim().toUpperCase());
  function confirmDelete(i) {
    const k = kindOf(i.kind);
    const input = el("input", { autocapitalize: "characters", autocomplete: "off", spellcheck: "false", "aria-describedby": "sheet-title" });
    let delBtn = null;
    input.addEventListener("input", () => { if (delBtn) delBtn.disabled = !deleteTyped(input.value); });
    openSheet({
      returnTo: () => focusKebab("integ:" + i.id),
      title: t("integ.delete.title", { title: i.title }),
      body: [
        el("p", { class: "body-text", text: hasPassword(k) ? t("integ.delete.textPassword") : t("integ.delete.text") }),
        ...(i.from_command_line ? [el("p", { class: "body-text", text: t("integ.delete.moved") })] : []),
        el("label", { class: "sheet-label" }, t("integ.delete.type"), input),
      ],
      actions: [
        { label: t("common.cancel"), cls: "s-plain", run: closeSheet },
        { label: t("common.delete"), cls: "s-danger", disabled: true, ref: (b) => { delBtn = b; }, run: async (btn) => {
          if (!deleteTyped(input.value)) return;
          btn.disabled = true;
          try {
            await api("/api/integrations/" + encodeURIComponent(i.id), { method: "DELETE" });
            closeSheet();
            invalidateIntegrations();
            await tick();
          } catch (err) { sheetError(t("integ.delete.failed", { error: err.message })); btn.disabled = false; }
        } },
      ],
      focus: input,
      submitOnEnter: true,
    });
  }

  /** One form field from the setup form of integration kind `kindId`. */
  function formField(kindId, f, prefix, value, secretSaved) {
    const id = prefix + f.key;
    // Label, help and a placeholder that is a word come from the server in
    // English, translated by the catalog where it has them (serverText).
    const key = (part) => kindKey(kindId, "field." + f.key + "." + part);
    const input = el("input", {
      id, name: f.key, type: { email: "email", password: "password" }[f.input] || "text",
      inputmode: f.input === "number" ? "decimal" : null, autocapitalize: "off", spellcheck: "false",
      autocomplete: f.input === "email" ? "username" : f.input === "password" ? (secretSaved ? "new-password" : "current-password") : "off",
    });
    if (f.input === "password" && secretSaved) input.placeholder = "••••••••";
    else if (f.default) {
      input.dataset.stPlaceholder = key("placeholder");
      input.dataset.stPlaceholderEn = f.default;
      input.placeholder = serverText(key("placeholder"), f.default);
    }
    if (value != null) input.value = value;
    input.required = !!f.required && !(f.input === "password" && secretSaved);
    const help = f.input === "password" && secretSaved ? tEl("p", { class: "help" }, "integ.passwordSaved")
      : f.help ? stEl("p", { class: "help" }, key("help"), f.help) : null;
    return el("div", { class: "form-field" },
      el("label", { for: id }, stEl("span", null, key("label"), f.label), f.required ? null : tEl("span", { class: "opt" }, "common.optional")),
      el("span", { class: "field" }, input),
      help);
  }
  function formValues(box) {
    const settings = {};
    for (const input of box.querySelectorAll("input")) {
      const v = input.type === "password" ? input.value : input.value.trim();
      if (v) settings[input.name] = v;
    }
    return settings;
  }

  async function startEdit(i) {
    const k = kindOf(i.kind);
    if (!k) return;
    let saved;
    try { saved = await api("/api/integrations/" + encodeURIComponent(i.id) + "/settings"); }
    catch (err) {
      return openSheet({ title: t("integ.edit.cant", { title: i.title }), body: [el("p", { class: "body-text", text: err.message })], actions: [{ label: t("common.ok"), cls: "s-plain", run: closeSheet }] });
    }
    const values = saved.settings || {};
    const secrets = new Set(saved.secrets_set || []);
    const fields = el("div", { class: "stack-12" }, k.fields.map((f) => formField(k.id, f, "ef-", f.input === "password" ? null : values[f.key], secrets.has(f.key))));
    const error = el("p", { class: "hint warn", role: "alert", hidden: true });
    const save = tEl("button", { type: "submit", class: "btn-send" }, "common.save");
    const form = el("form", { class: "integ stack-12", autocomplete: "off", "aria-label": t("integ.edit.label", { title: i.title }) },
      el("div", { class: "integ-row" }, iconFor(i.kind), el("div", { class: "integ-name" }, tEl("p", { class: "kicker" }, "integ.edit.kicker"), el("p", { class: "integ-title", text: i.title }))),
      fields, error,
      el("div", { class: "actions" }, tEl("button", { type: "button", class: "btn-plain sm", onclick: stopEdit }, "common.cancel"), save));
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      save.disabled = true; error.hidden = true;
      try {
        await api("/api/integrations/" + encodeURIComponent(i.id), { method: "PUT", body: JSON.stringify({ settings: formValues(fields) }) });
        stopEdit();
        await tick();
      } catch (err) { error.textContent = err.message; error.hidden = false; save.disabled = false; }
    });
    editing = i.id; editForm = form;
    invalidateIntegrations();
    const first = form.querySelector("input");
    if (first) first.focus();
  }
  function stopEdit() { const id = editing; editing = null; editForm = null; invalidateIntegrations(); if (id) focusKebab("integ:" + id); }

  // Add integration: pick a kind, then fill in its form.
  let addingKind = null;
  function openAdd(open) {
    $("add-panel").hidden = !open;
    $("add-integration").setAttribute("aria-expanded", String(open));
    if (open) {
      if (!integOpen) { integOpen = true; store.set("integOpen", "1"); }
      showPick();
      renderStatus();
      $("add-panel").scrollIntoView({ behavior: reducedMotion() ? "auto" : "smooth", block: "nearest" });
    } else { addingKind = null; renderStatus(); }
  }
  function showPick() {
    addingKind = null;
    $("add-pick").hidden = false; $("add-form").hidden = true;
    $("kind-list").replaceChildren(...(kinds || []).map((k) => el("li", null,
      el("button", { type: "button", onclick: () => showForm(k) },
        iconFor(k.id),
        el("span", { class: "minw" }, el("span", { class: "kind-title", text: k.title }), stEl("span", { class: "kind-sum" }, kindKey(k.id, "summary"), k.summary)),
        el("span", { class: "chev", style: "display:grid" }, icon("arrow", 18, 2.2))))));
  }
  function showForm(k) {
    addingKind = k;
    $("add-pick").hidden = true; $("add-form").hidden = false;
    iconFor(k.id, $("add-icon"));
    $("add-title").textContent = k.title;
    setServerText($("add-summary"), kindKey(k.id, "summary"), k.summary);
    $("add-error").hidden = true;
    $("add-note").hidden = kindsPersistent;
    $("add-fields").replaceChildren(...k.fields.map((f) => formField(k.id, f, "af-", null, false)));
    const first = $("add-fields").querySelector("input");
    if (first) first.focus();
  }
  $("add-integration").addEventListener("click", () => openAdd($("add-panel").hidden));
  $("add-cancel").addEventListener("click", () => openAdd(false));
  $("add-back").addEventListener("click", showPick);
  $("add-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!addingKind) return;
    $("add-submit").disabled = true; $("add-error").hidden = true;
    try {
      await api("/api/integrations", { method: "POST", body: JSON.stringify({ kind: addingKind.id, settings: formValues($("add-fields")) }) });
      openAdd(false);
      await tick();
    } catch (err) {
      $("add-error").textContent = err.message; $("add-error").hidden = false;
    } finally { $("add-submit").disabled = false; }
  });

  // Settings
  function renderSettings() {
    renderAppearance();
    renderLanguage();
    const st = S.settings;
    // Hidden until the state is in.
    $("set-foot").hidden = false;
    $("set-window-row").hidden = false;
    renderWindow();
    renderFrom("set-window", "shot_window_s", (v) => (v === 0 ? t("settings.noLimit") : clock(v * 1000) + " min"));
    renderUpdateSetting();
    // The grinder's address, in the form on the grinder card. Use start
    // setting only with a start address to go back to: without one it
    // would only remove the address.
    renderFrom("g-host", "grinder_host", (v) => v || t("settings.none"), !!(st.start && st.start.grinder_host));
    renderFrom("set-update", "update_check", (v) => (v ? t("settings.on") : t("settings.off")));
    $("set-foot").textContent = st.persistent ? t("settings.saved") : t("settings.notPersistent");
  }

  /** The line under a setting saved on the server: where its value comes
   *  from (`settings.source`), and when it was set in the app, the start
   *  setting it would go back to (`settings.start`, shown by `show`) with
   *  Use start setting, unless `canReset` is false. */
  function renderFrom(id, key, show, canReset = true) {
    const st = S.settings;
    const from = st.source && st.source[key];
    const start = st.start && key in st.start ? show(st.start[key]) : "";
    const text = from === "app" ? t("settings.from.app", { value: start })
      : from === "command_line" ? t("settings.from.start")
      : from === "default" ? t("settings.from.default") : "";
    const line = $(id + "-from-text");
    if (line.textContent !== text) line.textContent = text;
    $(id + "-from").hidden = !text;
    $(id + "-reset").hidden = from !== "app" || !st.start || !canReset;
  }

  // Time to pull the shot: − and + (saved a moment after the last tap) in
  // 15 s steps, in whole minutes above 10:00; typed into the field (saved
  // on Enter or when leaving it); or No limit, which turned off again goes
  // back to the limit before. The range is the server's (0:30 to 60:00, and
  // 0 for no limit). A shorter time from --brew-timeout-s shows as it is.
  const WIN_FINE = 600;   // − and + take whole minutes above this
  /** The shot windows the server takes besides 0 (no limit), in seconds:
   *  `settings.shot_window_range` from /api/state; null until it came. */
  function winRange() {
    const r = S && S.settings && S.settings.shot_window_range;
    return r && typeof r.min_s === "number" && typeof r.max_s === "number" ? r : null;
  }
  const win = { pending: null, saved: null, seq: 0, timer: null, msg: "", warn: false, lastLimit: 0 };
  /** The shot window as shown: the one waiting to be saved, else the one
   *  just saved (a state poll that left before the save still brings the
   *  old one, for a moment), else the server's. */
  function winValue() {
    if (win.pending != null) return win.pending;
    const v = S.settings.shot_window_s;
    if (win.saved && v !== win.saved.value && Date.now() - win.saved.at < 5000) return win.saved.value;
    win.saved = null;
    return v;
  }
  const winText = (v) => (v === 0 ? t("settings.noLimit") : clock(v * 1000));
  function renderWindow() {
    const r = winRange();
    if (!r) return;
    const v = winValue();
    const input = $("set-window");
    if (document.activeElement !== input) {
      input.value = winText(v);
      input.dataset.shown = input.value;
      $("set-window-field").classList.toggle("none", v === 0);
      $("set-window-field").classList.remove("editing");
      $("set-window-unit").hidden = v === 0;
    }
    if (v > 0) win.lastLimit = v;
    // aria-disabled rather than disabled: the button keeps the focus.
    const down = $("set-window-down"), up = $("set-window-up");
    down.setAttribute("aria-disabled", String(v !== 0 && v <= r.min_s));
    up.setAttribute("aria-disabled", String(v === 0 || v >= r.max_s));
    const less = v === 0 ? t("settings.windowToMax", { time: clock(r.max_s * 1000) }) : t("settings.windowLess", { step: stepText(v, -1) });
    const more = t("settings.windowMore", { step: stepText(v, 1) });
    if (down.title !== less) { down.title = less; down.setAttribute("aria-label", less); }
    if (up.title !== more) { up.title = more; up.setAttribute("aria-label", more); }
    $("set-window-none").setAttribute("aria-pressed", String(v === 0));
    $("set-window-range").textContent = t("settings.windowRange", { min: clock(r.min_s * 1000), max: clock(r.max_s * 1000) });
    $("set-window-field").classList.toggle("bad", win.warn && document.activeElement === input);
    const msg = $("set-window-msg");
    if (msg.textContent !== win.msg) msg.textContent = win.msg;
    msg.className = "hint" + (win.warn ? " warn" : "");
  }
  function windowNote(text, warn) { win.msg = text || ""; win.warn = !!warn; }

  /** Seconds from what was typed, or NaN. Three forms: m:ss ("2:30"), a
   *  number with a unit ("150 s", "2,5 min"), or a number alone: a whole
   *  number is minutes up to 60 and seconds above ("3" is 3:00, "150" is
   *  2:30), one with decimals is minutes ("2,5" is 2:30). 0 is no limit. */
  function parseWindow(text) {
    const s = String(text || "").trim().toLowerCase().replace(",", ".");
    let m = /^(\d{1,2}):([0-5]\d)(?:\s*min\.?)?$/.exec(s);
    if (m) return Number(m[1]) * 60 + Number(m[2]);
    m = /^(\d+(?:\.\d+)?)\s*(s|sec|sek|m|min)?\.?$/.exec(s);
    if (!m) return NaN;
    const n = Number(m[1]);
    // Decimals as typed ("60.0" too) are always minutes.
    const minutes = m[2] ? m[2].charAt(0) === "m" : n <= 60 || m[1].includes(".");
    const secs = minutes ? n * 60 : n;
    // Whole seconds, as the server takes them; a limit stays a limit.
    return secs > 0 ? Math.max(1, Math.round(secs)) : 0;
  }
  /** `secs` within the server's range, with a note when it was moved into it. */
  function clampWindow(secs) {
    const r = winRange();
    if (secs === 0 || !r) return { secs, note: "" };
    if (secs > r.max_s) return { secs: r.max_s, note: t("settings.windowMax", { time: clock(r.max_s * 1000) }) };
    if (secs < r.min_s) return { secs: r.min_s, note: t("settings.windowMin", { time: clock(r.min_s * 1000) }) };
    return { secs, note: "" };
  }
  /** What − (`dir` -1) or + (1) does from `cur`: 15 s steps up to 10:00,
   *  whole minutes above; from no limit, − gives the longest. */
  function stepUnit(cur, dir) { return (dir > 0 ? cur >= WIN_FINE : cur > WIN_FINE) ? 60 : 15; }
  function stepFrom(cur, dir, r) {
    if (cur === 0) return dir < 0 ? r.max_s : 0;
    const unit = stepUnit(cur, dir);
    const next = dir > 0 ? Math.floor(cur / unit) * unit + unit : Math.ceil(cur / unit) * unit - unit;
    return Math.min(r.max_s, Math.max(r.min_s, next));
  }
  const stepText = (cur, dir) => (stepUnit(cur, dir) === 60 ? t("settings.step1min") : t("settings.step15s"));

  /** Saves the shot window (null: the start setting); true once saved. */
  async function saveWindow(value) {
    clearTimeout(win.timer);
    win.timer = null;
    // Only the newest save's answer counts; an older one may come back later.
    const seq = ++win.seq;
    let saved = false;
    try {
      const st = await api("/api/settings", { method: "PUT", body: JSON.stringify({ shot_window_s: value }) });
      if (seq === win.seq) {
        if (st && typeof st.shot_window_s === "number") {
          S.settings = st;
          win.saved = { value: st.shot_window_s, at: Date.now() };
          // A state poll that left before this answer would bring back the
          // old source line; one more poll after it puts it right.
          tick();
        }
        if (win.warn) windowNote("");   // an earlier failure is past
        saved = true;
      }
    } catch (x) {
      if (seq === win.seq) windowNote(t("common.notSaved", { error: x.message }), true);
    } finally {
      // A newer value waits for its own save.
      if (win.pending === value) win.pending = null;
      renderAll();
    }
    return saved;
  }
  function stepWindow(dir) {
    const r = winRange();
    if (!r) return;
    const cur = winValue();
    if (dir > 0 ? cur === 0 || cur >= r.max_s : cur !== 0 && cur <= r.min_s) return;   // at the end
    const next = stepFrom(cur, dir, r);
    if (next === cur) return;
    win.pending = next;
    windowNote("");
    renderWindow();
    clearTimeout(win.timer);
    win.timer = setTimeout(() => saveWindow(next), 700);
  }
  $("set-window-down").addEventListener("click", () => stepWindow(-1));
  $("set-window-up").addEventListener("click", () => stepWindow(1));
  // No limit, saved at once; pressed again, the limit from before (else the
  // start setting's, else the longest).
  $("set-window-none").addEventListener("click", () => {
    const r = winRange();
    if (!r) return;
    const start = S.settings.start ? S.settings.start.shot_window_s : 0;
    // A flag may have set less than the app can (0:10): at least 0:30.
    const next = winValue() !== 0 ? 0 : clampWindow(win.lastLimit || start || r.max_s).secs;
    win.pending = next;
    windowNote("");
    renderWindow();
    saveWindow(next);
  });

  /** Use start setting under the setting `id`: `save` sends null and says
   *  whether it was saved. While it runs the button is aria-disabled, not
   *  disabled, so it keeps the focus; once saved, the button goes, and the
   *  focus moves to the line saying where the value comes from now, which
   *  a screen reader then reads. */
  function resetButton(id, save) {
    const b = $(id + "-reset");
    b.addEventListener("click", async () => {
      if (b.getAttribute("aria-disabled") === "true") return;
      b.setAttribute("aria-disabled", "true");
      let saved = false;
      try { saved = await save(); } finally { b.removeAttribute("aria-disabled"); }
      const at = document.activeElement;
      if (saved && b.hidden && (!at || at === b || at === document.body)) $(id + "-from-text").focus();
    });
  }
  // The server's answer says what the start setting is.
  resetButton("set-window", () => {
    clearTimeout(win.timer);
    win.pending = null;
    windowNote("");
    return saveWindow(null);
  });

  /** Takes what was typed; `leaving`: the field lost focus. Returns false
   *  when it cannot be read (the note says how to write it). */
  function commitWindow(leaving) {
    const input = $("set-window");
    const typed = input.value.trim();
    if (!typed || typed === input.dataset.shown) return true;
    const secs = parseWindow(typed);
    if (!isFinite(secs)) {
      const r = winRange() || { min_s: 0, max_s: 0 };
      windowNote(t("settings.windowInvalid", { min: clock(r.min_s * 1000), max: clock(r.max_s * 1000), decimal: fmt.upToOne.format(2.5) }), true);
      if (leaving) input.value = input.dataset.shown;
      renderWindow();
      return false;
    }
    const c = clampWindow(secs);
    windowNote(c.note);
    input.value = winText(c.secs);
    input.dataset.shown = input.value;
    if (c.secs !== winValue()) { win.pending = c.secs; saveWindow(c.secs); }
    renderWindow();
    return true;
  }
  $("set-window").addEventListener("focus", (e) => {
    // An empty field with the time as placeholder: what is typed replaces
    // it, and leaving it empty keeps it. No unit: 3 is minutes, 150 seconds.
    const input = e.target;
    $("set-window-unit").hidden = true;
    $("set-window-field").classList.remove("none");
    $("set-window-field").classList.add("editing");
    input.placeholder = winValue() === 0 ? "" : input.dataset.shown || "";
    input.value = "";
    windowNote("");
    renderWindow();
  });
  $("set-window").addEventListener("input", (e) => {
    // What the typing comes to, so "3" or "150" can be checked before it is saved.
    const typed = e.target.value.trim();
    const secs = parseWindow(typed);
    if (!typed || !isFinite(secs)) windowNote("");
    else {
      const c = clampWindow(secs);
      const is = c.secs === 0 ? t("settings.windowIsNone") : winText(c.secs) !== typed ? t("settings.windowIs", { time: winText(c.secs) }) : "";
      windowNote(c.note || is);
    }
    renderWindow();
  });
  $("set-window").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); if (commitWindow(false)) e.target.blur(); }
    else if (e.key === "Escape") { e.target.value = ""; windowNote(""); e.target.blur(); }
  });
  $("set-window").addEventListener("blur", () => { commitWindow(true); renderWindow(); });
  // Appearance: System, Light or Dark, kept per device (gbs.theme); the
  // script in <head> applies it.
  const themeChoice = () => { const v = store.get("theme"); return v === "light" || v === "dark" ? v : "system"; };
  function renderAppearance() {
    const choice = themeChoice();
    for (const b of $("appearance").querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.dataset.themeChoice === choice));
  }
  for (const b of $("appearance").querySelectorAll("button")) {
    b.addEventListener("click", () => {
      store.set("theme", b.dataset.themeChoice);
      if (window.gbsApplyTheme) window.gbsApplyTheme();
      renderAppearance();
    });
  }

  // Language: Auto, English or Deutsch, kept per device (gbs.lang); applied at once.
  function renderLanguage() {
    const choice = langChoice();
    for (const b of $("language").querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.dataset.langChoice === choice));
  }
  for (const b of $("language").querySelectorAll("button")) {
    b.addEventListener("click", () => {
      store.set("lang", b.dataset.langChoice);
      applyLanguage();
    });
  }
  // Changed in another tab of this browser: follow it here too.
  window.addEventListener("storage", (e) => {
    if (e.key === "gbs.lang") applyLanguage();
    if (e.key === "gbs.theme") renderAppearance();   // the head script applies it
  });
  /** Switches every text and format to the chosen language without a reload. */
  function applyLanguage() {
    setLanguage();
    applyStatic();
    renderLanguage();
    integSig = "";
    win.msg = "";
    setHint();
    rebuildLogLines();
    renderAll();
  }

  // Update check: on or off, saved on the server like the settings above.
  function renderUpdateSetting() {
    const on = S.settings.update_check;
    $("set-update-row").hidden = false;
    for (const b of $("set-update").querySelectorAll("button")) b.setAttribute("aria-pressed", String((b.dataset.update === "on") === on));
  }
  let updateSaving = false;
  /** Saves the update check: true, false, or null for the start setting;
   *  true once saved. */
  async function saveUpdateCheck(on) {
    if (updateSaving || !S || !S.settings) return false;
    updateSaving = true;
    $("set-update").setAttribute("aria-busy", "true");
    $("set-update-error").hidden = true;
    try {
      const st = await api("/api/settings", { method: "PUT", body: JSON.stringify({ update_check: on }) });
      if (st && "update_check" in st) S.settings = st;
      renderSettings();
      tick();
      return true;
    } catch (x) {
      $("set-update-error").textContent = t("common.notSaved", { error: x.message }); $("set-update-error").hidden = false;
      return false;
    } finally {
      updateSaving = false;
      $("set-update").removeAttribute("aria-busy");
    }
  }
  for (const b of $("set-update").querySelectorAll("button")) {
    b.addEventListener("click", () => {
      const on = b.dataset.update === "on";
      if (S && S.settings && S.settings.update_check !== on) saveUpdateCheck(on);
    });
  }
  resetButton("set-update", () => saveUpdateCheck(null));

  // The grinder's address, in the form on the grinder card (its ⋮ menu
  // opens it). `refocus` on closing puts focus back on the ⋮ button.
  function hostEditor(open, refocus) {
    const wasOpen = !$("g-host-form").hidden;
    $("g-host-form").hidden = !open;
    $("g-host-error").hidden = true;
    // Chosen again while open: what is typed stays.
    if (open && !wasOpen) $("f-host").value = (S.settings && S.settings.grinder_host) || "";
    if (open) $("f-host").focus();
    renderStatus();
    if (!open && refocus) focusKebab("grinder");
  }
  function hostError(x) { $("g-host-error").textContent = t("common.notSaved", { error: x.message }); $("g-host-error").hidden = false; }
  $("g-host-cancel").addEventListener("click", () => hostEditor(false, true));
  /** Saves the grinder's address on the server (the grinder card's form and
   *  setup step 1); the server says what it does not take. */
  async function saveHost(v) {
    const st = await api("/api/settings", { method: "PUT", body: JSON.stringify({ grinder_host: v }) });
    if (st && "grinder_host" in st) S.settings = st;
  }
  // An empty field removes the address ("" to the server, which without a
  // start address only forgets the app's one); Use start setting (null)
  // goes back to the start address.
  $("g-host-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const v = $("f-host").value.trim();
    if (v === (S.settings.grinder_host || "")) { hostEditor(false, true); return; }   // unchanged
    $("g-host-save").disabled = true; $("g-host-error").hidden = true;
    try {
      await saveHost(v);
      hostEditor(false, true);
      await tick();
    } catch (x) { hostError(x); } finally { $("g-host-save").disabled = false; }
  });
  resetButton("g-host", async () => {
    $("g-host-error").hidden = true;
    try {
      const st = await api("/api/settings", { method: "PUT", body: JSON.stringify({ grinder_host: null }) });
      if (st && "grinder_host" in st) S.settings = st;
      hostEditor(false, true);
      tick();
      return true;
    } catch (x) { hostError(x); return false; }
  });

  function renderVersion() {
    const u = S.update;
    const v = String(S.server.version || "");
    const [release, build] = v.split("+");
    $("v-installed").textContent = release ? release + (build ? " · " + build : "") : "–";
    $("v-installed").title = v;
    const badge = $("v-badge");
    const on = !!(u && S.settings && S.settings.update_check);
    const failing = on && !!u.failing;
    // A known newer release stays news when GitHub fails later; "up to date"
    // does not.
    const word = !on ? null : u.available ? ["run", t("version.updateAvailable")]
      : failing ? ["none", t("version.failing")] : u.latest ? ["ok", t("version.upToDate")] : null;
    badge.hidden = !word;
    if (word) { setTone(badge, word[0]); badge.textContent = word[1]; }
    $("v-latest-box").hidden = !on;
    $("v-latest").textContent = !on ? "–" : u.latest || (failing ? "–" : t("version.notChecked"));
    $("v-note").hidden = !on;
    $("v-off").hidden = !(u && !on);
    $("v-whatsnew").hidden = !(on && u.available);
  }

  function renderStatus() {
    if (!S || tab !== "status") return;
    renderGrinder();
    renderIntegrations();
    renderSettings();
    renderVersion();
  }

  // --- History tab -----------------------------------------------------------------
  const HKINDS = ["succeeded", "aborted", "failed"];
  const kindLabel = (k) => (HKINDS.includes(k) ? t("history.kind." + k) : k);
  let hHidden = {};
  try { hHidden = JSON.parse(store.get("historyHidden") || "{}") || {}; } catch (_) {}
  let histLoading = false, histTried = 0, histError = "";

  async function loadHistory() {
    if (histLoading) return;
    histLoading = true;
    histTried = Date.now();
    try {
      hist = await api("/api/history");
      histState = "ok";
    } catch (e) {
      // Tried again shortly (see refresh).
      histState = "failed"; histError = e.message;
    }
    histLoading = false;
    renderHistory();
  }

  /** "Today · 11 Oct", "Yesterday · 10 Oct", "Thursday · 9 Oct". */
  function dayLabel(d) {
    const today = new Date(); today.setHours(0, 0, 0, 0);
    const that = new Date(d); that.setHours(0, 0, 0, 0);
    const diff = Math.round((today - that) / 86400000);
    const date = (d.getFullYear() !== today.getFullYear() ? fmt.dayYear : fmt.day).format(d);
    const name = diff === 0 ? t("history.today") : diff === 1 ? t("history.yesterday") : fmt.weekday.format(d);
    return name + " · " + date;
  }

  function shotTitle(s) {
    if (s.outcome === "succeeded") {
      if (s.time_ms == null) return "–";
      const time = secs(s.time_ms / 1000);
      return s.weight_g != null ? t("unit.shot", { time, weight: grams(s.weight_g) }) : t("unit.seconds", { time });
    }
    const cause = causeOf(s);
    if (cause === "shot_timed_out") {
      const ms = Date.parse(s.ended_at) - Date.parse(s.started_at);
      return t("history.timeout", { time: isFinite(ms) ? clock(ms) : windowLabel() });
    }
    const title = causeInfo(cause).title;
    if (title) return t(title);
    // A cause this app does not know: as the server says it.
    return s.reason ? cap(s.reason) : kindLabel(s.outcome);
  }
  function shotSub(s) {
    const parts = [];
    const tgt = [s.target_time_ms != null ? t("unit.seconds", { time: secs(s.target_time_ms / 1000) }) : null,
      s.target_weight_g != null ? t("unit.grams", { weight: gramsShort(s.target_weight_g) }) : null].filter(Boolean);
    if (tgt.length) parts.push(t("history.target", { targets: tgt.join(" · ") }));
    return parts.join(" · ");
  }
  /** A success the grinder did not adjust from, as the server recorded it.
   *  Without a record the current window may not be the shot's: a success. */
  const ignoredShot = (s) => s.outcome === "succeeded" && s.in_grinder_range === false;

  // Until it syncs, a computer's clock may still read the last shutdown
  // time, or 1970 or the system's build date. A shot dated before the shot
  // history existed (2026) was recorded in one of the last two cases: its
  // day and time are not known. One dated by a last shutdown time looks
  // right and is not caught here.
  const CLOCK_SET_MS = Date.UTC(2026, 0, 1);
  /** When the shot started, or null when that is not known. */
  function shotDate(s) {
    const d = new Date(s.started_at);
    return isNaN(d) || d.getTime() < CLOCK_SET_MS ? null : d;
  }

  function renderHistory() {
    if (tab !== "history") return;
    const shots = (hist && hist.shots) || [];
    const chips = $("h-chips"), groups = $("h-groups"), empty = $("h-empty"), foot = $("h-foot");
    chips.hidden = !hist;
    if (!hist) {
      groups.replaceChildren(); empty.hidden = false; foot.hidden = true;
      empty.textContent = histState === "failed" ? t("history.failed", { error: histError }) : t("history.loading");
      return;
    }
    chips.replaceChildren(...HKINDS.map((k) => {
      const on = !hHidden[k];
      const n = shots.filter((s) => s.outcome === k).length;
      return el("button", {
        type: "button", class: "chip-hit k-" + k, "aria-pressed": String(on),
        "aria-label": t("history.chip", { label: kindLabel(k), n }),
        onclick: () => { hHidden[k] = on; store.set("historyHidden", JSON.stringify(hHidden)); renderHistory(); },
      }, el("span", { class: "chip" }, kindLabel(k), el("span", { class: "chip-count", text: fmt.int.format(n) })));
    }));
    // Newest first by the time shown, so a shot dated wrongly (the clock
    // was not set, or stepped) cannot split the days; those without a
    // known day come last, newest recorded first.
    const time = (x) => (x.d ? x.d.getTime() : 0);
    const visible = shots.filter((s) => !hHidden[s.outcome])
      .map((s, i) => ({ s, d: shotDate(s), i }))
      .sort((a, b) => time(b) - time(a) || a.i - b.i);
    const byDay = new Map();
    for (const x of visible) {
      const label = x.d ? dayLabel(x.d) : t("history.unknownDay");
      if (!byDay.has(label)) byDay.set(label, { label, items: [] });
      byDay.get(label).items.push(x);
    }
    const days = [...byDay.values()];
    groups.replaceChildren(...days.map((g) => el("section", { class: "day" },
      el("h2", { text: g.label }),
      el("ul", null, g.items.map(({ s, d }) => {
        const sub = shotSub(s);
        const ignored = ignoredShot(s);
        return el("li", { class: "k-" + (ignored ? "ignored" : s.outcome) },
          el("p", { class: "h-time", text: d ? hhmm(d) : "–" }),
          el("div", { class: "h-main" }, el("p", { class: "h-title", text: shotTitle(s) }), sub ? el("p", { class: "h-sub", text: sub }) : null),
          el("span", { class: "h-badge", text: ignored ? t("history.ignored") : kindLabel(s.outcome) }));
      })))));
    empty.hidden = visible.length > 0;
    empty.textContent = shots.length === 0 ? t("history.empty") : t("history.noMatch");
    foot.hidden = false;
    const kept = { n: hist.capacity || 200 };
    foot.textContent = hist.persistent === false ? t("history.keptMemory", kept) : t("history.kept", kept);
  }

  // --- Log ---------------------------------------------------------------------
  const log = { open: false, lines: [], last: 0, state: "idle", es: null, poll: null, retry: null, connected: false, follow: true, warnOnly: false, capacity: 1000, done: null, doneTimer: null };
  const LOG_NOTE = () => t("log.note", { n: log.capacity });

  /** The server restarted (a new boot_id, or its line numbers went back): start over from its first line. */
  function restartLog() {
    stopLogFeed();
    log.lines = []; log.last = 0;
    $("log-lines").replaceChildren();
    if (log.state === "live") { log.state = "idle"; if (logShown()) openLog(); }
  }

  /** One log line on the page: time in the language's format, level (lower case from the server, shown in capitals), text as the server wrote it. */
  function lineEl(l) {
    const d = new Date(l.at);
    const level = String(l.level || "info");
    return el("div", { class: "ll l-" + level },
      el("span", { class: "t", text: isNaN(d) ? "--:--:--" : hhmmss(d) }),
      el("span", { class: "v", text: level.toUpperCase() }),
      el("span", { class: "x", lang: "en", text: String(l.text || "") }));
  }
  function rebuildLogLines() {
    $("log-lines").replaceChildren(...log.lines.map(lineEl));
    if (log.follow) scrollLogEnd();
  }
  function addLines(lines) {
    const box = $("log-lines");
    const frag = document.createDocumentFragment();
    for (const l of lines) {
      if (!l || typeof l.seq !== "number" || l.seq <= log.last) continue;
      log.last = l.seq;
      log.lines.push(l);
      frag.append(lineEl(l));
    }
    box.append(frag);
    const over = log.lines.length - log.capacity;
    if (over > 0) {
      log.lines.splice(0, over);
      for (let i = 0; i < over && box.firstChild; i++) box.firstChild.remove();
    }
    if (log.follow) scrollLogEnd();
  }

  async function fetchLog() {
    try {
      const r = await api("/api/log?after=" + log.last);
      if (typeof r.capacity === "number") log.capacity = r.capacity;
      if (typeof r.last_seq === "number" && r.last_seq < log.last) { restartLog(); return false; }
      addLines(r.lines);
      return true;
    } catch (_) {
      return false;
    }
  }

  function stopLogFeed() {
    if (log.es) { log.es.close(); log.es = null; }
    if (log.poll) { clearInterval(log.poll); log.poll = null; }
    clearTimeout(log.retry);
    log.connected = false;
  }

  async function openLog() {
    renderLog();
    if (log.state !== "idle") { if (log.follow) scrollLogEnd(); return; }
    if (!S) return;
    log.state = "live";
    const ok = await fetchLog();
    if (!ok || log.state !== "live") return;
    renderLog();
    if (window.EventSource) connectLog(); else startLogPolling();
  }

  // The stream first replays the kept lines after `after`, so none fall
  // between the fetch and the stream. It is reopened by hand after an error,
  // with the newest `after` and after a fetch that notices a server restart.
  function connectLog() {
    if (log.state !== "live" || log.es) return;
    const es = new EventSource("/api/log/stream?after=" + log.last);
    log.es = es;
    es.addEventListener("open", () => { log.connected = true; renderLog(); });
    es.addEventListener("log", (m) => { try { addLines([JSON.parse(m.data)]); } catch (_) {} });
    es.addEventListener("error", () => {
      es.close();
      if (log.es === es) log.es = null;
      log.connected = false;
      renderLog();
      clearTimeout(log.retry);
      log.retry = setTimeout(reconnectLog, 3000);
    });
  }
  async function reconnectLog() {
    if (log.state !== "live" || log.es) return;
    const ok = await fetchLog();
    if (log.state !== "live" || log.es) return;   // a restart: openLog took over
    if (ok) connectLog(); else log.retry = setTimeout(reconnectLog, 3000);
  }
  function startLogPolling() {
    if (log.poll) return;
    log.poll = setInterval(() => { if (!document.hidden && logShown()) fetchLog(); }, 2000);
  }

  /** Brings the log card's head just below the sticky header. */
  function scrollToLog() {
    const head = document.querySelector(".top").offsetHeight;
    window.scrollTo({ top: $("log-card").getBoundingClientRect().top + window.scrollY - head - 8 });
  }
  /** Whether the log card is open on the visible Status tab. */
  const logShown = () => tab === "status" && log.open;

  function renderLog() {
    if (tab !== "status") return;
    $("log-toggle").setAttribute("aria-expanded", String(log.open));
    $("log-body").hidden = !log.open;
    if (!log.open) return;
    $("log-follow").setAttribute("aria-pressed", String(log.follow));
    $("log-warn").setAttribute("aria-pressed", String(log.warnOnly));
    $("log-lines").classList.toggle("warn-only", log.warnOnly);
    const live = window.EventSource ? log.connected : log.state === "live" && online;
    $("log-live-block").classList.toggle("on", log.follow && live);
    $("log-live-text").textContent = !live ? t("log.reconnecting") : log.follow ? t("log.live") : t("log.paused");
    // A status region announces every change, so only real ones.
    if (!log.done && $("log-note").textContent !== LOG_NOTE()) $("log-note").textContent = LOG_NOTE();
    if (!log.done) $("log-note").classList.remove("done");
    $("log-copy").classList.toggle("done", log.done === "copy");
    $("log-dl").classList.toggle("done", log.done === "dl");
  }

  let logScrolling = false;
  function scrollLogEnd() {
    const box = $("log-box");
    logScrolling = true;
    box.scrollTop = box.scrollHeight;
    requestAnimationFrame(() => { logScrolling = false; });
  }
  $("log-box").addEventListener("scroll", () => {
    if (logScrolling) return;
    const box = $("log-box");
    const atEnd = box.scrollHeight - box.scrollTop - box.clientHeight < 8;
    if (!atEnd && log.follow) { log.follow = false; renderLog(); }
  }, { passive: true });
  // The box changes height with the note below it and the screen: keep the end in view.
  if (window.ResizeObserver) new ResizeObserver(() => { if (log.follow && logShown()) scrollLogEnd(); }).observe($("log-box"));
  // Opened by hand, or from "Show the log" next to an error.
  $("log-toggle").addEventListener("click", () => {
    log.open = !log.open;
    if (log.open) { openLog(); return; }
    // Closed: no stream until it opens again, which catches up from `log.last`,
    // and a reload doesn't open it again.
    if (log.state === "live") { stopLogFeed(); log.state = "idle"; }
    if (location.hash === "#log") history.replaceState(null, "", "#status");
    renderLog();
  });
  $("sys-setup-link").addEventListener("click", (e) => {
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
    e.preventDefault();
    openSetup("setup");
  });
  $("sys-log-link").addEventListener("click", (e) => {
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
    e.preventDefault();
    goTab("log");
  });
  $("log-follow").addEventListener("click", () => {
    log.follow = !log.follow;
    if (log.follow) scrollLogEnd();
    renderLog();
  });
  $("log-warn").addEventListener("click", () => {
    log.warnOnly = !log.warnOnly;
    renderLog();
    if (log.follow) scrollLogEnd();
  });

  // Copy and Download share the export: IP addresses replaced the same way
  // throughout, and a header line saying what it holds.
  function scrubber() {
    const seen = new Map();
    const name = (ip) => {
      const key = ip.toLowerCase();
      if (!seen.has(key)) seen.set(key, "<ip-" + (seen.size + 1) + ">");
      return seen.get(key);
    };
    // The IPv4 tail goes first, so ::ffff:192.168.1.30 is one address.
    const V6 = /(^|[^0-9A-Za-z_:.])((?:[0-9A-Fa-f]{0,4}:){2,7}(?:\d{1,3}(?:\.\d{1,3}){3}|[0-9A-Fa-f]{1,4})?)(?=$|[^0-9A-Za-z_:])/g;
    const V4 = /(^|[^0-9.])((?:25[0-5]|2[0-4]\d|[01]?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|[01]?\d?\d)){3})(?![0-9]|\.\d)/g;
    return (text) => text
      .replace(/\/\/[^\s\/@]+@/g, "//<login>@")   // user:password@ in a URL
      .replace(V6, (all, pre, ip) => {
        const colons = (ip.match(/:/g) || []).length;
        const valid = /[0-9A-Fa-f]/.test(ip) && (ip.includes("::") ? !/:::/.test(ip) && ip.split("::").length === 2 : colons === 7 || (colons === 6 && /\d\.\d/.test(ip)));
        return valid ? pre + name(ip) : all;
      })
      .replace(V4, (all, pre, ip) => pre + name(ip));
  }
  // The export is meant for bug reports: always English, ISO-like times.
  function stamp(d) {
    return d.getFullYear() + "-" + pad2(d.getMonth() + 1) + "-" + pad2(d.getDate())
      + " " + pad2(d.getHours()) + ":" + pad2(d.getMinutes()) + ":" + pad2(d.getSeconds());
  }
  function utcOffset(d) {
    const m = -d.getTimezoneOffset();
    return "UTC" + (m >= 0 ? "+" : "-") + pad2(Math.floor(Math.abs(m) / 60)) + ":" + pad2(Math.abs(m) % 60);
  }
  function exportText(lines, filter) {
    const scrub = scrubber();
    const body = lines.map((l) => {
      const d = new Date(l.at);
      return (isNaN(d) ? String(l.at) : stamp(d) + "." + String(d.getMilliseconds()).padStart(3, "0")) + " "
        + String(l.level).toUpperCase().padEnd(5) + " " + scrub(String(l.text || ""));
    });
    const first = lines.length ? new Date(lines[0].at) : new Date();
    const last = lines.length ? new Date(lines[lines.length - 1].at) : new Date();
    const head = "# gbs-anywhere " + ((S && S.server.version) || "") + " log · " + stamp(first) + " to " + stamp(last)
      + " (" + utcOffset(last) + ") · " + lines.length + (lines.length === 1 ? " line" : " lines") + " · " + filter + " · IP addresses and passwords in addresses replaced";
    return head + "\n" + body.join("\n") + "\n";
  }
  /** Shows `note` under the log for a moment; `kind` "copy" or "dl" also ticks that button. */
  function logDone(kind, note) {
    log.done = kind;
    $("log-note").textContent = note;
    $("log-note").classList.toggle("done", kind === "copy" || kind === "dl");
    clearTimeout(log.doneTimer);
    log.doneTimer = setTimeout(() => { log.done = null; renderLog(); }, 3500);
    renderLog();
  }

  async function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      try { await navigator.clipboard.writeText(text); return true; } catch (_) {}
    }
    // Plain HTTP on a LAN address: no clipboard API, so a hidden textarea.
    const ta = el("textarea", { readonly: true, "aria-hidden": "true" });
    ta.value = text;
    ta.style.cssText = "position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;font-size:16px;";
    document.body.append(ta);
    const active = document.activeElement;
    ta.focus(); ta.select();
    try { ta.setSelectionRange(0, text.length); } catch (_) {}
    let ok = false;
    try { ok = document.execCommand("copy"); } catch (_) {}
    ta.remove();
    if (active && active.focus) active.focus();
    return ok;
  }

  $("log-copy").addEventListener("click", async () => {
    const since = Date.now() - 60 * 60000;
    let lines = log.lines.filter((l) => Date.parse(l.at) >= since && (!log.warnOnly || l.level !== "info"));
    if (lines.length > 500) lines = lines.slice(-500);
    if (!lines.length) {
      logDone("note", log.warnOnly ? t("log.nothingWarnings") : t("log.nothingLines"));
      return;
    }
    const filter = log.warnOnly ? "warnings only" : "all levels";
    const text = exportText(lines, filter);
    const note = log.warnOnly ? t("log.copiedWarn", { n: lines.length }) : t("log.copied", { n: lines.length });
    if (await copyText(text)) { logDone("copy", note); return; }
    // Copying is blocked: show the text to copy by hand.
    const ta = el("textarea", { readonly: true, "aria-label": t("log.copySheet.label") });
    ta.value = text;
    openSheet({
      title: t("log.copySheet.title"),
      body: [el("p", { class: "body-text", text: t("log.copySheet.text") }), ta],
      actions: [
        { label: t("log.copySheet.selectAll"), cls: "s-plain", run: () => { ta.focus(); ta.select(); try { ta.setSelectionRange(0, ta.value.length); } catch (_) {} } },
        { label: t("common.done"), cls: "s-primary", run: closeSheet },
      ],
      focus: ta,
    });
    requestAnimationFrame(() => { ta.select(); try { ta.setSelectionRange(0, ta.value.length); } catch (_) {} });
  });

  $("log-dl").addEventListener("click", () => {
    const lines = log.lines.slice();
    const now = new Date();
    const name = "gbs-anywhere-log-" + now.getFullYear() + "-" + pad2(now.getMonth() + 1) + "-" + pad2(now.getDate())
      + "-" + pad2(now.getHours()) + pad2(now.getMinutes()) + ".txt";
    const url = URL.createObjectURL(new Blob([exportText(lines, "all levels")], { type: "text/plain;charset=utf-8" }));
    const a = el("a", { href: url, download: name });
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 10000);
    logDone("dl", t("log.downloaded", { file: name, n: lines.length }));
  });

  // --- Setup -------------------------------------------------------------------------
  // Setting up a grinder step by step, in a view of its own. It starts by
  // asking which grinder it is, unless the app can tell; then each grinder
  // has steps of its own: an E64 WS is told where gbs-anywhere is and polls
  // it, an E65/E80 GbS is paired. Which step it is comes from the server
  // (grinder.wifi and the E64 WS's polls in /api/state) wherever it can, so
  // a reload lands on the same one. What the person says about the
  // grinder's screen (Enabled, Pairing started, shown, Cleared) is kept
  // here, and in this tab's sessionStorage until the setup closes or
  // finishes, so a reload goes on from it too.
  //
  // The view (header, cards, progress, focus, what is announced) is shared
  // here. The steps are a "flow" per grinder, below: E64_FLOW and
  // PAIR_FLOW, each with its own state in `wz` (wz.e64, wz.pair) and the
  // same parts (see SETUP_FLOWS). A change to one grinder's steps stays in
  // its flow.

  /** The setup's state on this page, fresh for `mode` ("setup" or "repair",
   *  an E65/E80 re-pair): the view's, and each flow's. */
  function freshSetup(mode) {
    return {
      mode,
      kind: null,           // the grinder the person chose: "e64" or "wifi" (see setupKind)
      open: {}, at: "", sig: "", focus: null, say: "", reveal: null,
      e64: freshE64(),
      pair: freshPair(),
    };
  }
  let wz = freshSetup("setup");
  let setupFrom = "start";     // the tab the close button goes back to

  // The person's answers, kept across a reload of this tab (sessionStorage
  // "gbs.setup"): the mode, the choice of grinder and what each flow keeps
  // (its `kept`). The rest comes from the server again.
  let setupKept = null;   // what was last written: only a change is written
  function keepSetup() {
    const pick = (o, keys) => Object.fromEntries(keys.map((k) => [k, o[k]]));
    const v = JSON.stringify({ mode: wz.mode, kind: wz.kind, e64: pick(wz.e64, E64_FLOW.kept), pair: pick(wz.pair, PAIR_FLOW.kept) });
    if (v === setupKept) return;
    setupKept = v;
    try { sessionStorage.setItem("gbs.setup", v); } catch (_) {}
  }
  function forgetSetup() {
    setupKept = null;
    try { sessionStorage.removeItem("gbs.setup"); } catch (_) {}
  }
  /** The setup as the person left it before a reload, for `mode`, or null.
   *  A re-pair goes on as #setup once the old pairing is cleared. */
  function restoredSetup(mode) {
    let v = null;
    try { v = JSON.parse(sessionStorage.getItem("gbs.setup") || "null"); } catch (_) {}
    if (!v || typeof v !== "object" || !v.e64 || !v.pair) return null;
    if (v.mode !== mode && !(v.mode === "repair" && mode === "setup")) return null;
    const fresh = freshSetup(v.mode);
    if (v.kind === "e64" || v.kind === "wifi") fresh.kind = v.kind;
    for (const [part, flow] of [["e64", E64_FLOW], ["pair", PAIR_FLOW]]) {
      for (const k of flow.kept) {
        const was = v[part][k], into = fresh[part];
        // lastCode starts as null and is a string once set.
        if (typeof was === typeof into[k] || (into[k] === null && typeof was === "string")) into[k] = was;
      }
    }
    // A kept code is a string or nothing (the loop lets any object through
    // for a field that starts as null).
    if (typeof fresh.pair.lastCode !== "string") fresh.pair.lastCode = null;
    // The hash moves to #setup only once the old pairing is cleared.
    if (v.mode === "repair" && mode === "setup") fresh.pair.cleared = true;
    return fresh;
  }

  /** The E65/E80 link has a pairing (whether or not it reaches the grinder now). */
  const wifiPaired = () => { const w = wifiOf(); return !!(w && w.pairing_id != null); };
  /** An E65/E80 is set up but not paired: the way into the setup shows. */
  const setupNeeded = () => wifiSetup() && !wifiPaired();
  /** No grinder known yet: no E65/E80 address, and no E64 WS polled since
   *  gbs-anywhere started. An E64 WS that is off looks the same, so the
   *  setup asks which grinder it is. */
  const noGrinderYet = () => !!S && !wifiSetup() && S.grinder.last_poll_age_ms == null;
  const setupOffered = () => setupNeeded() || noGrinderYet();
  /** The grinder the setup is for: "e64", "wifi" (E65/E80) or null while
   *  the person has not said and the app can't tell. Re-pairing and an
   *  address are an E65/E80, also over a choice of E64 WS (an address may
   *  be saved from another device meanwhile); an E64 WS that polled since
   *  gbs-anywhere started is one. */
  function setupKind() {
    if (wz.mode === "repair" || wifiSetup()) return "wifi";
    if (wz.kind) return wz.kind;
    if (S && S.grinder.last_poll_age_ms != null) return "e64";
    return null;
  }
  /** The steps of the grinder the setup is for, or null while it asks. */
  const setupFlow = () => SETUP_FLOWS[setupKind()] || null;

  /** Opens the setup from the app; `mode` "repair" starts with clearing the
   *  pairing on the grinder. */
  function openSetup(mode) {
    if (tab !== "setup") setupFrom = tab;
    // showTab then starts it afresh, with nothing kept to restore.
    forgetSetup();
    goTab(mode === "repair" ? "repair" : "setup");
  }
  function closeSetup() { goTab(setupFrom); }

  const SETUP_TONE = { done: "done", active: "start", wait: "run", retry: "warn", later: "none" };
  const SETUP_BADGE = { done: "done", active: "now", wait: "wait", retry: "retry", later: "later" };
  /** A card's head as most steps have it: "Step n", its title, open by
   *  itself while it is the one to act on or wait for. */
  const stepHead = (no, title, st) => ({
    kicker: t("setup.step", { n: no }), title: t(title), mark: null, no, auto: st === "active" || st === "wait" || st === "retry",
  });
  /** What tells one shown step from another: its step, part and whether a
   *  Pairing Code shows. */
  const setupKey = (at) => at.step + ":" + (at.part || "") + ":" + (at.code ? 1 : 0);

  // Building blocks, after the design.
  const answer = (act, cls, ic, label, small, disabled) => el("button", { type: "button", class: "wz-btn " + cls, "data-act": act, disabled: !!disabled },
    ic ? icon(ic, 16, 2.5) : null, el("span", null, label, small ? el("small", { text: small }) : null));
  const answers = (left, right) => el("div", { class: "wz-pair" }, left, right);
  const question = (text) => el("p", { class: "wz-question", text });
  const hint = (text, cls) => el("p", { class: "hint" + (cls ? " " + cls : ""), text });
  function crumbs(labels) {
    const items = [];
    labels.forEach((label, i) => {
      if (i) items.push(el("span", { class: "wz-sep" }, icon("arrow", 14, 2.2)));
      items.push(el("span", { class: "wz-crumb" + (i === labels.length - 1 ? " key" : ""), text: label }));
    });
    return el("div", { class: "wz-path" }, items);
  }
  /** The parts of a step: `s` done, now or next; only the current one shows
   *  its `body`. */
  function parts(items) {
    return el("ol", { class: "wz-steps" }, items.map((it) => {
      const sr = it.s === "done" ? t("setup.doneSr") : it.s === "next" ? t("setup.nextSr") : "";
      return el("li", { class: "wz-si " + it.s },
        el("span", { class: "wz-rail" }, el("span", { class: "wz-dot" }, it.s === "done" ? icon("check", 12, 3) : null)),
        el("div", { class: "wz-si-body" + (it.s === "now" && wz.reveal === it.id ? " wz-reveal" : "") },
          el("p", { class: "wz-si-t", id: it.id, tabindex: "-1" }, it.title, sr ? el("span", { class: "sr", text: ", " + sr }) : null),
          it.s === "now" ? it.body : null));
    }));
  }
  // Notes and errors are announced once, through the setup's status region
  // (setupSay), not as live regions of their own: the steps are rebuilt
  // when something changes, which would announce them again.
  function banner(cls, text, ic) {
    return el("div", { class: "banner " + cls + (ic ? " wz-banner-icon" : "") }, ic ? icon(ic, 18, 2.2) : null, el("span", { text }));
  }
  function waiting(text) {
    return el("div", { class: "wz-wait" }, el("span", { class: "wz-spinner" }), el("p", { class: "body-text", text }));
  }
  const errorLine = (id, text) => el("p", { class: "hint warn", id, text });
  /** "Text\nmore" with the line break kept. */
  const lines = (text) => text.split("\n").flatMap((line, i) => (i ? [el("br"), line] : [line]));

  /** The chevron of a card head, turned while it is open. */
  function chevron() {
    const c = icon("down", 20, 2);
    c.setAttribute("class", "chev");
    return c;
  }

  /** Says `text` once through the setup's status region. */
  function setupSay(text) { if (text) wz.say = text; }

  /** A part is done: the next one shows, gets the focus, and is announced. */
  function setupNext(nextId, doneTitle) {
    wz.focus = nextId;
    wz.reveal = nextId;
    wz.say = doneTitle + ", " + t("setup.doneSr");
  }

  // The choice of grinder: a card of the view's own, first while it asks,
  // then on top (when the person chose here, not when the app could tell).
  const KIND_CARD = {
    head: (st) => ({ kicker: t("setup.kind.kicker"), title: t("setup.kind.title"), mark: icon("grinder", 16, 2), no: null, auto: st === "active" }),
    sub: (st) => (st === "done" ? t("setup.kind." + setupKind()) : ""),
    body(st) {
      if (st === "done") {
        // Once an E65/E80 address is saved, that is the grinder: removing
        // the address (grinder card, ⋮) is the way back.
        const change = setupKind() === "e64" || !wifiSetup();
        return [el("p", { class: "body-text", text: t("setup.kind.chosen", { grinder: t("setup.kind." + setupKind()) }) }),
          change ? el("button", { type: "button", class: "wz-btn plain wide", "data-act": "kindChange" }, icon("back", 16, 2.5), t("setup.kind.change")) : null];
      }
      return [el("p", { class: "body-text", text: t("setup.kind.text") }),
        answers(answer("kindE64", "ok", null, t("setup.kind.e64"), t("setup.kind.e64Sub")),
          answer("kindWifi", "ok", null, t("setup.kind.wifi"), t("setup.kind.wifiSub")))];
    },
  };

  function renderSetup() {
    if (tab !== "setup" || !S) return;
    // An address saved meanwhile makes it an E65/E80, and the card on top
    // says so.
    if (wz.kind === "e64" && wifiSetup()) wz.kind = "wifi";
    const F = setupFlow();
    let at;
    if (F) at = F.sync();
    else {
      at = { step: "kind" };
      if (setupKey(at) !== wz.at) { wz.at = setupKey(at); wz.open = {}; }
    }
    // Finished: nothing to go on with after a reload.
    if (at.part === "done") forgetSetup(); else keepSetup();
    // The app's own pill is not shown here, so this one says when
    // gbs-anywhere does not answer.
    const [pillKey, pillTone, pulse] = !online ? [null, "error", false] : F ? F.pill(at) : ["choose", "none", false];
    const pill = $("setup-pill");
    setTone(pill, pillTone);
    pill.classList.toggle("pulse", pulse);
    const pillText = pillKey ? t("setup.pill." + pillKey) : t("sys.label.offline");
    if ($("setup-pill-text").textContent !== pillText) $("setup-pill-text").textContent = pillText;
    const cards = F ? [...(wz.kind ? ["kind"] : []), ...F.cards()] : ["kind"];
    const states = cards.map((n) => (n === "kind" ? (F ? "done" : "active") : F.card(n, at)));
    // The bars and "Step n of …" count the numbered steps.
    const steps = F ? F.cards().filter((n) => F.number(n) != null) : [];
    const progress = !F ? t("setup.pill.choose") : F.progress(at) || t("setup.progress.step", { n: F.number(at.step), total: steps.length });
    if ($("wz-progress").textContent !== progress) $("wz-progress").textContent = progress;

    // Rebuilt only when something shown changes, so a field keeps what is
    // typed and the focus stays put between polls.
    const sig = JSON.stringify([lang, wz.kind, at, cards, states, wz.open, F ? F.sig(at) : null]);
    if (sig !== wz.sig) {
      wz.sig = sig;
      $("wz-bars").replaceChildren(...steps.map((n) => el("span", { class: "wz-bar " + F.card(n, at) })));
      const active = document.activeElement;
      const keep = active && $("wz-steps").contains(active) && active.id ? active.id : null;
      $("wz-steps").replaceChildren(
        ...(F ? F.top(at) : []),
        ...cards.map((n, i) => setupCardEl(F, n, states[i], at)),
        ...(F ? F.end(at) : []));
      wz.reveal = null;
      if (!wz.focus) wz.focus = keep;
    }
    // Where an answer moves the focus to (or back where it was, after a
    // rebuild); a field keeps its caret at the end.
    const target = wz.focus && $(wz.focus);
    wz.focus = null;
    if (target && document.activeElement !== target) {
      target.focus();
      if (target.tagName === "INPUT") { const n = target.value.length; try { target.setSelectionRange(n, n); } catch (_) {} }
    }
    // A status region announces every change, so only real ones.
    if (wz.say) {
      const say = wz.say;
      wz.say = "";
      $("wz-live").textContent = "";
      setTimeout(() => { $("wz-live").textContent = say; }, 30);
    }
  }

  /** One card of flow `F` (or the choice of grinder): number or mark, title,
   *  status and the short info; its body while open. */
  function setupCardEl(F, n, st, at) {
    const body = n === "kind" ? KIND_CARD.body(st) : F.body(n, st, at);
    const head = n === "kind" ? KIND_CARD.head(st) : F.head(n, st, at);
    const open = !!body && (wz.open[n] != null ? wz.open[n] : head.auto);
    const sub = st === "later" ? "" : n === "kind" ? KIND_CARD.sub(st) : F.sub(n, at);
    const tone = SETUP_TONE[st];
    const num = st === "done" ? icon("check", 16, 2.6) : head.mark || (st === "retry" ? "!" : String(head.no));
    const bodyId = "wz-body-" + n;
    return el("section", { class: "card wz-card t-" + tone + (st === "later" ? " later" : st !== "done" ? " current" : "") },
      el("button", { type: "button", class: "wz-head", id: "wz-head-" + n, "data-toggle": String(n), "aria-expanded": String(open),
        "aria-controls": open ? bodyId : null, disabled: st === "later" || !body },
        el("span", { class: "num" }, num),
        el("span", { class: "head-col" },
          el("span", { class: "kicker", text: head.kicker }),
          el("span", { class: "step-title", text: head.title })),
        el("span", { class: "badge", text: t("setup.badge." + SETUP_BADGE[st]) }),
        chevron(),
        sub ? el("span", { class: "meta" + (open ? " ghost" : ""), "aria-hidden": open ? "true" : null, text: sub }) : null),
      open ? el("div", { class: "wz-body", id: bodyId }, body) : null);
  }

  async function setupAction(act) {
    switch (act) {
      case "close": closeSetup(); return;
      case "finish": goTab("start"); return;
      case "kindE64":
        Object.assign(wz, { kind: "e64", e64: freshE64(), focus: "wz-head-x1" });
        setupSay(t("setup.kind.chosen", { grinder: t("setup.kind.e64") }));
        break;
      case "kindWifi":
        Object.assign(wz, { kind: "wifi", focus: "wz-host" });
        setupSay(t("setup.kind.chosen", { grinder: t("setup.kind.wifi") }));
        break;
      case "kindChange": Object.assign(wz, { kind: null, e64: freshE64(), focus: "wz-head-kind" }); break;
      default: {
        // The rest are the flow's; false when it rendered by itself, or
        // does not know the action.
        const F = setupFlow();
        if (!F || !(await F.act(act))) return;
      }
    }
    renderSetup();
  }

  // Every second while the setup is open (see the poll below): a flow's
  // clock, such as the E65/E80's restart wait.
  function renderSetupTimer() {
    if (tab !== "setup") return;
    const F = setupFlow();
    if (F && F.tick) F.tick();
  }

  // --- Setup: E64 WS -------------------------------------------------------------------
  // It is told where gbs-anywhere is and polls it: enable Xenia, enter
  // gbs-anywhere's address, wait for its first poll.

  /** The E64 WS's part of the setup state. */
  function freshE64() {
    return {
      x1: false,            // Xenia enabled
      x2: false,            // gbs-anywhere's address entered on it
    };
  }
  /** The E64 WS's setup is done: it polled since gbs-anywhere started, as
   *  the server says, so a pause in its polls, the grinder switched off or
   *  a reload doesn't take back the finished setup. Not with an E65/E80
   *  link, which polls too. */
  const e64Done = () => !wifiOf() && !!S && S.grinder.last_poll_age_ms != null;
  /** The address the page was opened at, when it is an IPv4 address the
   *  grinder can reach: the one to enter on the E64 WS. A name is left out,
   *  as an IP is more reliable on the grinder, and so are loopback and
   *  link-local addresses. */
  function pageIp() {
    const h = location.hostname;
    if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(h)) return "";
    return /^(127\.|0\.|169\.254\.)/.test(h) ? "" : h;
  }
  const E64_TITLE = { x1: "setup.x1.title", x2: "setup.x2.title", x3: "setup.x3.title" };
  const E64_ORDER = { x1: 1, x2: 2, x3: 3 };

  const E64_FLOW = {
    kept: ["x1", "x2"],
    cards: () => ["x1", "x2", "x3"],
    number: (n) => E64_ORDER[n],
    /** Where it is: `step` x1 to x3, with `part` done once the grinder polls. */
    at() {
      if (e64Done()) return { step: "x3", part: "done" };
      return { step: !wz.e64.x1 ? "x1" : !wz.e64.x2 ? "x2" : "x3" };
    },
    sync() {
      const at = E64_FLOW.at();
      const key = setupKey(at);
      if (key !== wz.at) {
        wz.at = key;
        wz.open = {};
        if (at.part === "done") setupSay(t("setup.e64.title"));
      }
      return at;
    },
    card(n, at) {
      if (at.part === "done") return "done";
      const cur = E64_ORDER[at.step], me = E64_ORDER[n];
      if (me < cur) return "done";
      if (me > cur) return "later";
      return n === "x3" ? "wait" : "active";
    },
    head: (n, st) => stepHead(E64_ORDER[n], E64_TITLE[n], st),
    body(n, st) {
      if (n === "x1") {
        if (st === "done") return [el("p", { class: "body-text", text: t("setup.x1.done") })];
        if (st === "later") return null;
        return [
          el("p", { class: "body-text", text: t("setup.x1.text") }),
          crumbs([t("setup.s21.path2"), t("setup.s21.path3"), t("setup.x1.path4")]),
          hint(t("setup.x1.network")), question(t("setup.x1.question")),
          answers(answer("kindChange", "plain", "back", t("setup.back")), answer("x1", "ok", "check", t("setup.x1.yes"))),
        ];
      }
      if (n === "x2") {
        if (st === "done") return [el("p", { class: "body-text", text: t("setup.x2.done") })];
        if (st === "later") return null;
        const ip = pageIp();
        return [
          el("p", { class: "body-text", text: t("setup.x2.text") }),
          crumbs([t("setup.s21.path3"), t("setup.s22.path2"), t("setup.x2.path3")]),
          // The address this page came from is gbs-anywhere's; a name would do
          // on the grinder less reliably, so then the person looks it up.
          ip ? el("div", { class: "wz-codebox" },
            el("p", { class: "meta", text: t("setup.x2.label") }),
            el("p", { class: "wz-code addr", text: ip }))
            : hint(t("setup.x2.byName")),
          hint(t("setup.x2.ip")),
          // The grinder always connects to port 80: an app on another port
          // (say, Docker's -p 8080:80) needs gbs-anywhere on port 80 too.
          location.port && location.port !== "80" ? hint(t("setup.x2.port", { port: location.port }), "warn") : null,
          question(t("setup.x2.question")),
          answers(answer("x2Back", "plain", "back", t("setup.back")), answer("x2", "ok", "check", t("setup.x2.yes"))),
        ];
      }
      if (n === "x3") {
        if (st === "done") return [el("p", { class: "body-text", text: t("setup.x3.done") })];
        if (st === "later") return null;
        return [
          waiting(t("setup.x3.wait")),
          hint(t("setup.x3.help")),
          el("button", { type: "button", class: "wz-btn plain wide", "data-act": "x3Back" }, icon("back", 16, 2.5), t("setup.back")),
        ];
      }
      return null;
    },
    sub(n) {
      if (n === "x2") { const ip = pageIp(); return ip ? t("setup.x2.sub", { ip }) : ""; }
      return "";
    },
    pill(at) {
      if (at.step === "x3") return at.part === "done" ? ["connected", "done", false] : ["polling", "run", true];
      return ["prepare", "start", false];
    },
    progress: (at) => (at.part === "done" ? t("setup.pill.connected") : ""),
    sig: () => [wz.e64.x1, wz.e64.x2, pageIp()],
    top: () => [],
    end(at) {
      if (at.part !== "done") return [];
      return [el("section", { class: "card wz-hero" },
        el("span", { class: "wz-big" }, icon("check", 34, 2.4)),
        el("h2", { text: t("setup.e64.title") }),
        el("p", { class: "body-text" }, lines(t("setup.e64.text"))),
        hint(t("setup.e64.recipe")),
        el("button", { type: "button", id: "wz-finish", class: "wz-btn ok wide", "data-act": "finish" }, t("setup.done.button")))];
    },
    act(a) {
      switch (a) {
        case "x1": wz.e64.x1 = true; wz.focus = "wz-head-x2"; setupSay(t("setup.x1.done")); return true;
        case "x2": wz.e64.x2 = true; wz.focus = "wz-head-x3"; setupSay(t("setup.x2.done")); return true;
        case "x2Back": wz.e64.x1 = false; wz.focus = "wz-head-x1"; return true;
        case "x3Back": wz.e64.x2 = false; wz.focus = "wz-head-x2"; return true;
        default: return false;
      }
    },
  };

  // --- Setup: E65/E80 pairing --------------------------------------------------------
  // Pairing over Local WiFi: the address, Local WiFi and Start Pairing on
  // the grinder, the Pairing Code, the wait while the grinder restarts, the
  // Pairing ID if needed, and Status On. Re-pairing starts with a card of
  // its own above step 1: clear the pairing on the grinder, until the link
  // lets go of it.

  /** The E65/E80 pairing's part of the setup state. */
  function freshPair() {
    return {
      cleared: false,       // repair: the link let go of the old pairing
      clearTapped: false,   // repair: the person said it is cleared
      noClear: false,       // repair after Status Off: nothing to clear, no clearing card
      restartAt: 0,         // Date.now() of that restart (or of a reload into it)
      stillPaired: false,   // since then the link kept the pairing and the grinder answers
      c21: false,           // Local WiFi enabled
      started: 0,           // Date.now() of "Pairing started"
      c31: false,           // gbs-anywhere is shown on the grinder
      note: "",             // why it is back at step 2: retry, mismatch, notShown
      attempt: 1,
      lostAck: false,       // the lost pairing was seen and "Try again" tapped
      gone: false,          // the wait ended with a new request: shown as lost
      host: null,           // the address the last sync saw (not kept)
      afterWait: false,     // past the restart wait, before the next attempt
      lastCode: null,       // the Pairing Code last shown, for step 3 once done
      editHost: false, hostValue: null, hostError: "", hostBusy: false,
      acceptError: "", acceptBusy: false, tapped: false,
      acceptFor: null,      // the Pairing Code "Accepted" was sent for, until the link took it
      entry: null,          // the link's phase when the view opened (filled by pairSync)
      unreached: false,     // step 1 says gbs-anywhere doesn't reach the grinder
      idValue: "", idError: "", idBusy: false, refused: null, late: false,
      confirmed: false,     // Accepted was tapped here for this attempt
      statusOn: false,      // once paired: Status reads On on the grinder (true), or Off ("no")
      waitFrom: null,       // performance.now() the restart wait counts from
      quietFrom: null,      // performance.now() the quiet time began, while it lasts
      quietMs: 0,           // how long it lasts (quiet_ms)
    };
  }
  // "Pairing started" without a Pairing Code after this long: a hint.
  const NO_CODE_MS = 75000;
  // After Restart setup, a link still paired (not failing) after a few of
  // its 2-s polls has the grinder taking the pairing.
  const STILL_PAIRED_MS = 15000;
  // Connecting for this long counts as not reaching the grinder.
  const CONNECT_LONG_MS = 15000;
  // The phases of the restart wait (step 4).
  const WAIT_PHASES = ["holding", "checking", "quiet"];

  /** The grinder's address, as set, else as the link has it. */
  function pairHost() {
    const w = wifiOf();
    return (S && S.settings && S.settings.grinder_host) || (w && urlHost(w.grinder)) || "";
  }

  /** Where the pairing is: `step` 1 to 5 or "clear"; for step 5 its `part`
   *  (done, id or lost); for step 3 the Pairing Code, when there is one. */
  function pairAt() {
    const w = wifiOf();
    if (!pairHost() || wz.pair.editHost) return { step: 1 };
    // Re-pair: the link keeps the old pairing until it notices it was
    // cleared on the grinder, which takes Local WiFi on again. After
    // "Cleared" the setup goes on meanwhile.
    const old = wz.mode === "repair" && !wz.pair.cleared;
    if (old && !wz.pair.clearTapped) return { step: "clear" };
    if (!old && wifiPaired()) return { step: 5, part: "done" };
    const phase = w && w.state === "pairing" ? w.phase : null;
    if (phase === "asking") return { step: 5, part: "id" };
    // The restart wait, once the person confirmed here, or when the view
    // opened in it. The quiet always: a check went unanswered, also when
    // the person accepted on the grinder without confirming here.
    const entry = wz.pair.entry || {};
    if (phase === "quiet" || (WAIT_PHASES.includes(phase) && (wz.pair.tapped || WAIT_PHASES.includes(entry.phase)))) return { step: 4 };
    // The last accepted pairing did not stick (shown until one does), also
    // when the wait ended with a new request and the link didn't say so.
    if (w && (w.pairing_lost || wz.pair.gone) && !wz.pair.lostAck) return { step: 5, part: "lost" };
    const code = phase === "accepting" && w.pairing_code != null ? String(w.pairing_code) : null;
    // Step 3 only after "Pairing started" here: the grinder can answer with
    // a Pairing Code before pairing is started on it, even with Local WiFi
    // off, so a code says nothing about steps 2.1 and 2.2.
    if (wz.pair.started) return { step: 3, code };
    return { step: 2 };
  }

  // Each card's place: the clear card sits between steps 1 and 2.
  const PAIR_ORDER = { 1: 1, clear: 1.5, 2: 2, 3: 3, 4: 4, 5: 5 };
  const PAIR_TITLE = { clear: "setup.clear.title", 1: "setup.s1.title", 2: "setup.s2.title", 3: "setup.s3.title", 4: "setup.s4.title", 5: "setup.s5.title" };
  /** done, active, wait (for the grinder), retry or later. */
  function pairCard(n, at) {
    if (at.part === "done") return "done";
    if (n === "clear" && wz.pair.cleared) return "done";
    // Cleared on the grinder: waits in the background for the link.
    if (n === "clear" && wz.pair.clearTapped) return "wait";
    const cur = PAIR_ORDER[at.step], me = PAIR_ORDER[n];
    // The address is set, but gbs-anywhere doesn't reach the grinder there.
    if (n === 1 && cur > 1 && pairUnreached()) return "retry";
    if (me < cur) return "done";
    if (me > cur) return "later";
    if (n === "clear") return wz.pair.clearTapped ? "wait" : "active";
    if (n === 3 && !at.code) return "wait";
    if (n === 4) return "wait";
    if (n === 5 && at.part === "lost") return "retry";
    return "active";
  }

  /** The header pill: its text (setup.pill.<key>), tone and whether it pulses. */
  function pairPill(at) {
    if (at.part === "done") return wz.pair.statusOn === "no" ? ["lost", "warn", false] : ["done", "done", false];
    if (at.step === 1) return ["address", "none", false];
    if (at.step === "clear") return ["clear", wz.pair.clearTapped ? "run" : "start", wz.pair.clearTapped];
    if (at.step === 2) return ["prepare", "start", false];
    if (at.step === 3) return ["code", at.code ? "start" : "run", !at.code];
    if (at.step === 4) return [pairConfirmed() ? "restart" : "prepare", "run", true];
    if (at.part === "lost") return ["lost", "warn", false];
    return ["id", "start", false];
  }

  /** Accepted was tapped here for this attempt, or the link holds the
   *  check after one (after a reload). */
  function pairConfirmed() {
    const w = wifiOf();
    return wz.pair.confirmed || wz.pair.tapped || !!(w && w.state === "pairing" && (w.phase === "holding" || w.phase === "checking"));
  }

  /** gbs-anywhere doesn't reach the grinder at the address: it does not
   *  answer, or the link has been connecting for long. Not while the old
   *  pairing is cleared on the grinder (that can take Local WiFi off). */
  function pairUnreached() {
    const w = wifiOf();
    if (!w || (wz.mode === "repair" && !wz.pair.cleared)) return false;
    if (w.state === "failing") return !!w.unreachable;
    return w.state === "connecting" && linkStateAge(w) > CONNECT_LONG_MS;
  }

  /** The link reached the grinder: it answers, paired or not. */
  function pairReached() {
    const w = wifiOf();
    return !!(w && (w.state === "pairing" || w.state === "paired" || (w.state === "failing" && w.unreachable === false)));
  }

  /** The body of card `n` in state `st`, or null when it has none. */
  function pairBody(n, st, at) {
    const w = wifiOf();
    if (n === 1) {
      if (st === "done") {
        const host = pairHost();
        return [el("p", { class: "body-text", text: pairReached() ? t("setup.s1.found", { host }) : t("setup.s1.sub", { host }) })];
      }
      const unreached = st === "retry" ? [banner("warn", t("setup.s1.unreached", { host: pairHost() }), "warn")] : [];
      const input = el("input", {
        id: "wz-host", inputmode: "url", autocomplete: "off", autocapitalize: "off", spellcheck: "false", placeholder: "192.168.1.30",
        enterkeyhint: "done", "aria-describedby": wz.pair.hostError ? "wz-host-error" : null,
      });
      input.value = wz.pair.hostValue != null ? wz.pair.hostValue : pairHost();
      return [
        ...unreached,
        el("p", { class: "body-text", text: t("setup.s1.text") }),
        el("form", { class: "stack", "data-form": "host", autocomplete: "off" },
          el("label", { class: "field-label" }, el("span", { text: t("setup.s1.label") }), el("span", { class: "field wz-host" }, input)),
          wz.pair.hostError ? errorLine("wz-host-error", wz.pair.hostError) : null,
          el("button", { type: "submit", class: "wz-btn wide", disabled: wz.pair.hostBusy }, t("setup.s1.button"))),
      ];
    }
    if (n === "clear") {
      if (st === "done") return [el("p", { class: "body-text", text: t("setup.clear.done") })];
      if (st === "later") return null;
      // Paired, Configuration opens the paired device's page, with Clear
      // Pairing (Pairing löschen) at its end.
      const path = crumbs([t("setup.s21.path2"), t("setup.s21.path3"), t("setup.s22.path2"), t("setup.clear.button")]);
      if (wz.pair.clearTapped) {
        return [el("p", { class: "body-text", text: t("setup.clear.text") }), path, hint(t("setup.s21.menu")), waiting(t("setup.clear.wait")),
          el("button", { type: "button", class: "wz-btn plain wide", "data-act": "unclear" }, icon("back", 16, 2.5), t("setup.back"))];
      }
      return [el("p", { class: "body-text", text: t("setup.clear.text") }), path, hint(t("setup.s21.menu")), question(t("setup.clear.question")),
        answers(answer("close", "plain", "back", t("setup.back")), answer("cleared", "ok", "check", t("setup.clear.yes"), t("setup.clear.yesSub")))];
    }
    if (n === 2) {
      if (st === "later") return null;
      const fin = st === "done";
      const enabled = wz.pair.c21 || fin;
      const out = [el("p", { class: "body-text", text: t("setup.s2.intro") })];
      if (!fin && wz.pair.note === "mismatch") out.push(banner("bad", t("setup.note.mismatch")));
      if (!fin && wz.pair.note === "notShown") out.push(banner("warn", t("setup.note.notShown")));
      if (!fin && wz.pair.note === "retry") out.push(banner("warn", t("setup.note.retry"), "warn"));
      if (!fin && wz.pair.note === "cleared") out.push(banner("warn", t("setup.note.cleared"), "warn"));
      out.push(parts([
        { id: "wz-s21", title: t("setup.s21.title"), s: enabled ? "done" : "now", body: [
          crumbs([t("setup.s21.path2"), t("setup.s21.path3"), t("setup.s21.path4"), t("setup.s21.path5")]),
          hint(t("setup.s21.menu")),
          // The switch is named after the last device the grinder knew.
          hint(t("setup.s21.name")), hint(t("setup.s21.tip")), question(t("setup.s21.question")),
          answers(answer("back21", "plain", "back", t("setup.back")), answer("c21", "ok", "check", t("setup.s21.yes")))] },
        { id: "wz-s22", title: t("setup.s22.title"), s: fin ? "done" : enabled ? "now" : "next", body: [
          // On the Machine To Machine screen of 2.1: Local WiFi connected is the
          // switch's second line, not a level.
          crumbs([t("setup.s22.path2"), t("setup.s22.path4")]),
          hint(t("setup.s22.status")), question(t("setup.s22.question")),
          answers(answer("back22", "plain", "back", t("setup.back")), answer("started", "ok", "check", t("setup.s22.yes"), t("setup.s22.yesSub")))] },
      ]));
      return out;
    }
    if (n === 3) {
      if (st === "later") return null;
      const fin = st === "done";
      const code = at.code || wz.pair.lastCode;
      const shown = wz.pair.c31 || fin;
      const out = [];
      if (wz.pair.attempt > 1 && !fin) out.push(el("span", { class: "wz-attempt", text: t("setup.attempt", { n: wz.pair.attempt }) }));
      if (!fin && wz.pair.note === "notAccepted") out.push(banner("warn", t("setup.note.notAccepted"), "warn"));
      let first;
      if (at.code) first = [hint(t("setup.s31.text")), answers(answer("no31", "no", "cross", t("setup.s31.no")), answer("c31", "ok", "check", t("setup.s31.yes")))];
      else {
        first = [hint(t("setup.s31.text")), el("div", { class: "wz-codebox waiting" },
          el("p", { class: "meta", text: t("setup.code.wait") }), el("p", { class: "wz-code", "aria-hidden": "true", text: "··" }))];
        if (wz.pair.late) {
          first.push(banner("warn", t("setup.note.noCode")),
            el("button", { type: "button", class: "wz-btn plain wide", "data-act": "noCodeBack" }, icon("back", 16, 2.5), t("setup.back")));
        }
      }
      out.push(parts([
        { id: "wz-s31", title: shown ? t("setup.s31.done") : t("setup.s31.title"), s: shown ? "done" : "now", body: first },
        { id: "wz-s32", title: fin ? t("setup.s32.done", { code: code || "" }) : t("setup.s32.title"), s: fin ? "done" : shown ? "now" : "next", body: [
          el("div", { class: "wz-codebox" },
            el("p", { class: "wz-name", text: "gbs-anywhere" }),
            el("p", { class: "wz-code", text: code || "" }),
            el("p", { class: "meta", text: t("setup.code.label") })),
          hint(t("setup.code.note")), question(t("setup.s32.text")),
          wz.pair.acceptError ? errorLine("wz-accept-error", wz.pair.acceptError) : null,
          // Only while the link waits for it (it shows a code); else the
          // server would refuse it.
          answers(answer("no32", "no", "cross", t("setup.s32.no"), t("setup.s32.noSub")),
            answer("accepted", "ok", "check", t("setup.s32.yes"), t("setup.s32.yesSub"), wz.pair.acceptBusy || !at.code))] },
      ]));
      return out;
    }
    if (n === 4) {
      if (st === "done") return [el("p", { class: "body-text", text: t("setup.s4.back") })];
      if (st !== "wait") return null;
      // Said only when it is so: Accepted was tapped on this page (another
      // device's tap also holds, but isn't "your" acceptance here).
      const registered = wz.pair.confirmed;
      // Nothing is sent to the grinder for a while: the usual time no longer
      // holds, so it is not shown.
      const quiet = !!w && w.state === "pairing" && w.phase === "quiet";
      // Opened in the wait (or reloaded), the timer counts from where this
      // phase began, not from the tap: the usual total doesn't fit then.
      const midWait = WAIT_PHASES.includes((wz.pair.entry || {}).phase);
      return [
        // A Timeout or a frozen screen looks like a failure: hands off, first.
        el("div", { class: "banner warn wz-banner-icon" }, icon("warn", 18, 2.2),
          el("span", null, el("strong", { text: t("setup.s4.hands.title") }), " " + t("setup.s4.hands.text"))),
        pairConfirmed()
          ? el("p", { class: "body-text" }, el("strong", { text: t("setup.s4.intro") }), registered ? " " + t("setup.s4.registered") : null)
          // No Accepted here: the grinder may be restarting after an accept
          // on it, or simply be off.
          : el("p", { class: "body-text", text: t("setup.s4.unconfirmed") }),
        el("div", { class: "wz-wait" }, el("span", { class: "wz-spinner" }),
          el("div", null, el("p", { class: "wz-timer", id: "wz-timer", text: pairWaited() }),
            quiet || midWait ? null : el("p", { class: "meta wz-timeline" }, icon("timer", 16, 2.2), t("setup.s4.time")))),
        quiet ? hint(t("setup.s4.quiet", { n: quietMinutes(w) })) : null,
        // What the grinder usually shows meanwhile, so none of it is a surprise.
        el("p", { class: "hint" }, el("strong", { text: t(pairConfirmed() ? "setup.s4.flow.title" : "setup.s4.flow.titleIf") })),
        el("ol", { class: "wz-flow" }, ["setup.s4.flow1", "setup.s4.flow2", "setup.s4.flow3", "setup.s4.flow4"].map((k) => el("li", { text: t(k) }))),
        el("ul", { class: "wz-dont" }, ["setup.s4.dont1", "setup.s4.dont3"].map((k) => el("li", null, icon("stop", 16, 2.2), el("span", { text: t(k) })))),
      ];
    }
    if (n === 5) {
      if (at.part === "done") {
        return [el("div", { class: "outcome good wz-outcome" },
          el("p", null, el("strong", { text: t("setup.done.result") }), " " + t("setup.done.resultValue")),
          el("p", null, el("strong", { text: t("setup.done.id") }), " " + String(w.pairing_id)),
          el("p", null, el("strong", { text: t("setup.done.grinder") }), " " + pairHost()))];
      }
      if (at.part === "id") {
        const refused = w.unknown_pairing_id != null ? t("setup.s5.refused", { id: String(w.unknown_pairing_id) }) : "";
        const error = wz.pair.idError || refused;
        const input = el("input", { id: "wz-id", inputmode: "numeric", autocomplete: "off", enterkeyhint: "send",
          placeholder: t("setup.s5.placeholder"), "aria-describedby": error ? "wz-id-error" : null });
        input.value = wz.pair.idValue;
        return [
          el("p", { class: "body-text", text: t("setup.s5.text") }),
          // After the restart the grinder is on its home screen: the whole way.
          crumbs([t("setup.s21.path2"), t("setup.s21.path3"), t("setup.s22.path2")]),
          hint(t("setup.s21.menu")),
          el("form", { class: "stack", "data-form": "id", autocomplete: "off" },
            el("label", { class: "field-label" }, el("span", { text: t("setup.s5.label") }), el("span", { class: "field wz-field" }, input)),
            error ? errorLine("wz-id-error", error) : null,
            el("button", { type: "submit", class: "wz-btn ok wide", disabled: wz.pair.idBusy }, icon("check", 16, 2.5), t("setup.s5.button"))),
        ];
      }
      if (at.part === "lost") {
        return [
          // Without Accepted tapped here, the grinder may just have been off;
          // with a new request the old one may have expired (see pairLostTitle).
          pairLostTitle() === "setup.lost.title"
            ? el("div", { class: "banner warn" }, el("b", { class: "wz-banner-title", text: t("setup.lost.title") }), t("setup.lost.text"))
            : el("div", { class: "banner warn" }, el("b", { class: "wz-banner-title", text: t("setup.lost.titleOff") }), t(wz.pair.gone ? "setup.lost.textGone" : "setup.lost.textOff")),
          hint(t("setup.lost.honest")),
          // Seen to help now and then, not always.
          wz.pair.attempt > 1 ? hint(t("setup.lost.restart")) : null,
          el("button", { type: "button", class: "wz-btn wide", "data-act": "again" }, icon("retry", 18, 2.2), t("setup.lost.button")),
        ];
      }
    }
    return null;
  }

  /** The short info under a card's title, or "". */
  function pairSub(n, at) {
    if (n === 1) { const host = pairHost(); return host ? t("setup.s1.sub", { host }) : t("setup.s1.subNone"); }
    if (n === "clear") return wz.pair.cleared ? t("setup.clear.done") : wz.pair.clearTapped ? t("setup.clear.subWait") : "";
    if (n === 2) return t("setup.s2.sub");
    if (n === 3) {
      // The last code once the step is done; while it waits, only the one
      // showing (an earlier one may have gone).
      const past = PAIR_ORDER[at.step] > 3;
      const code = at.code || (past ? wz.pair.lastCode : null);
      // Done without a code to show (after a reload): nothing to wait for.
      return code ? t("setup.s3.sub", { code }) : past ? "" : t("setup.s3.subWait");
    }
    return "";
  }

  /** How long the restart wait has gone on, as m:ss, counted from when the
   *  link's phase began (the server's phase_age_ms); in the quiet time, how
   *  long until gbs-anywhere checks again. */
  function pairWaited() {
    if (wz.pair.quietFrom != null) return clockDown(Math.max(0, wz.pair.quietMs - (performance.now() - wz.pair.quietFrom)));
    return wz.pair.waitFrom == null ? "0:00" : clockDown(performance.now() - wz.pair.waitFrom);
  }

  /** The first Pairing Code after the restart wait: the same one is the
   *  same request, not accepted yet; another one, with no word of a lost
   *  pairing, means the old request is gone (it ran out on the grinder, or
   *  gbs-anywhere restarted), shown as a lost pairing rather than a silent
   *  step 2. */
  function pairFirstCodeAfterWait(w) {
    const p = wz.pair;
    p.afterWait = false;
    if (String(w.pairing_code) === p.lastCode) {
      Object.assign(p, { started: Date.now(), c21: true, c31: true, note: "notAccepted", lostAck: true, confirmed: false });
      setupSay(t("setup.note.notAccepted"));
    } else if (p.lastCode != null && !w.pairing_lost) p.gone = true;
  }

  /** The lost card's title: a restart lost an accepted pairing only when
   *  Accepted was tapped here and the link didn't just get a new request
   *  (that one also fits an expired request or gbs-anywhere restarting). */
  const pairLostTitle = () => (wz.pair.confirmed && !wz.pair.gone ? "setup.lost.title" : "setup.lost.titleOff");

  /** Follows the link: the clear card ends once the link lets go of the
   *  old pairing, and a new step starts with its cards as they open. */
  function pairSync() {
    const w = wifiOf();
    const phase = w && w.state === "pairing" ? w.phase : null;
    // A hold follows a confirmation, also one tapped on another device or
    // before its reply came: the wait shows at once, and the check after it
    // stays part of it.
    if (phase === "holding") wz.pair.tapped = true;
    // Each quiet round restarts the server's phase_age_ms; within a round
    // the first anchor stays, so poll jitter doesn't shift the countdown.
    const quietFrom = phase === "quiet" ? stateAt - (w.phase_age_ms || 0) : null;
    if (quietFrom == null || wz.pair.quietFrom == null || Math.abs(quietFrom - wz.pair.quietFrom) > 2000) wz.pair.quietFrom = quietFrom;
    if (quietFrom != null) wz.pair.quietMs = w.quiet_ms || 0;
    // The link's phase as the view opens: the restart wait then shows as
    // step 4.
    if (!wz.pair.entry) wz.pair.entry = { phase };
    // After Restart setup the link still holding the pairing for long, with
    // the grinder answering, means it has it after all: Status may be On.
    if (wz.pair.noClear && !wz.pair.cleared && !wz.pair.restartAt) wz.pair.restartAt = Date.now();
    const stillPaired = wz.pair.noClear && !wz.pair.cleared && !!w && w.state === "paired" && Date.now() - wz.pair.restartAt > STILL_PAIRED_MS;
    if (stillPaired && !wz.pair.stillPaired) setupSay(t("setup.note.stillPaired"));
    wz.pair.stillPaired = stillPaired;
    if (wz.mode === "repair" && !wz.pair.cleared && !wifiPaired()) {
      wz.pair.cleared = true;
      // From now on a reload goes on with the setup, not with clearing again.
      if (location.hash === "#repair") history.replaceState(null, "", "#setup");
    }
    // "Accepted" stays busy until the link took it: it left accepting, or
    // shows another code.
    if (wz.pair.acceptFor != null && !(phase === "accepting" && String(w.pairing_code) === wz.pair.acceptFor)) {
      // Another code while still accepting: the tap was for the old one.
      if (phase === "accepting") wz.pair.tapped = false;
      wz.pair.acceptFor = null;
      wz.pair.acceptBusy = false;
    }
    // The code of the request the wait is for, also when the view opened
    // in the wait (the link shows it while holding and checking).
    if (WAIT_PHASES.includes(phase) && w.pairing_code != null) wz.pair.lastCode = String(w.pairing_code);
    // Another address is another grinder: nothing from the last one's wait
    // applies (also when it changed elsewhere, not through Back here).
    const host = pairHost();
    if (wz.pair.host != null && host !== wz.pair.host) Object.assign(wz.pair, { afterWait: false, gone: false, lastCode: null });
    wz.pair.host = host;
    // The first code after the wait, also after the link restarted in
    // between (gbs-anywhere restarted); not while another address is being
    // typed, which may be another grinder.
    if (wz.pair.afterWait && !wz.pair.editHost && phase === "accepting" && w.pairing_code != null) pairFirstCodeAfterWait(w);
    let at = pairAt();
    // Asking for the Pairing ID or paired: the wait's question is answered.
    if (at.part === "id" || at.part === "done") Object.assign(wz.pair, { afterWait: false, gone: false });
    // What the server says by itself, announced when it changes.
    const refused = at.part === "id" && w.unknown_pairing_id != null ? w.unknown_pairing_id : null;
    if (refused !== wz.pair.refused) {
      wz.pair.refused = refused;
      if (refused != null) setupSay(t("setup.s5.refused", { id: String(refused) }));
    }
    // Not while the link is past the code, or still holds the old pairing:
    // no new code can come then.
    const unreached = pairCard(1, at) === "retry";
    if (unreached && !wz.pair.unreached) setupSay(t("setup.s1.unreached", { host: pairHost() }));
    wz.pair.unreached = unreached;
    const late = at.step === 3 && !at.code && !!wz.pair.started && Date.now() - wz.pair.started > NO_CODE_MS
      && !WAIT_PHASES.includes(phase) && !(wz.mode === "repair" && !wz.pair.cleared);
    if (late && !wz.pair.late) setupSay(t("setup.note.noCode"));
    wz.pair.late = late;
    let key = setupKey(at);
    if (key !== wz.at) {
      const wasWaiting = wz.at.startsWith("4:");
      if (at.part !== "done") {
        // Status was not On, and the link let go of the pairing: again from
        // 2.1, as after a lost pairing (Local WiFi may be off).
        if (wz.pair.statusOn === "no") {
          Object.assign(wz.pair, { attempt: wz.pair.attempt + 1, started: 0, c31: false, c21: false, confirmed: false });
          at = pairAt();
          key = setupKey(at);
          if (at.step === 2) {
            wz.pair.note = "retry";
            wz.focus = "wz-s21";
            setupSay(t("setup.note.retry"));
          }
        }
        // A later pairing is asked about again.
        wz.pair.statusOn = false;
      }
      wz.at = key;
      wz.open = {};
      if (at.step === 4) {
        if (!wasWaiting) wz.pair.waitFrom = stateAt - ((w && w.phase_age_ms) || 0);
        // What the person said about this attempt is past; the next one
        // starts afresh, and a pairing lost after it is shown again.
        Object.assign(wz.pair, { started: 0, c31: false, note: "", lostAck: false, gone: false, afterWait: false, acceptError: "" });
      } else {
        wz.pair.waitFrom = null;
        wz.pair.tapped = false;
        // Past the wait the view opened in: the next one starts from a tap.
        // Not once it asks for the Pairing ID or is paired: a later code is
        // then a new attempt.
        if (wasWaiting) Object.assign(wz.pair, { entry: { phase: null }, afterWait: at.step !== 5 });
        // Back from the check with a Pairing Code at once: told by the code,
        // the same request (not accepted yet: the code shows again, with a
        // note) or a new one (see pairFirstCodeAfterWait).
        if (wasWaiting && phase === "accepting" && w.pairing_code != null) {
          pairFirstCodeAfterWait(w);
          at = pairAt();
          wz.at = setupKey(at);
        }
      }
      if (at.part === "lost") setupSay(t(pairLostTitle()));
    }
    // The code the link shows now, at any step (also at step 2 before
    // "Pairing started"): a wait entered from here is about this request.
    // Taken last, so the comparisons above still see the one before. Not
    // under the lost card: its done step 3 keeps the code that was accepted,
    // and Try again goes back to step 2, which takes the new one.
    if (phase === "accepting" && w.pairing_code != null && !wz.pair.editHost && at.part !== "lost") wz.pair.lastCode = String(w.pairing_code);
    return at;
  }

  const PAIR_FLOW = {
    kept: ["clearTapped", "noClear", "c21", "started", "c31", "attempt", "note", "lostAck", "gone", "afterWait", "confirmed", "lastCode"],
    // The clear card sits above step 1, as the first thing to do.
    cards: () => [...(wz.mode === "repair" && !wz.pair.noClear ? ["clear"] : []), 1, 2, 3, 4, 5],
    number: (n) => (typeof n === "number" ? n : null),
    at: pairAt,
    sync: pairSync,
    card: pairCard,
    head(n, st, at) {
      if (n === "clear") {
        // It waits in the background, closed, once the setup went on.
        return { kicker: t("setup.pill.clear"), title: t(PAIR_TITLE.clear), mark: icon("unlink", 16, 2.2), no: null,
          auto: (st === "active" || st === "wait" || st === "retry") && at.step === "clear" };
      }
      return stepHead(n, n === 4 && st === "wait" && !pairConfirmed() ? "setup.s4.titleWait" : PAIR_TITLE[n], st);
    },
    body: pairBody,
    sub: pairSub,
    pill: pairPill,
    progress(at) {
      if (at.part === "done") return t(wz.pair.statusOn === "no" ? "setup.pill.lost" : "setup.pill.done");
      return at.step === "clear" ? t("setup.pill.clear") : "";
    },
    sig() {
      const w = wifiOf(), p = wz.pair;
      return [pairHost(), pairReached(), p.c21, p.c31, !!p.started, p.late, p.note, p.attempt, p.clearTapped, p.cleared, p.lastCode,
        p.hostError, p.hostBusy, p.acceptError, p.acceptBusy, p.tapped, p.idError, p.idBusy, p.statusOn, p.stillPaired, p.confirmed,
        w && w.phase, w && w.unknown_pairing_id, w && w.pairing_id];
    },
    top() {
      if (!wz.pair.stillPaired) return [];
      return [el("section", { class: "card pad stack-12" },
        banner("warn", t("setup.note.stillPaired"), "warn"),
        el("button", { type: "button", class: "wz-btn ok wide", "data-act": "stillPaired" }, icon("check", 16, 2.5), t("setup.stillPaired.button")))];
    },
    end(at) {
      if (at.part !== "done") return [];
      return [wz.pair.statusOn === "no"
        // Status not On: the grinder has no pairing, so nothing to clear on
        // it. Restart goes on from 2.1 now; the link lets go of the pairing
        // the grinder refuses meanwhile (and the page moves by itself if
        // that comes first).
        ? el("section", { class: "card wz-hero" },
          el("span", { class: "wz-big bad" }, icon("cross", 34, 2.4)),
          el("h2", { text: t("setup.done.titleOff") }),
          el("p", { class: "body-text", text: t("setup.done.statusOff") }),
          el("button", { type: "button", id: "wz-restart", class: "wz-btn ok wide", "data-act": "restart" }, t("setup.done.restart")))
        : el("section", { class: "card wz-hero" },
          el("span", { class: "wz-big" }, icon("check", 34, 2.4)),
          el("h2", { text: t("setup.done.title") }),
          el("p", { class: "body-text" }, lines(t("setup.done.text"))),
          // What is left on the grinder, asked here where it shows (step 5
          // is closed once done): Status On (An).
          ...(wz.pair.statusOn ? [
            hint(t("setup.done.home")),
            el("button", { type: "button", id: "wz-finish", class: "wz-btn ok wide", "data-act": "finish" }, t("setup.done.button")),
            // Only once Status is known to be On: before, the question decides.
            el("button", { type: "button", class: "wz-redo", "data-act": "redo" }, t("grinder.clear"))]
          : [
            el("p", { class: "wz-question", id: "wz-status-q", tabindex: "-1", text: t("setup.done.statusQ", { section: t("setup.s21.path4") }) }),
            answers(answer("statusNo", "plain", "cross", t("setup.done.statusNo")), answer("statusYes", "ok", "check", t("setup.done.statusYes")))]))];
    },
    async act(a) {
      const p = wz.pair;
      switch (a) {
        case "statusYes": p.statusOn = true; wz.focus = "wz-finish"; setupSay(t("setup.done.home")); return true;
        // Status is Off only without a pairing: the link lets go of it once
        // the grinder refuses it for a while, and the setup goes on from 2.1.
        case "statusNo": p.statusOn = "no"; wz.focus = "wz-restart"; setupSay(t("setup.done.titleOff") + ". " + t("setup.done.statusOff")); return true;
        // Status is On after all: back to the finished card.
        case "stillPaired":
          wz = freshSetup("setup");
          history.replaceState(null, "", "#setup");
          return true;
        // Again from 2.1 while the link still holds the pairing, as after
        // Cleared in a re-pair, but without the clearing card.
        case "restart": {
          const attempt = p.attempt + 1;
          wz = freshSetup("repair");
          Object.assign(wz.pair, { clearTapped: true, noClear: true, attempt, note: "retry" });
          wz.focus = "wz-s21";
          history.replaceState(null, "", "#repair");
          setupSay(t("setup.note.retry"));
          return true;
        }
        case "redo":
          // Re-pair: clear the pairing on the grinder first.
          wz = freshSetup("repair");
          history.replaceState(null, "", "#repair");
          return true;
        // Clear Pairing can also switch Local WiFi off on the grinder: on to
        // step 2.1 while the link notices the cleared pairing.
        case "cleared":
          Object.assign(p, { clearTapped: true, c21: false, started: 0, note: "cleared" });
          setupNext("wz-s21", t("setup.clear.title"));
          return true;
        case "unclear": Object.assign(p, { clearTapped: false, note: "" }); return true;
        case "c21": p.c21 = true; setupNext("wz-s22", t("setup.s21.title")); return true;
        case "back21": p.editHost = true; p.hostValue = pairHost(); wz.focus = "wz-host"; return true;
        case "back22": p.c21 = false; wz.focus = "wz-s21"; return true;
        case "started": Object.assign(p, { started: Date.now(), note: "", c31: false, confirmed: false, afterWait: false }); wz.focus = "wz-s31"; return true;
        case "c31": p.c31 = true; setupNext("wz-s32", t("setup.s31.done")); return true;
        // Not shown: start pairing again on the grinder (Local WiFi is on).
        case "no31":
          Object.assign(p, { started: 0, c31: false, c21: true, note: "notShown" });
          wz.focus = "wz-s22";
          setupSay(t("setup.note.notShown"));
          return true;
        // The codes differ: don't accept; the next attempt brings a new code.
        case "no32":
          Object.assign(p, { attempt: p.attempt + 1, started: 0, c31: false, c21: true, confirmed: false, note: "mismatch" });
          wz.focus = "wz-s22";
          setupSay(t("setup.note.mismatch"));
          return true;
        // Lost in the restart: from step 2, and Local WiFi may be off again.
        case "again":
          Object.assign(p, { attempt: p.attempt + 1, started: 0, c31: false, c21: false, confirmed: false, note: "retry", lostAck: true, gone: false, afterWait: false });
          wz.focus = "wz-s21";
          setupSay(t("setup.note.retry"));
          return true;
        case "noCodeBack": Object.assign(p, { started: 0, c31: false, c21: false, note: "" }); wz.focus = "wz-s21"; return true;
        case "accepted": await pairSendAccepted(); return false;
        default: return false;
      }
    },
    input(id, value) {
      if (id === "wz-host") wz.pair.hostValue = value;
      if (id === "wz-id") wz.pair.idValue = value;
    },
    submit(form) {
      if (form === "host" && !wz.pair.hostBusy) pairSaveHost();
      if (form === "id" && !wz.pair.idBusy) pairSendId();
    },
    tick() {
      if (wz.pair.waitFrom == null) return;
      const e = $("wz-timer");
      if (e) e.textContent = pairWaited();
    },
  };

  /** "Accepted": the server takes it only while it waits for it. Busy until
   *  the link took it (see pairSync), so a second tap can't be refused. */
  async function pairSendAccepted() {
    if (wz.pair.acceptBusy) return;
    wz.pair.acceptBusy = true; wz.pair.acceptError = "";
    renderSetup();
    try {
      await api("/api/grinder/accepted", { method: "POST" });
      wz.pair.tapped = true;
      wz.pair.confirmed = true;
      wz.pair.acceptFor = wz.pair.lastCode;
      if (wz.pair.note === "notAccepted") wz.pair.note = "";
      await tick();
    } catch (x) {
      wz.pair.acceptBusy = false;
      wz.pair.acceptError = x.message;
      setupSay(x.message);
    } finally { renderSetup(); }
  }

  /** Step 1: the address goes to the settings, as on the grinder card;
   *  the server says what it does not take. */
  async function pairSaveHost() {
    const v = String(wz.pair.hostValue == null ? pairHost() : wz.pair.hostValue).trim();
    if (!v) { wz.pair.hostError = t("setup.s1.empty"); wz.focus = "wz-host"; setupSay(wz.pair.hostError); renderSetup(); return; }
    if (v === ((S.settings && S.settings.grinder_host) || "")) {
      wz.pair.editHost = false; wz.pair.hostError = ""; wz.pair.hostValue = null;
      renderSetup();
      return;
    }
    wz.pair.hostBusy = true; wz.pair.hostError = "";
    renderSetup();
    try {
      await saveHost(v);
      wz.pair.editHost = false; wz.pair.hostValue = null;
      await tick();
    } catch (x) {
      wz.pair.hostError = x.message; wz.focus = "wz-host"; setupSay(x.message);
    } finally { wz.pair.hostBusy = false; renderAll(); }
  }

  /** Step 5: the Pairing ID the grinder shows; the link checks it with the
   *  grinder, and says when the grinder did not take it. */
  async function pairSendId() {
    const raw = String(wz.pair.idValue || "").trim();
    const id = Number(raw);
    if (!/^\d+$/.test(raw) || id < 1 || id > 65535) { wz.pair.idError = t("setup.s5.range"); wz.focus = "wz-id"; setupSay(wz.pair.idError); renderSetup(); return; }
    wz.pair.idBusy = true; wz.pair.idError = "";
    renderSetup();
    try {
      await api("/api/grinder/pairing", { method: "POST", body: JSON.stringify({ pairing_id: id }) });
      wz.pair.idValue = "";
      await tick();
    } catch (x) {
      wz.pair.idError = x.message; wz.focus = "wz-id"; setupSay(x.message);
    } finally { wz.pair.idBusy = false; renderSetup(); }
  }

  // The steps of each grinder (see setupKind).
  const SETUP_FLOWS = { e64: E64_FLOW, wifi: PAIR_FLOW };

  $("wz-steps").addEventListener("click", (e) => {
    const head = e.target.closest("[data-toggle]");
    if (head) {
      const n = /^\d+$/.test(head.dataset.toggle) ? Number(head.dataset.toggle) : head.dataset.toggle;
      wz.open[n] = head.getAttribute("aria-expanded") !== "true";
      wz.focus = null;
      renderSetup();
      const again = $("wz-steps").querySelector('[data-toggle="' + head.dataset.toggle + '"]');
      if (again) again.focus();
      return;
    }
    const b = e.target.closest("[data-act]");
    if (b && !b.disabled) setupAction(b.dataset.act);
  });
  $("wz-steps").addEventListener("input", (e) => {
    const F = setupFlow();
    if (F && F.input) F.input(e.target.id, e.target.value);
  });
  $("wz-steps").addEventListener("submit", (e) => {
    e.preventDefault();
    const F = setupFlow();
    if (F && F.submit) F.submit(e.target.dataset.form);
  });
  $("setup-close").addEventListener("click", closeSetup);

  // --- Bottom sheet ------------------------------------------------------------------
  // While open, the page behind is inert, so focus stays in the sheet.
  // `returnTo` (an element or a function) gets the focus back on close.
  let sheetReturn = null;
  function openSheet({ title, body, actions, focus, submitOnEnter, returnTo }) {
    sheetReturn = returnTo || document.activeElement;
    $("sheet-title").textContent = title;
    $("sheet-body").replaceChildren(...body);
    $("sheet-error").hidden = true;
    let first = focus || null;
    const btns = actions.map((a) => {
      const b = el("button", { type: "button", class: a.cls, text: a.label });
      if (a.autofocus && !first) first = b;
      b.disabled = !!a.disabled;
      b.addEventListener("click", () => a.run(b));
      if (a.ref) a.ref(b);
      return b;
    });
    $("sheet-actions").className = "sheet-actions" + (btns.length === 1 ? " one" : "");
    $("sheet-actions").replaceChildren(...btns);
    if (submitOnEnter && focus) {
      focus.addEventListener("keydown", (e) => {
        const last = btns[btns.length - 1];
        if (e.key === "Enter" && !last.disabled) { e.preventDefault(); last.click(); }
      });
    }
    $("app").setAttribute("inert", "");
    $("sheet").hidden = false;
    requestAnimationFrame(() => (first || btns[btns.length - 1]).focus());
  }
  function closeSheet() {
    $("sheet").hidden = true;
    $("sheet-body").replaceChildren();
    $("app").removeAttribute("inert");
    const back = sheetReturn;
    sheetReturn = null;
    if (typeof back === "function") back();
    else if (back && back.focus && document.contains(back)) back.focus();
  }
  function sheetError(text) { $("sheet-error").textContent = text; $("sheet-error").hidden = false; }
  $("sheet").addEventListener("click", (e) => { if (e.target === $("sheet")) closeSheet(); });

  // --- Live updates --------------------------------------------------------------------
  function renderAll() {
    if (!S) { renderHeader({ sc: "ready" }); return; }
    // Name and version at the bottom of every tab but Status, whose
    // gbs-anywhere card shows them.
    $("app-footer").hidden = tab === "status";
    const [release, build] = String(S.server.version || "").split("+");
    $("app-footer").textContent = "gbs-anywhere" + (release ? " " + release + (build ? " · " + build : "") : "");
    $("app-footer").title = String(S.server.version || "");
    renderStart();
    renderStatus();
    renderHistory();
    renderLog();
    renderSetup();
  }

  // Another version of gbs-anywhere answers (it was updated): the page that
  // comes with it is loaded, so the page always matches its server. Not
  // while something is typed or open here; the next poll tries again.
  // PAGE_VERSION is the version this page was built with, filled in by the
  // server like STRINGS, so a page from the browser's cache keeps its own.
  const PAGE_VERSION = /* version */;
  // `reloadedFor`: the version the last reload was for (see the start
  // below). A page still older after it, say from a cache in between, is
  // not reloaded again for that version.
  let reloading = false, reloadedFor = null;
  /** Something typed or open that a reload would lose, or a shot window
   *  still waiting to be saved. */
  function midEdit() {
    const at = document.activeElement;
    return !!(editing || addingKind || win.timer || win.pending != null || !$("g-host-form").hidden || !$("sheet").hidden
      || (at && at.matches && at.matches("input, textarea")));
  }

  async function refresh() {
    let st;
    try { st = await api("/api/state"); }
    catch (_) { if (online) offlineSince = Date.now(); online = false; renderAll(); return; }
    online = true;
    // A server from before the names were grouped (0.2.1) has its version
    // at the top: the reload then brings that server's page.
    const version = (st.server || st).version;
    if (version !== PAGE_VERSION && version !== reloadedFor && !midEdit()) {
      if (!reloading) {
        reloading = true;
        // A weight typed for the next shot comes along (see the start below).
        try { sessionStorage.setItem("gbs.reload", JSON.stringify({ version, weight: $("f-weight").value })); } catch (_) {}
        location.reload();
      }
      return;
    }
    // The server restarted: a new boot_id.
    const restarted = !!S && st.server.boot_id !== S.server.boot_id;
    S = st; stateAt = performance.now();
    if (restarted) {
      events = []; lastSeq = 0; seenAt.clear();
      ui.mine.clear(); ui.mineRange = null; ui.dismissed = 0; ui.planKey = null;
      kinds = null;
      restartLog();
    }
    // Its shots may have changed while it was away.
    if (primed && restarted) { histState = "unknown"; loadHistory(); }
    if (!kinds) loadKinds();
    let shotDone = false;
    let caughtUp = st.server.last_event_seq === lastSeq;
    if (!caughtUp) {
      try {
        const fresh = await api("/api/events?after=" + lastSeq);
        if (Array.isArray(fresh)) {
          for (const e of fresh) {
            if (e.seq <= lastSeq) continue;
            events.push(e);
            // The server says how long ago, so an event caught up after a gap
            // (phone asleep, server restarted) keeps its real time.
            if (typeof e.age_ms === "number") seenAt.set(e.seq, Date.now() - e.age_ms);
            if (e.kind === "shot_reported" || e.kind === "shot_aborted") shotDone = true;
            lastSeq = e.seq;
          }
          markMine();
          if (events.length > 500) {
            events.splice(0, events.length - 500);
            const min = events[0].seq;
            for (const k of seenAt.keys()) if (k < min) seenAt.delete(k);
            for (const k of ui.mine) if (k < min) ui.mine.delete(k);
          }
          lastSeq = Math.max(lastSeq, st.server.last_event_seq);
          caughtUp = true;
        }
      } catch (_) {}
    }
    // The first catch-up after the page opened loads the history; a finished
    // shot loads it again.
    if (!primed && caughtUp) {
      primed = true;
      loadHistory();
      // Opened at #log: the card could only open now, so scroll to it now.
      if (logShown()) openLog().then(() => { if (location.hash === "#log") scrollToLog(); });
    } else if (shotDone) loadHistory();
    else if (histState === "failed" && Date.now() - histTried > 5000) loadHistory();
    renderAll();
  }

  let ticking = false, again = false;
  async function tick() {
    if (ticking) { again = true; return; }
    ticking = true;
    try { do { again = false; await refresh(); } while (again); }
    finally { ticking = false; }
  }

  // Machine events arrive as server-sent events; /api/state is polled for the
  // rest (grinder link, integrations), less often while the stream is up.
  function connectEvents() {
    if (!window.EventSource) return;
    const es = new EventSource("/api/events/stream");
    const onEvent = () => tick();
    for (const k of ["grinder_grind_finished", "grinder_shot_requested", "shot_started", "shot_reported", "shot_aborted", "shot_finished", "message"]) {
      es.addEventListener(k, onEvent);
    }
    es.addEventListener("open", () => { sseOpen = true; tick(); });
    es.addEventListener("error", () => { sseOpen = false; if (es.readyState === 2) setTimeout(connectEvents, 5000); });
  }

  let lastPoll = 0;
  setInterval(() => {
    if (document.hidden) return;
    // Every second while the setup is open: it follows the pairing.
    const every = sseOpen && tab !== "setup" ? 3000 : 1000;
    if (performance.now() - lastPoll >= every - 50) { lastPoll = performance.now(); tick(); }
  }, 250);
  setInterval(() => { renderClock(); renderSetupTimer(); }, 250);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) tick(); });

  setLanguage();
  applyStatic();
  // Carried over the reload for an update (see refresh): the version it was
  // for, and a weight typed for the next shot.
  try {
    const r = JSON.parse(sessionStorage.getItem("gbs.reload") || "null");
    sessionStorage.removeItem("gbs.reload");
    if (r) { reloadedFor = r.version; if (r.weight) $("f-weight").value = r.weight; }
  } catch (_) {}
  showTab(location.hash.slice(1) || "start");
  tick();
  connectEvents();
})();
