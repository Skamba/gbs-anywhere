//! The task: log in, find the machine, and for each brew the grinder asks
//! for poll its coffee log until the shot shows up.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::bail;

use super::TITLE;
use super::cloud::{CloudClient, LastCoffee, Permanent, Thing};
use super::config::Config;
use crate::integration::{Backoff, BoxFuture, Brew, Detail, Integration, Link, tenths};

const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// The wait after an error that trying again soon won't fix
/// ([`is_permanent`]). Changing the settings in the app restarts the
/// integration at once.
const PERMANENT_RETRY: Duration = Duration::from_secs(3600);
/// How long refusals in a row get the normal backoff before the hour: a
/// login signed while this computer's clock was still unset can be refused
/// once and work a minute later. Counted from the first refusal rather than
/// in tries, so it holds whatever the backoff's steps.
const REFUSED_BEFORE_THE_HOUR: Duration = Duration::from_secs(120);
/// How often an error that keeps coming back unchanged is logged again.
const REPEAT_REMINDER: Duration = Duration::from_secs(3600);

/// The integration. See the module docs.
pub(crate) struct LaMarzocco {
    pub(crate) cfg: Config,
}

impl Integration for LaMarzocco {
    fn run(self: Box<Self>, link: Link) -> BoxFuture {
        Box::pin(run(link, self.cfg))
    }
}

// ---------------------------------------------------------------------------

/// The coffee to report: the newest one logged after `after_ms`, if valid.
/// Invalid ones (the cloud's own judgement) are skipped so the grinder keeps
/// waiting for a real shot.
pub(crate) fn new_coffee(coffees: &[LastCoffee], after_ms: i64) -> Option<&LastCoffee> {
    coffees
        .iter()
        .filter(|c| c.time > after_ms)
        .max_by_key(|c| c.time)
        .filter(|c| c.valid)
}

async fn run(link: Link, cfg: Config) {
    let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
    // When the first of the refusals in a row came, if the last error was one.
    let mut refused_since: Option<Instant> = None;
    let mut repeats = Repeats::default();
    loop {
        link.status
            .starting(Detail::keyed("la_marzocco.logging_in", "logging in"));
        let e = match connect(&cfg).await {
            Ok((client, machine)) => {
                backoff.reset();
                refused_since = None;
                repeats.clear();
                link.status.subject(&machine.subject);
                link.status
                    .connected(idle_line(machine.last.as_ref(), now_ms()));
                match watch(&link, &cfg, client, machine).await {
                    Ok(()) => return,
                    Err(e) => e,
                }
            }
            Err(e) => e,
        };
        // A wrong or missing serial: the card lists the machines in full,
        // the log only by their last four characters.
        let logged = repeats.log(&match e.downcast_ref::<MachineChoice>() {
            Some(choice) => choice.log.clone(),
            None => format!("{e:#}"),
        });
        let now = Instant::now();
        refused_since = is_permanent(&e).then(|| refused_since.unwrap_or(now));
        if refused_since.is_some_and(|first| waits_an_hour(first, now)) {
            link.status.error(format!("{e:#}; trying again in an hour"));
            tracing::info!("{TITLE}: trying again in an hour, or once its settings change");
            tokio::time::sleep(PERMANENT_RETRY).await;
        } else {
            link.status.error(format!("{e:#}"));
            if logged {
                backoff.retry(TITLE).await;
            } else {
                backoff.retry_quietly(TITLE).await;
            }
        }
    }
}

/// Keeps an error that comes back unchanged out of the log, as the cloud
/// can refuse every request for hours: the first one is logged, the same one
/// again only after [`REPEAT_REMINDER`], with how often it came meanwhile.
/// The rest go to the debug log; the card always shows the latest.
#[derive(Debug, Default)]
struct Repeats {
    /// The last error logged, and when.
    last: Option<(String, Instant)>,
    /// How often it came since, unlogged.
    unlogged: u32,
}

impl Repeats {
    /// The line to log for `error` at `now`, or `None` to leave it out.
    fn note(&mut self, error: &str, now: Instant) -> Option<String> {
        let line = match &self.last {
            Some((last, at)) if last == error => {
                let since = now.saturating_duration_since(*at);
                if since < REPEAT_REMINDER {
                    self.unlogged += 1;
                    return None;
                }
                let since_ms = i64::try_from(since.as_millis()).unwrap_or(i64::MAX);
                match self.unlogged {
                    0 => error.to_owned(),
                    1 => format!(
                        "{error} (1 more time since it was logged {} ago)",
                        age_of(since_ms)
                    ),
                    n => format!(
                        "{error} ({n} more times since it was logged {} ago)",
                        age_of(since_ms)
                    ),
                }
            }
            _ => error.to_owned(),
        };
        self.last = Some((error.to_owned(), now));
        self.unlogged = 0;
        Some(line)
    }

