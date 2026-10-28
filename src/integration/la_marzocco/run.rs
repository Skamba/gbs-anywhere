//! The task: log in, find the machine, and after each knob press poll its
//! coffee log until the shot shows up.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::bail;

use super::TITLE;
use super::cloud::{BbwSelection, CloudClient, LastCoffee, Thing};
use super::config::Config;
use crate::integration::{Backoff, BoxFuture, Detail, Integration, Link, tenths};

const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// The integration. See the module docs.
pub struct LaMarzocco {
    pub cfg: Config,
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
pub fn new_coffee(coffees: &[LastCoffee], after_ms: i64) -> Option<&LastCoffee> {
    coffees
        .iter()
        .filter(|c| c.time > after_ms)
        .max_by_key(|c| c.time)
        .filter(|c| {
            c.valid
                && c.extraction_seconds.is_finite()
                && c.extraction_seconds > 0.0
                && c.extraction_seconds <= 3600.0
        })
}

async fn run(link: Link, cfg: Config) {
    let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
    loop {
        link.status
            .starting(Detail::keyed("la_marzocco.logging_in", "logging in"));
        match connect(&cfg).await {
            Ok((client, machine)) => {
                backoff.reset();
                link.status.subject(&machine.subject);
                link.status
                    .connected(idle_line(machine.last.as_ref(), now_ms()));
                if let Err(e) = watch(&link, &cfg, client, machine).await {
                    tracing::warn!("{TITLE}: {e:#}");
                    link.status.error(format!("{e:#}"));
                }
            }
            Err(e) => {
                // A wrong or missing serial: the card lists the machines in
                // full, the log only by their last four characters.
                match e.downcast_ref::<MachineChoice>() {
                    Some(choice) => tracing::warn!("{TITLE}: {}", choice.log),
                    None => tracing::warn!("{TITLE}: {e:#}"),
                }
                link.status.error(format!("{e:#}"));
            }
        }
        tracing::info!("{TITLE}: retrying in {} s", backoff.peek().as_secs());
        backoff.wait().await;
    }
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

/// `last coffee 3 min ago: 25.1 s, 38.0 g`, with the age as of `now_ms`.
fn idle_line(last: Option<&Idle>, now_ms: i64) -> Detail {
    let Some(c) = last else {
        return Detail::keyed("la_marzocco.no_coffee", "no coffee in the last 7 days");
    };
    let age_ms = (now_ms - c.time_ms).max(0);
    let seconds = tenths(c.extraction_seconds);
    let time = format!("last coffee {} ago: {seconds:.1} s", age_of(age_ms));
    let line = match c.grams.map(tenths) {
        Some(g) => {
            Detail::keyed("la_marzocco.last_coffee", format!("{time}, {g:.1} g")).var("weight_g", g)
        }
        None => Detail::keyed("la_marzocco.last_coffee_no_weight", time),
    };
    line.var("age_ms", age_ms)
        .var("time_ms", (seconds * 1000.0).round() as i64)
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
            [] => bail!("no coffee machine on this account"),
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
    let last = coffees.iter().max_by_key(|c| c.time).map(Idle::from);
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

const RECIPE_WINDOW: Duration = Duration::from_secs(15);

fn measured_grams(coffee: &LastCoffee) -> Option<f64> {
    if coffee.dose_mode.as_deref() != Some("MassType") {
        return None;
    }
    let grams = coffee.dose_value?;
    let rounded = (grams * 10.0).round() / 10.0;
    (grams.is_finite() && grams > 0.0 && grams <= 1000.0 && rounded > 0.0).then_some(rounded)
}

fn manual_target() -> Detail {
    Detail::keyed(
        "la_marzocco.recipe_manual",
        "target not changed · select and check it on the machine before brewing",
    )
}

async fn follow_recipe(
    link: &Link,
    cfg: &Config,
    client: &mut CloudClient,
    serial: &str,
    started: Instant,
) -> Detail {
    if link
        .server
        .integrations()
        .list()
        .iter()
        .filter(|i| i.status.kind == "la_marzocco")
        .count()
        != 1
    {
        tracing::warn!("{TITLE}: automatic targets require exactly one La Marzocco integration");
        return manual_target();
    }
    let Some(password) = cfg.grinder_owner_password.as_deref() else {
        return manual_target();
    };
    link.status.watching(Detail::keyed(
        "la_marzocco.recipe_reading",
        "reading the recipe target · wait before brewing",
    ));
    let job = loop {
        if !link.waiting_for(started) || started.elapsed() >= RECIPE_WINDOW {
            return manual_target();
        }
        if let Some(job) = link.server.claim_recipe_job(started) {
            break job;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let current = || started.elapsed() < RECIPE_WINDOW && link.server.recipe_job_current(&job);
    if !current() {
        return manual_target();
    }
    let remaining = RECIPE_WINDOW.saturating_sub(started.elapsed());
    let target = tokio::time::timeout(
        remaining,
        super::recipe_log::target_weight(&job.grinder, password, &job.grind_uuid),
    )
    .await;
    let grams = match target {
        Ok(Ok(grams)) => grams,
        Ok(Err(error)) => {
            tracing::warn!("{TITLE}: recipe lookup failed: {error}");
            return manual_target();
        }
        Err(_) => {
            tracing::warn!("{TITLE}: recipe lookup deadline expired");
            return manual_target();
        }
    };
    // The claimed job is never retried, even if the command's outcome is unknown.
    match client
        .select_bbw_target(serial, f64::from(grams), current)
        .await
    {
        BbwSelection::Confirmed | BbwSelection::AlreadySet => Detail::keyed(
            "la_marzocco.recipe_ready",
            format!("target {grams} g confirmed · check the machine before brewing"),
        )
        .var("weight_g", grams),
        BbwSelection::Unknown => Detail::keyed(
            "la_marzocco.recipe_unknown",
            "target change unconfirmed · check the machine; no automatic retry",
        ),
        BbwSelection::NotApplied(reason) => {
            tracing::warn!("{TITLE}: recipe target not changed: {reason}");
            manual_target()
        }
    }
}

/// Waits for new grinder requests; every request establishes its own cloud baseline.
async fn watch(
    link: &Link,
    cfg: &Config,
    mut client: CloudClient,
    machine: Machine,
) -> anyhow::Result<()> {
    let mut brews = link.grinder_brews();
    let mut newest_ms = machine.last.map_or(i64::MIN, |c| c.time_ms);
    let mut idle = machine.last;
    let mut refresh = tokio::time::interval(IDLE_REFRESH);
    refresh.tick().await;
    loop {
        let started = tokio::select! {
            pressed = brews.next_started() => {
                let Some(started) = pressed else { return Ok(()) };
                started
            }
            _ = refresh.tick() => {
                link.status.connected(idle_line(idle.as_ref(), now_ms()));
                continue;
            }
        };
        if !link.waiting_for(started) {
            continue;
        }
        if cfg.recipe_follow {
            link.status.watching(Detail::keyed(
                "la_marzocco.recipe_reading",
                "reading the recipe target · wait before brewing",
            ));
        }
        // A failed first read cannot make an older cloud coffee eligible later.
        match client.last_coffees(&machine.serial, 1).await {
            Ok(list) => {
                if let Some(c) = list.iter().max_by_key(|c| c.time) {
                    newest_ms = newest_ms.max(c.time);
                }
            }
            Err(e) => {
                tracing::warn!("{TITLE}: baseline failed; skipping this grind: {e:#}");
                link.status.watching(Detail::keyed(
                    "la_marzocco.baseline_failed",
                    "this grind has no reliable coffee baseline · target changes and feedback skipped",
                ));
                continue;
            }
        }
        if !link.waiting_for(started) {
            continue;
        }
        let poll_s = cfg.poll.as_secs();
        let waiting = if cfg.recipe_follow {
            follow_recipe(link, cfg, &mut client, &machine.serial, started).await
        } else {
            Detail::keyed(
                "la_marzocco.watching",
                format!("grinder is waiting · checking the coffee log every {poll_s} s"),
            )
            .var("every_s", poll_s)
        };
        link.status.watching(waiting.clone());
        let mut errors = 0u32;
        loop {
            tokio::time::sleep(cfg.poll).await;
            if !link.waiting_for(started) {
                break;
            }
            let list = match client.last_coffees(&machine.serial, 1).await {
                Ok(l) => {
                    if errors > 0 {
                        errors = 0;
                        link.status.watching(waiting.clone());
                    }
                    l
                }
                Err(e) => {
                    errors += 1;
                    tracing::warn!("{TITLE}: {e:#}");
                    link.status.error(format!("{e:#}"));
                    if errors >= 10 {
                        bail!("giving up on this session after {errors} errors");
                    }
                    continue;
                }
            };
            if !link.waiting_for(started) {
                break;
            }
            let coffee = new_coffee(&list, newest_ms);
            if let Some(c) = list.iter().max_by_key(|c| c.time) {
                newest_ms = newest_ms.max(c.time);
            }
            let Some(c) = coffee else { continue };
            if cfg.recipe_follow && measured_grams(c).is_none() {
                link.status.watching(Detail::keyed(
                    "la_marzocco.recipe_no_weight",
                    "coffee has no measured scale weight · automatic feedback skipped",
                ));
                continue;
            }
            let time = Duration::from_secs_f64(c.extraction_seconds);
            let weight = c.dose_value.map(|g| {
                (
                    (g * 10.0).round() / 10.0,
                    "weighed by the machine".to_owned(),
                )
            });
            link.report_for(started, time, weight);
            idle = Some(Idle::from(c));
            break;
        }
        link.status.connected(idle_line(idle.as_ref(), now_ms()));
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod watch_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::Status;
    use crate::integration::la_marzocco::cloud::parse_last_coffees;
    use serde_json::json;

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
    fn automatic_feedback_requires_a_positive_weight_after_rounding() {
        let mut coffee: LastCoffee = serde_json::from_value(json!({
            "time": 100, "extractionSeconds": 28, "doseValue": 30.2, "doseMode": "MassType"
        }))
        .unwrap();
        assert_eq!(measured_grams(&coffee), Some(30.2));
        for grams in [0.0, 0.01, f64::NAN, f64::INFINITY, -1.0, 1001.0] {
            coffee.dose_value = Some(grams);
            assert_eq!(measured_grams(&coffee), None, "{grams}");
        }
        coffee.dose_value = Some(30.2);
        coffee.dose_mode = Some("PulsesType".into());
        assert_eq!(measured_grams(&coffee), None);
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
            serde_json::Value::Object(snap.detail_vars),
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
            serde_json::Value::Object(snap.detail_vars),
            json!({"age_ms": 90_000, "time_ms": 28_500})
        );

        let snap = shown(idle_line(None, now));
        assert_eq!(snap.detail, "no coffee in the last 7 days");
        assert_eq!(snap.detail_key, Some("la_marzocco.no_coffee"));
        assert!(snap.detail_vars.is_empty());
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
}
