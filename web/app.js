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
  // the wording for an E64 WS or an E65/E80 GbS (Generic WiFi), picked by tv().
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
  /** An E65/E80 setup: a Generic WiFi link, or an address it is starting with. */
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
  const TABS = ["start", "status", "history"];
  function showTab(name) {
    // The log is the last card on Status: #log opens it there.
    const toLog = name === "log";
    if (toLog) name = "status";
    if (!TABS.includes(name)) name = "start";
    const changed = name !== tab;
    tab = name;
    for (const t of TABS) $("tab-" + t).hidden = t !== name;
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
  const STOPS = ["offline", "noGrinder", "blocked"];
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
    if (wifi && wifi.needs_pairing_id) return s("start", "alert", "pairing", t("sys.text.needsId"));
    if (wifi && wifi.state === "pairing") return s("start", "alert", "pairing", t("sys.text.pairing"));
    if (linkStarting()) {
      const host = S.settings && S.settings.grinder_host;
      return s("start", "clock", "linking", host ? t("sys.text.linking", { host }) : t("sys.text.linkingNoHost"));
    }
    if (!linkOn()) {
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
      if (["noGrinder", "pairing", "linking", "problem"].includes(s.key)) { /* said above */ }
      else if (wifi && (wifi.state === "pairing" || wifi.needs_pairing_id)) grinder = ["wait", "clock", t("check.grinderPairing")];
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
    else if (wifi && (wifi.needs_pairing_id || wifi.state === "pairing")) b.hidden = true; // the pairing card says it
    else if (linkStarting()) b.hidden = true; // the system status says it
    else if (!linkOn()) { if (s.key === "noGrinder") b.hidden = true; else show("bad", grinderMissing(wifi).text); }
    else if (wifi && wifi.problem && s.key !== "problem" && !(f.sc === "error" && f.reason === wifi.problem)) show("warn", t("sys.problem", { problem: sentence(wifi.problem) }));
    else b.hidden = true;
  }

  function renderPairing() {
    const wifi = wifiOf();
    const pairing = !!(wifi && wifi.state === "pairing" && !wifi.needs_pairing_id);
    $("pair-card").hidden = !pairing;
    // The instructions are the app's own (data-i18n), so they follow the
    // language; the link's detail says the same in English, for the log.
    if (pairing) {
      $("pair-code-box").hidden = wifi.pairing_code == null;
      $("pair-code").textContent = wifi.pairing_code == null ? "" : String(wifi.pairing_code);
    }
    const needsId = !!(wifi && wifi.needs_pairing_id);
    $("pairing-form").hidden = !needsId;
    // An ID the grinder did not take, until it takes one.
    const unknown = needsId && wifi.unknown_pairing_id != null ? t("pair.id.unknown", { id: String(wifi.unknown_pairing_id) }) : "";
    if ($("pairing-unknown").textContent !== unknown) $("pairing-unknown").textContent = unknown;
    $("pairing-unknown").hidden = !unknown;
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
    renderPairing();

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
    if (stop && ["ready", "done", "error", "aborted"].includes(f.sc)) plan = [[sys.tone === "warn" ? "warn" : "error", "blocked"], ...plan.slice(1)];
    else if (f.sc === "ready" && (sys.key === "linking" || sys.key === "pairing")) plan = [["wait", "waiting"], ["none", ""], ["none", ""]];

    // Which step is open: the running or failed one opens by itself when the
    // shot moves on; otherwise step 1, or none during a stop (System status
    // tells why). A tap overrides it until then.
    const key = f.sc + ":" + (f.outcome ? f.outcome.seq : 0) + ":" + (f.job ? f.job.seq : 0) + ":" + (stop ? sys.key : "") + ":" + ui.typeIn;
    if (key !== ui.planKey) {
      ui.planKey = key;
      const auto = plan.findIndex(([tone]) => tone === "run" || tone === "error");
      ui.open = stop ? 0 : auto >= 0 ? auto + 1 : 1;
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
    let res = null;
    if (shown) {
      res = f.sc === "checking" ? S.shot.result : (f.outcome && f.outcome.result);
      $("s2-shot").textContent = res ? t("unit.shot", { time: secs(res.time_ms / 1000), weight: grams(res.weight_g) }) : "";
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
    if (shown && res) renderOutcome(res, tg, f.outcome && f.outcome.kind === "shot_reported" ? f.outcome : null);
    $("s3-skip").hidden = !(f.sc === "error" || f.sc === "aborted");
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
   *  the shot or (as in the design) the shot was on target. */
  function renderOutcome(res, tg, ev) {
    const secsShot = res.time_ms / 1000;
    const box = $("s3-outcome");
    const range = grinderRange();
    const inRange = (ev ? ev.in_grinder_range : inGrinderRange(res.time_ms)) !== false;
    let result, change = "", good = false;
    // in_grinder_range does not say on which side a shot missed, so the
    // time picks the wording: a shot too short also gets the hint to grind
    // finer by hand.
    if (range && res.time_ms <= range.min_ms) {
      result = t("outcome.tooShort", { time: secs(secsShot) });
      change = t("outcome.finerByHand");
    } else if (!inRange) {
      result = t("outcome.tooLong", { time: secs(secsShot) });
    } else if (!tg.t) {
      good = true;
      result = t("outcome.sent");
      change = t("outcome.compares");
    } else {
      const d = secsShot - tg.t;
      if (Math.abs(d) <= ON_TARGET_S) result = t("outcome.onTarget", { time: secs(secsShot), target: secs(tg.t) });
      else {
        good = true;
        const diff = { diff: secs(Math.abs(d)) };
        result = d < 0 ? t("outcome.fast", diff) : t("outcome.slow", diff);
        change = d < 0 ? t("outcome.finer") : t("outcome.coarser");
      }
    }
    box.classList.toggle("good", good);
    $("s3-result").textContent = result;
    $("s3-change-row").hidden = !change;
    $("s3-change").textContent = change;
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

  $("pairing-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const err = $("pairing-error");
    const id = Number($("f-pairing").value.trim());
    err.hidden = true;
    if (!Number.isInteger(id) || id < 1 || id > 65535) {
      err.textContent = t("pair.id.range"); err.hidden = false;
      return;
    }
    $("pairing-submit").disabled = true;
    try {
      await api("/api/grinder/pairing", { method: "POST", body: JSON.stringify({ pairing_id: id }) });
      $("f-pairing").value = "";
      await tick();
    } catch (x) {
      err.textContent = x.message; err.hidden = false;
    } finally { $("pairing-submit").disabled = false; }
  });

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
    if (wifi && wifi.needs_pairing_id) { tone = "start"; state = "needsId"; }
    else if (wifi && wifi.state === "pairing") { tone = "start"; state = "pairing"; }
    else if (on) { tone = "ok"; state = "ready"; }
    else if (linkStarting()) { tone = "start"; state = "connecting"; }
    setTone($("g-pill"), tone);
    $("g-pill-text").textContent = t("grinder.state." + state);
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

  // Integrations list. Rebuilt only when something shown changes; an open
  // edit form stays in place while the other cards change around it.
  let integSig = "", menuFor = null, editing = null, editForm = null, integOpen = store.get("integOpen") !== "0";
  /** Rebuilds the integrations list on the next render, and renders. */
  function invalidateIntegrations() { integSig = ""; renderStatus(); }
  /** Closes the ⋮ menu; `refocus` puts focus back on its button. */
  function closeMenu(refocus) {
    const id = menuFor;
    menuFor = null;
    invalidateIntegrations();
    if (refocus && id) focusKebab(id);
  }
  function focusKebab(id) {
    const b = [...document.querySelectorAll(".kebab")].find((x) => x.dataset.integ === id);
    if (b) b.focus();
  }
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
    const menuOpen = menuFor === i.id;
    const more = el("button", {
      type: "button", class: "kebab", "aria-label": t("integ.more", { title: i.title }), "data-integ": i.id,
      "aria-haspopup": "menu", "aria-expanded": String(menuOpen),
      onclick: (e) => {
        e.stopPropagation();
        if (menuOpen) { closeMenu(true); return; }
        menuFor = i.id; invalidateIntegrations(); focusMenu();
      },
    }, kebab());
    const actions = integActions(i);
    const head = el("div", { class: "integ-row" },
      iconFor(i.kind),
      el("div", { class: "integ-name" },
        el("p", { class: "integ-title", text: i.title }),
        i.subject ? el("p", { class: "integ-subject", text: i.subject }) : null),
      el("div", { class: "integ-end" },
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
    if (menuOpen) {
      item.append(el("div", { class: "menu", role: "menu", "aria-label": t("integ.menu", { title: i.title }) },
        actions.map(([name, ic, cls, run]) => el("button", {
          type: "button", role: "menuitem", class: cls,
          onclick: () => { closeMenu(false); run(); },
        }, icon(ic, 18, 2), name))));
    }
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

  $("integ-toggle").addEventListener("click", () => {
    integOpen = !integOpen; store.set("integOpen", integOpen ? "1" : "0"); menuFor = null; invalidateIntegrations();
  });

  /** Edit one from the start settings: it is moved into the app first. */
  function moveToEdit(i) {
    const k = kindOf(i.kind);
    if (!i.adoptable) {
      const noFile = S.settings && S.settings.persistent === false;
      return openSheet({
        returnTo: () => focusKebab(i.id),
        title: t("integ.cmdOnly.title", { title: i.title }),
        body: [el("p", { class: "body-text", text: t("integ.cmdOnly.text") + (noFile ? " " + t("integ.cmdOnly.noFile") : "") })],
        actions: [{ label: t("common.ok"), cls: "s-plain", run: closeSheet }],
      });
    }
    openSheet({
      returnTo: () => focusKebab(i.id),
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
      returnTo: () => focusKebab(i.id),
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
    const save = tEl("button", { type: "submit", class: "btn-send ok" }, "common.save");
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
  function stopEdit() { const id = editing; editing = null; editForm = null; invalidateIntegrations(); if (id) focusKebab(id); }

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
    if ($("set-host-form").hidden) $("set-host-row").hidden = false;
    renderWindow();
    renderFrom("set-window", "shot_window_s", (v) => (v === 0 ? t("settings.noLimit") : clock(v * 1000) + " min"));
    renderUpdateSetting();
    $("set-host-value").textContent = st.grinder_host ? st.grinder_host : t("settings.hostNone");
    renderFrom("set-host", "grinder_host", (v) => v || t("settings.none"));
    renderFrom("set-update", "update_check", (v) => (v ? t("settings.on") : t("settings.off")));
    $("set-foot").textContent = st.persistent ? t("settings.saved") : t("settings.notPersistent");
  }

  /** The line under a setting saved on the server: where its value comes
   *  from (`settings.source`), and when it was set in the app, the start
   *  setting it would go back to (`settings.start`, shown by `show`) with
   *  Use start setting. */
  function renderFrom(id, key, show) {
    const st = S.settings;
    const from = st.source && st.source[key];
    const start = st.start && key in st.start ? show(st.start[key]) : "";
    const text = from === "app" ? t("settings.from.app", { value: start })
      : from === "command_line" ? t("settings.from.start")
      : from === "default" ? t("settings.from.default") : "";
    const line = $(id + "-from-text");
    if (line.textContent !== text) line.textContent = text;
    $(id + "-from").hidden = !text;
    $(id + "-reset").hidden = from !== "app" || !st.start;
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

  function hostEditor(open) {
    $("set-host-form").hidden = !open;
    $("set-host-row").hidden = open;
    $("set-host-change").setAttribute("aria-expanded", String(open));
    $("set-host-error").hidden = true;
    $("set-host-reset-error").hidden = true;
    if (open) { $("f-host").value = (S.settings && S.settings.grinder_host) || ""; $("f-host").focus(); }
  }
  $("set-host-change").addEventListener("click", () => hostEditor(true));
  $("set-host-cancel").addEventListener("click", () => hostEditor(false));
  // An empty field removes the address ("" to the server, which without a
  // start address only forgets the app's one); Use start setting (null)
  // goes back to the start address.
  $("set-host-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const v = $("f-host").value.trim();
    if (v === (S.settings.grinder_host || "")) { hostEditor(false); return; }   // unchanged
    $("set-host-save").disabled = true; $("set-host-error").hidden = true;
    try {
      const st = await api("/api/settings", { method: "PUT", body: JSON.stringify({ grinder_host: v }) });
      if (st && "grinder_host" in st) S.settings = st;
      hostEditor(false);
      await tick();
    } catch (x) {
      $("set-host-error").textContent = t("common.notSaved", { error: x.message }); $("set-host-error").hidden = false;
    } finally { $("set-host-save").disabled = false; }
  });
  resetButton("set-host", async () => {
    $("set-host-reset-error").hidden = true;
    try {
      const st = await api("/api/settings", { method: "PUT", body: JSON.stringify({ grinder_host: null }) });
      if (st && "grinder_host" in st) S.settings = st;
      renderSettings();
      tick();
      return true;
    } catch (x) {
      $("set-host-reset-error").textContent = t("common.notSaved", { error: x.message }); $("set-host-reset-error").hidden = false;
      return false;
    }
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
    const d = new Date(l.ts);
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
      const d = new Date(l.ts);
      return (isNaN(d) ? String(l.ts) : stamp(d) + "." + String(d.getMilliseconds()).padStart(3, "0")) + " "
        + String(l.level).toUpperCase().padEnd(5) + " " + scrub(String(l.text || ""));
    });
    const first = lines.length ? new Date(lines[0].ts) : new Date();
    const last = lines.length ? new Date(lines[lines.length - 1].ts) : new Date();
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
    let lines = log.lines.filter((l) => Date.parse(l.ts) >= since && (!log.warnOnly || l.level !== "info"));
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
    return !!(editing || addingKind || win.timer || win.pending != null || !$("set-host-form").hidden || !$("sheet").hidden
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
    const every = sseOpen ? 3000 : 1000;
    if (performance.now() - lastPoll >= every - 50) { lastPoll = performance.now(); tick(); }
  }, 250);
  setInterval(renderClock, 250);
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