    /// Logs `error` as a warning, or only to the debug log while it repeats;
    /// whether it was logged.
    fn log(&mut self, error: &str) -> bool {
        match self.note(error, Instant::now()) {
            Some(line) => {
                tracing::warn!("{TITLE}: {line}");
                true
            }
            None => {
                tracing::debug!("{TITLE}: {error}");
                false
            }
        }
    }

    /// After a success: the next error is logged again. Whether one was
    /// logged before, so the log can say it is over.
    fn clear(&mut self) -> bool {
        std::mem::take(self).last.is_some()
    }
}

/// Whether permanent errors in a row since `first` wait the hour at `now`
/// rather than the normal backoff.
fn waits_an_hour(first: Instant, now: Instant) -> bool {
    now.saturating_duration_since(first) >= REFUSED_BEFORE_THE_HOUR
}

/// Whether trying again in a few minutes won't fix `e`: the login was
/// refused, or which machine to use is unclear.
fn is_permanent(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Permanent>().is_some() || e.downcast_ref::<MachineChoice>().is_some()
}

struct Machine {
    serial: String,
    /// `MI000000 · Linea Mini R`.
    subject: String,
    last: Option<Idle>,
}

/// The newest coffee we know of, for the idle status line.
#[derive(Debug, Clone, Copy)]
struct Idle {
    time_ms: i64,
    extraction_seconds: f64,
    grams: Option<f64>,
}

impl From<&LastCoffee> for Idle {
    fn from(c: &LastCoffee) -> Self {
        Self {
            time_ms: c.time,
            extraction_seconds: c.extraction_seconds,
            grams: c.dose_value,
        }
    }
}

/// How far this computer's clock may be behind the cloud's before the idle
/// line leaves the coffee's age out rather than show `0 s ago`.
const CLOCK_SLACK_MS: i64 = 60_000;

/// `last coffee 3 min ago: 25.1 s, 38.0 g`, with the age as of `now_ms`.
/// That is this computer's clock, which only words the age here; when it is
/// well behind the cloud's (or not set), the line has no age.
fn idle_line(last: Option<&Idle>, now_ms: i64) -> Detail {
    let Some(c) = last else {
        return Detail::keyed("la_marzocco.no_coffee", "no coffee in the last 7 days");
    };
    let seconds = tenths(c.extraction_seconds);
    let grams = c.grams.map(tenths);
    let age_ms = now_ms.saturating_sub(c.time_ms);
    let line = if age_ms < -CLOCK_SLACK_MS {
        let time = format!("last coffee: {seconds:.1} s");
        match grams {
            Some(g) => Detail::keyed(
                "la_marzocco.last_coffee_undated",
                format!("{time}, {g:.1} g"),
            )
            .var("weight_g", g),
            None => Detail::keyed("la_marzocco.last_coffee_undated_no_weight", time),
        }
    } else {
        let age_ms = age_ms.max(0);
        let time = format!("last coffee {} ago: {seconds:.1} s", age_of(age_ms));
        match grams {
            Some(g) => Detail::keyed("la_marzocco.last_coffee", format!("{time}, {g:.1} g"))
                .var("weight_g", g),
            None => Detail::keyed("la_marzocco.last_coffee_no_weight", time),
        }
        .var("age_ms", age_ms)
    };
    line.var("time_ms", (seconds * 1000.0).round() as i64)
}

/// How often the idle line's age is refreshed.
const IDLE_REFRESH: Duration = Duration::from_secs(60);

/// Registers, logs in, finds the machine and reads its last coffee.
async fn connect(cfg: &Config) -> anyhow::Result<(CloudClient, Machine)> {
    let mut client = CloudClient::new(cfg)?;
    tracing::debug!("{TITLE}: installation {}", client.installation_id());
    client.register().await?;
    let things = client.things().await?;
    let machines: Vec<&Thing> = things
        .iter()
        .filter(|t| t.r#type.is_empty() || t.r#type == "CoffeeMachine")
        .collect();
    let machine = match &cfg.serial {
        Some(sn) => {
            let sn = sn.trim().to_uppercase();
            machines
                .iter()
                .find(|t| t.serial_number.eq_ignore_ascii_case(&sn))
                .copied()
                .ok_or_else(|| {
                    MachineChoice::new(|full| {
                        format!(
                            "machine {} not on this account (found: {})",
                            show_serial(&sn, full),
                            list_serials(&machines, full)
                        )
                    })
                })?
        }
        None => match machines.as_slice() {
            [one] => one,
            [] => return Err(Permanent::new("no coffee machine on this account").into()),
            many => {
                return Err(MachineChoice::new(|full| {
                    format!(
                        "more than one machine on this account, set the serial number: {}",
                        list_serials(many, full)
                    )
                })
                .into());
            }
        },
    };
    let serial = machine.serial_number.clone();
    let coffees = client.last_coffees(&serial, 7).await?;
    let last = coffees.first().map(Idle::from);
    let subject = format!("{serial} · {}", machine.model_name);
    tracing::info!(
        "{TITLE}: connected to {} (serial ending {}), {}",
        machine.model_name,
        tail(&serial),
        idle_line(last.as_ref(), now_ms()).text()
    );
    Ok((
        client,
        Machine {
            serial,
            subject,
            last,
        },
    ))
}

fn list_serials(machines: &[&Thing], full: bool) -> String {
    machines
        .iter()
        .map(|t| format!("{} ({})", show_serial(&t.serial_number, full), t.model_name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which machine to use is unclear. The card names the machines by their
/// serial numbers in full, as the person needs them to pick one; the log,
/// which may end up in a bug report, only by their last four characters.
#[derive(Debug)]
struct MachineChoice {
    card: String,
    log: String,
}

impl MachineChoice {
    /// `text(full)` words the error with the serials in full or cut short.
    fn new(text: impl Fn(bool) -> String) -> Self {
        Self {
            card: text(true),
            log: text(false),
        }
    }
}

impl std::fmt::Display for MachineChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.card)
    }
}

impl std::error::Error for MachineChoice {}

/// A serial's last four characters: enough to tell machines apart.
fn tail(serial: &str) -> &str {
    serial.get(serial.len().saturating_sub(4)..).unwrap_or("")
}

/// A serial number in full, or as `…1234` for the log.
fn show_serial(serial: &str, full: bool) -> String {
    if full {
        serial.to_owned()
    } else {
        format!("…{}", tail(serial))
    }
}

/// This computer's clock, only to word the idle line's age: coffees are
/// told apart on the cloud's clock alone.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// `45 s`, `12 min`, `5 h`, `2 days`. The page words it in the same steps
/// (`ageOf`), so German shows the same number.
fn age_of(delta_ms: i64) -> String {
    let s = delta_ms.max(0) / 1000;
    if s < 120 {
        format!("{s} s")
    } else if s < 7200 {
        format!("{} min", s / 60)
    } else if s < 48 * 3600 {
        format!("{} h", s / 3600)
    } else {
        format!("{} days", s / 86_400)
    }
}

/// What the coffee log has shown so far.
struct Seen {
    /// The newest coffee's time, on the cloud's clock (never this
    /// computer's, which may be unset or off). Only a newer one can be a
    /// brew's shot.
    newest_ms: Option<i64>,
    /// For the idle line.
    idle: Option<Idle>,
}

impl Seen {
    /// Takes in a fetched list, in whatever order the cloud sent it.
    fn note(&mut self, list: &[LastCoffee]) {
        if let Some(t) = list.iter().map(|c| c.time).max() {
            self.newest_ms = Some(self.newest_ms.map_or(t, |n| n.max(t)));
        }
    }
}

/// Waits for brews and polls the coffee log during each.
async fn watch(
    link: &Link,
    cfg: &Config,
    mut client: CloudClient,
    machine: Machine,
) -> anyhow::Result<()> {
    // A brew the grinder already waits on (pressed during the login) comes
    // first.
    let mut brews = link.grinder_brews();
    let mut seen = Seen {
        newest_ms: machine.last.map(|c| c.time_ms),
        idle: machine.last,
    };
    let mut refresh = tokio::time::interval(IDLE_REFRESH);
    refresh.tick().await; // the first tick fires at once
    let mut repeats = Repeats::default();
    loop {
        let brew = tokio::select! {
            next = brews.next() => match next {
                Some(brew) => brew,
                None => return Ok(()),
            },
            _ = refresh.tick() => {
                link.status.connected(idle_line(seen.idle.as_ref(), now_ms()));
                continue;
            }
        };
        watch_brew(
            link,
            cfg,
            &mut client,
            &machine.serial,
            brew,
            &mut seen,
            &mut repeats,
        )
        .await?;
        link.status
            .connected(idle_line(seen.idle.as_ref(), now_ms()));
    }
}

/// Polls the coffee log for `brew` until its coffee shows up or the grinder
/// stops waiting for it. The first fetch that succeeds is the baseline:
/// whatever the log holds then is not this shot. Until one has succeeded no
/// coffee is taken, as one made earlier without a knob press may be among
/// them: a missed shot, which the person can type, is better than a wrong
/// one.
async fn watch_brew(
    link: &Link,
    cfg: &Config,
    client: &mut CloudClient,
    serial: &str,
    brew: Brew,
    seen: &mut Seen,
    repeats: &mut Repeats,
) -> anyhow::Result<()> {
    let poll_s = cfg.poll.as_secs();
    tracing::info!("{TITLE}: grinder is waiting, watching the coffee log every {poll_s} s");
    let waiting = Detail::keyed(
        "la_marzocco.watching",
        format!("grinder is waiting · checking the coffee log every {poll_s} s"),
    )
    .var("every_s", poll_s);
    link.status.watching(waiting.clone());
    let mut baseline = false;
    let mut errors = 0u32;
    // The baseline at once, then every `poll`.
    let mut pause = Duration::ZERO;
    loop {
        tokio::time::sleep(pause).await;
        pause = cfg.poll;
        if !link.waits_for(brew) {
            tracing::info!("{TITLE}: brew ended without a coffee from the log");
            return Ok(());
        }
        let list = match client.last_coffees(serial, 1).await {
            Ok(l) => {
                if repeats.clear() {
                    tracing::info!("{TITLE}: coffee log answers again");
                }
                if errors > 0 {
                    // Back from a failed fetch: still watching this brew.
                    errors = 0;
                    link.status.watching(waiting.clone());
                }
                l
            }
            Err(e) => {
                errors += 1;
                repeats.log(&format!("{e:#}"));
                link.status.error(format!("{e:#}"));
                if errors >= 10 {
                    bail!("giving up on this session after {errors} errors");
                }
                continue;
            }
        };
        let after_ms = seen.newest_ms.unwrap_or(i64::MIN);
        seen.note(&list);
        if !baseline {
            baseline = true;
            continue;
        }
        let Some(c) = new_coffee(&list, after_ms) else {
            if let Some(c) = list.iter().find(|c| c.time > after_ms) {
                tracing::info!(
                    "{TITLE}: ignoring coffee marked invalid ({}): {:.1} s",
                    c.invalid_reason.as_deref().unwrap_or("no reason"),
                    c.extraction_seconds
                );
            }
            continue;
        };
        let time = Duration::from_millis((c.extraction_seconds.max(0.0) * 1000.0).round() as u64);
        // The cloud sends float32 noise like 37.4000015; the scale reads 0.1 g.
        let weight = c
            .dose_value
            .map(|g| (tenths(g), "weighed by the machine".to_owned()));
        link.report(brew, time, weight);
        seen.idle = Some(Idle::from(c));
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Json, Response};
    use axum::routing::{get, post};
    use serde_json::{Value, json};

    use super::*;
    use crate::integration::la_marzocco::cloud::parse_last_coffees;
    use crate::integration::{Health, Status};
    use crate::protocol::MachineConfig;
    use crate::server::Server;

    #[test]
    fn the_newest_coffee_is_found_in_any_order() {
        let list: Vec<LastCoffee> = [5_000, 9_000, 7_000]
            .into_iter()
            .map(|t| serde_json::from_value(coffee(t, 25.0, 36.0)).unwrap())
            .collect();
        let mut seen = Seen {
            newest_ms: Some(1_000),
            idle: None,
        };
        seen.note(&list);
        assert_eq!(seen.newest_ms, Some(9_000));
        seen.note(&list[..1]);
        assert_eq!(seen.newest_ms, Some(9_000), "never goes back");
    }

    #[test]
    fn an_error_that_keeps_coming_is_logged_once_an_hour() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let e = "machine list failed: 403 Forbidden";
        let mut r = Repeats::default();
        assert_eq!(r.note(e, t0).as_deref(), Some(e));
        // Every 5 minutes for the rest of the hour: left out.
        for n in 1..12 {
            assert_eq!(r.note(e, at(n * 300)), None, "try {n}");
        }
        assert_eq!(
            r.note(e, at(3600)).as_deref(),
            Some(
                "machine list failed: 403 Forbidden (11 more times since it was logged 60 min ago)"
            )
        );
        // An hour on without a repeat in between (a longer wait): no count.
        assert_eq!(r.note(e, at(7200)).as_deref(), Some(e));
        assert_eq!(r.note(e, at(7500)), None, "the hour starts again");
        // Another error is logged at once, and so is the first one back.
        let other = "coffee log failed: 502 Bad Gateway";
        assert_eq!(r.note(other, at(7503)).as_deref(), Some(other));
        assert_eq!(r.note(e, at(7506)).as_deref(), Some(e));
        // After a success, the same error is new again.
        assert!(r.clear(), "one was logged");
        assert!(!r.clear(), "none since");
        assert_eq!(r.note(e, at(7509)).as_deref(), Some(e));
        // A single repeat in between.
        assert_eq!(r.note(e, at(7510)), None);
        assert_eq!(
            r.note(e, at(7509 + 3600)).as_deref(),
            Some("machine list failed: 403 Forbidden (1 more time since it was logged 60 min ago)")
        );
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age_of(45_000), "45 s");
        assert_eq!(age_of(76 * 60_000), "76 min");
        assert_eq!(age_of(5 * 3_600_000), "5 h");
        assert_eq!(age_of(47 * 3_600_000), "47 h");
        assert_eq!(age_of(3 * 86_400_000 + 5), "3 days");
        assert_eq!(age_of(-5), "0 s");
    }

    #[test]
    fn status_lines_carry_a_key_and_their_numbers() {
        let now = 1_790_536_239_820;
        let status = Status::new("la_marzocco", "la_marzocco", TITLE);
        let shown = |line: Detail| {
            status.connected(line);
            status.snapshot()
        };

        let coffee = Idle {
            time_ms: now - 5 * 3_600_000,
            extraction_seconds: 28.491_999,
            grams: Some(40.900_001),
        };
        let snap = shown(idle_line(Some(&coffee), now));
        assert_eq!(snap.detail, "last coffee 5 h ago: 28.5 s, 40.9 g");
        assert_eq!(snap.detail_key, Some("la_marzocco.last_coffee"));
        assert_eq!(
            Value::Object(snap.detail_vars),
            json!({"age_ms": 18_000_000, "time_ms": 28_500, "weight_g": 40.9})
        );

        // No Connected Scale: no weight.
        let no_scale = Idle {
            time_ms: now - 90_000,
            grams: None,
            ..coffee
        };
        let snap = shown(idle_line(Some(&no_scale), now));
        assert_eq!(snap.detail, "last coffee 90 s ago: 28.5 s");
        assert_eq!(snap.detail_key, Some("la_marzocco.last_coffee_no_weight"));
        assert_eq!(
            Value::Object(snap.detail_vars),
            json!({"age_ms": 90_000, "time_ms": 28_500})
        );

        let snap = shown(idle_line(None, now));
        assert_eq!(snap.detail, "no coffee in the last 7 days");
        assert_eq!(snap.detail_key, Some("la_marzocco.no_coffee"));
        assert!(snap.detail_vars.is_empty());
    }

    #[test]
    fn a_clock_behind_the_clouds_gives_no_age() {
        let coffee = Idle {
            time_ms: 1_790_536_239_820,
            extraction_seconds: 25.1,
            grams: Some(38.0),
        };
        // A little behind: just now.
        let line = idle_line(Some(&coffee), coffee.time_ms - 30_000);
        assert_eq!(line.text(), "last coffee 0 s ago: 25.1 s, 38.0 g");
        // Not set (1970), or far behind: no age rather than a wrong one.
        let status = Status::new("la_marzocco", "la_marzocco", TITLE);
        for now in [0, coffee.time_ms - 3_600_000] {
            status.connected(idle_line(Some(&coffee), now));
            let snap = status.snapshot();
            assert_eq!(snap.detail, "last coffee: 25.1 s, 38.0 g");
            assert_eq!(snap.detail_key, Some("la_marzocco.last_coffee_undated"));
            assert_eq!(
                Value::Object(snap.detail_vars),
                json!({"time_ms": 25_100, "weight_g": 38.0})
            );
        }
        let no_scale = Idle {
            grams: None,
            ..coffee
        };
        let line = idle_line(Some(&no_scale), 0);
        assert_eq!(line.text(), "last coffee: 25.1 s");
        status.connected(line);
        assert_eq!(
            status.snapshot().detail_key,
            Some("la_marzocco.last_coffee_undated_no_weight")
        );
    }

    #[test]
    fn parses_and_picks_the_new_coffee() {
        let raw = json!({"output": {"lastCoffees": [
            {"time": 1790536239820i64, "extractionSeconds": 22.091999, "doseMode": "MassType",
             "doseIndex": "DoseA", "doseValue": 37.9, "doseValueNumerator": null,
             "targetTemperature": 93, "valid": true, "invalidReason": null},
            {"time": 1790535892774i64, "extractionSeconds": 24.777, "doseMode": "MassType",
             "doseIndex": "DoseA", "doseValue": 38, "valid": true},
            {"time": 1790535000000i64, "extractionSeconds": 3.0, "valid": false,
             "invalidReason": "TooShort"}
        ]}});
        let list = parse_last_coffees(&raw).unwrap();
        assert_eq!(list[0].dose_value, Some(37.9));
        assert!((list[0].extraction_seconds - 22.092).abs() < 1e-3);
        assert!(!list[2].valid);

        // Nothing newer than the newest: keep waiting.
        assert_eq!(new_coffee(&list, 1790536239820), None);
        // The shot after the knob press.
        assert_eq!(
            new_coffee(&list, 1790535892774).unwrap().time,
            1790536239820
        );
        // Two new ones: the newest wins.
        assert_eq!(new_coffee(&list, 0).unwrap().time, 1790536239820);
        // Only an invalid one is new: skipped.
        let only_invalid = vec![list[2].clone()];
        assert_eq!(new_coffee(&only_invalid, 0), None);
    }

    #[test]
    fn the_log_gets_only_the_serials_last_digits() {
        let things: Vec<Thing> = serde_json::from_value(json!([
            {"serialNumber": "MI000012", "modelName": "Linea Mini R"},
            {"serialNumber": "GS000034", "modelName": "GS3"},
        ]))
        .unwrap();
        let machines: Vec<&Thing> = things.iter().collect();
        let e = anyhow::Error::from(MachineChoice::new(|full| list_serials(&machines, full)));
        assert_eq!(format!("{e:#}"), "MI000012 (Linea Mini R), GS000034 (GS3)");
        let choice = e.downcast_ref::<MachineChoice>().unwrap();
        assert_eq!(choice.log, "…0012 (Linea Mini R), …0034 (GS3)");
        assert_eq!(show_serial("AB", false), "…AB");
    }

    // -----------------------------------------------------------------------
    // The task against a fake cloud on a local port
    // -----------------------------------------------------------------------

    const SERIAL: &str = "MI000001";

    /// What the fake cloud holds and saw.
    #[derive(Default)]
    struct Cloud {
        /// The machines on the account (`GET /things`).
        things: Mutex<Value>,
        /// The coffee log, newest first, as `lastCoffees` entries.
        coffees: Mutex<Vec<Value>>,
        /// This many coffee log requests fail (502) before they work again.
        failing: AtomicU32,
        /// Coffee log requests so far.
        asked: AtomicU32,
        refuse_login: AtomicBool,
        /// The machine list is refused until new terms are confirmed.
        ask_terms: AtomicBool,
    }

    impl Cloud {
        fn asked(&self) -> u32 {
            self.asked.load(Ordering::SeqCst)
        }

        fn log(&self, coffee: Value) {
            self.coffees.lock().unwrap().insert(0, coffee);
        }
    }

    fn coffee(time: i64, seconds: f64, grams: f64) -> Value {
        json!({"time": time, "extractionSeconds": seconds, "doseValue": grams, "valid": true})
    }

    async fn signin(State(c): State<Arc<Cloud>>) -> Response {
        if c.refuse_login.load(Ordering::SeqCst) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Json(json!({"accessToken": "access", "refreshToken": "refresh"})).into_response()
    }

    async fn things(State(c): State<Arc<Cloud>>) -> Response {
        if c.ask_terms.load(Ordering::SeqCst) {
            let reply = json!({"success": false, "message": "User must accept latest policy", "code": "USER_MUST_ACCEPT_POLICY"});
            return (StatusCode::FORBIDDEN, Json(reply)).into_response();
        }
        Json(c.things.lock().unwrap().clone()).into_response()
    }

    async fn last_coffees(State(c): State<Arc<Cloud>>) -> Response {
        c.asked.fetch_add(1, Ordering::SeqCst);
        let failing = c.failing.load(Ordering::SeqCst);
        if failing > 0 {
            c.failing.store(failing - 1, Ordering::SeqCst);
            return StatusCode::BAD_GATEWAY.into_response();
        }
        let list = c.coffees.lock().unwrap().clone();
        Json(json!({"output": {"lastCoffees": list}})).into_response()
    }

    /// The customer-app API's routes the task uses, on a local port, with
    /// one machine on the account. Returns it and its base URL.
    async fn fake_cloud() -> (Arc<Cloud>, String) {
        let cloud = Arc::new(Cloud::default());
        *cloud.things.lock().unwrap() =
            json!([{"serialNumber": SERIAL, "type": "CoffeeMachine", "modelName": "Linea Mini R"}]);
        let app = Router::new()
            .route("/auth/init", post(|| async { "{}" }))
            .route("/auth/signin", post(signin))
            .route("/things", get(things))
            .route("/things/{serial}/stats/LAST_COFFEE/1", get(last_coffees))
            .with_state(cloud.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (cloud, format!("http://{addr}"))
    }

    fn config(base_url: &str) -> Config {
        Config {
            username: "me@example.org".into(),
            password: "secret".into(),
            serial: None,
            poll: Duration::from_millis(50),
            base_url: base_url.into(),
        }
    }

    async fn wait_for(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..250 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// The task, logged in to the fake cloud at `base_url` and idle.
    async fn start(base_url: &str) -> Link {
        let link = Link {
            server: Server::new(MachineConfig::default()),
            status: Status::new("la_marzocco", "la_marzocco", TITLE),
        };
        tokio::spawn(run(link.clone(), config(base_url)));
        wait_for("connected", || {
            link.status.snapshot().health == Health::Connected
        })
        .await;
        link
    }

    fn knob_press(link: &Link) -> Brew {
        link.server.with(|m, now| m.on_start_request(now, Some(9)));
        link.waiting_brew().expect("the grinder waits")
    }

    /// Lets the task look at the log `n` more times.
    async fn polls(cloud: &Cloud, n: u32) {
        let asked = cloud.asked();
        wait_for("polls", || cloud.asked() >= asked + n).await;
    }

    #[tokio::test]
    async fn reports_the_coffee_made_after_the_knob_press() {
        let (cloud, base) = fake_cloud().await;
        let t0 = now_ms();
        cloud.log(coffee(t0 - 3_600_000, 27.0, 36.0));
        let l = start(&base).await;
        assert_eq!(
            l.status.snapshot().detail_key,
            Some("la_marzocco.last_coffee")
        );
        knob_press(&l);
        polls(&cloud, 2).await;
        assert_eq!(l.status.snapshot().health, Health::Watching);
        assert_eq!(l.status.snapshot().reports, 0, "the old coffee is not it");

        cloud.log(coffee(t0 + 30_000, 25.14, 38.04));
        wait_for("the report", || l.status.snapshot().reports == 1).await;
        let shot = l.status.snapshot().last_report.unwrap();
        assert_eq!((shot.time_ms, shot.weight_g), (25_140, 38.0));
        wait_for("idle", || l.status.snapshot().health == Health::Connected).await;
        assert!(l.status.snapshot().detail.starts_with("last coffee "));
    }

    #[tokio::test]
    async fn a_failed_baseline_takes_no_older_coffee() {
        let (cloud, base) = fake_cloud().await;
        let t0 = now_ms();
        cloud.log(coffee(t0 - 3_600_000, 27.0, 36.0));
        let l = start(&base).await;
        // A coffee made without a knob press, then a knob press whose first
        // look at the log fails.
        cloud.log(coffee(t0 + 1_000, 24.0, 38.0));
        cloud.failing.store(1, Ordering::SeqCst);
        knob_press(&l);
        polls(&cloud, 5).await;
        let snap = l.status.snapshot();
        assert_eq!(
            snap.reports, 0,
            "the coffee before the knob press is not it"
        );
        assert_eq!(snap.health, Health::Watching, "back from the failed look");

        // The shot itself still counts.
        cloud.log(coffee(t0 + 60_000, 26.0, 37.5));
        wait_for("the report", || l.status.snapshot().reports == 1).await;
        let shot = l.status.snapshot().last_report.unwrap();
        assert_eq!((shot.time_ms, shot.weight_g), (26_000, 37.5));
    }

    #[tokio::test]
    async fn no_coffee_is_taken_while_the_log_cannot_be_read() {
        let (cloud, base) = fake_cloud().await;
        let l = start(&base).await;
        cloud.failing.store(4, Ordering::SeqCst);
        knob_press(&l);
        // The shot is logged while every look fails: missed, not guessed.
        cloud.log(coffee(now_ms(), 25.0, 38.0));
        polls(&cloud, 7).await;
        assert_eq!(l.status.snapshot().reports, 0);
    }

    #[tokio::test]
    async fn a_new_brew_does_not_take_the_last_ones_coffee() {
        let (cloud, base) = fake_cloud().await;
        let t0 = now_ms();
        let l = start(&base).await;
        let first = knob_press(&l);
        polls(&cloud, 1).await;
        // The person types the first shot, its coffee reaches the log, and
        // the next brew starts, all before the task looks again.
        l.report(first, Duration::from_secs(25), Some((38.0, "typed".into())));
        cloud.log(coffee(t0 + 1_000, 25.0, 38.0));
        let second = knob_press(&l);
        assert_ne!(first, second);
        polls(&cloud, 5).await;
        assert_eq!(l.status.snapshot().reports, 1, "only the typed one");

        cloud.log(coffee(t0 + 60_000, 29.0, 40.0));
        wait_for("the report", || l.status.snapshot().reports == 2).await;
        let shot = l.status.snapshot().last_report.unwrap();
        assert_eq!((shot.time_ms, shot.weight_g), (29_000, 40.0));
    }

    #[tokio::test]
    async fn a_knob_press_during_the_login_is_watched() {
        let (cloud, base) = fake_cloud().await;
        let l = Link {
            server: Server::new(MachineConfig::default()),
            status: Status::new("la_marzocco", "la_marzocco", TITLE),
        };
        knob_press(&l);
        tokio::spawn(run(l.clone(), config(&base)));
        wait_for("watching", || {
            l.status.snapshot().health == Health::Watching
        })
        .await;
        polls(&cloud, 2).await;
        cloud.log(coffee(now_ms(), 25.0, 38.0));
        wait_for("the report", || l.status.snapshot().reports == 1).await;
    }

    /// Connects, for the error.
    async fn connect_error(cfg: &Config) -> anyhow::Error {
        match connect(cfg).await {
            Ok(_) => panic!("connected"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn errors_that_retrying_wont_fix_are_told_apart() {
        let (cloud, base) = fake_cloud().await;
        let cfg = config(&base);
        cloud.refuse_login.store(true, Ordering::SeqCst);
        let e = connect_error(&cfg).await;
        assert!(is_permanent(&e), "{e:#}");
        assert!(format!("{e:#}").contains("login refused"), "{e:#}");
        cloud.refuse_login.store(false, Ordering::SeqCst);

        let other = Config {
            serial: Some("GS000099".into()),
            ..cfg.clone()
        };
        assert!(is_permanent(&connect_error(&other).await));
        let two = json!([
            {"serialNumber": SERIAL, "modelName": "Linea Mini R"},
            {"serialNumber": "GS000034", "modelName": "GS3"},
        ]);
        *cloud.things.lock().unwrap() = two;
        assert!(is_permanent(&connect_error(&cfg).await));
        *cloud.things.lock().unwrap() = json!([]);
        assert!(is_permanent(&connect_error(&cfg).await));

        // A cloud that fails now and then is tried again soon.
        *cloud.things.lock().unwrap() = json!([{"serialNumber": SERIAL}]);
        cloud.failing.store(1, Ordering::SeqCst);
        let e = connect_error(&cfg).await;
        assert!(!is_permanent(&e), "{e:#}");
        // New terms to confirm too: confirming them in the app ends it, and
        // the next try within 5 minutes connects.
        cloud.ask_terms.store(true, Ordering::SeqCst);
        let e = connect_error(&cfg).await;
        assert!(!is_permanent(&e), "{e:#}");
        assert!(format!("{e:#}").contains("confirm them"), "{e:#}");
        cloud.ask_terms.store(false, Ordering::SeqCst);

        // A refusal is tried again soon at first, in case this computer's
        // clock was just being set; only refusals in a row for 2 minutes
        // wait the hour. With the backoff's steps the tries come at 0, 5,
        // 15, 35, 75 and 155 s: the last one waits the hour.
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        for s in [0, 5, 15, 35, 75] {
            assert!(!waits_an_hour(t0, at(s)), "{s} s");
        }
        assert!(waits_an_hour(t0, at(120)) && waits_an_hour(t0, at(155)));
        cloud.refuse_login.store(true, Ordering::SeqCst);
        let l = Link {
            server: Server::new(MachineConfig::default()),
            status: Status::new("la_marzocco", "la_marzocco", TITLE),
        };
        tokio::spawn(run(l.clone(), cfg));
        wait_for("the error", || {
            l.status.snapshot().health == Health::Failing
        })
        .await;
        let error = l.status.snapshot().last_error.unwrap();
        assert!(error.contains("login refused"), "{error}");
        assert!(!error.contains("in an hour"), "{error}");
    }
}
