//! The task: log in, find the machine, and after each knob press poll its
//! coffee log until the shot shows up.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};

use super::TITLE;
use super::cloud::{CloudClient, LastCoffee, Thing};
use super::config::Config;
use crate::integration::{Backoff, BoxFuture, Integration, Link};

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
        .filter(|c| c.valid)
}

async fn run(link: Link, cfg: Config) {
    let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
    loop {
        link.status.starting("logging in");
        match connect(&cfg).await {
            Ok((client, machine)) => {
                backoff.reset();
                link.status.subject(&machine.subject);
                link.status.connected(idle_line(machine.last.as_ref()));
                if let Err(e) = watch(&link, &cfg, client, machine).await {
                    tracing::warn!("{TITLE}: {e:#}");
                    link.status.error(format!("{e:#}"));
                }
            }
            Err(e) => {
                tracing::warn!("{TITLE}: {e:#}");
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

/// `last coffee 3 min ago: 25.1 s, 38.0 g`, with the age as of now.
fn idle_line(last: Option<&Idle>) -> String {
    match last {
        Some(c) => format!(
            "last coffee {} ago: {:.1} s{}",
            age(c.time_ms),
            c.extraction_seconds,
            c.grams.map_or(String::new(), |g| format!(", {g:.1} g"))
        ),
        None => "no coffee in the last 7 days".to_owned(),
    }
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
                .with_context(|| {
                    format!(
                        "machine {sn} not on this account (found: {})",
                        list_serials(&machines)
                    )
                })?
        }
        None => match machines.as_slice() {
            [one] => one,
            [] => bail!("no coffee machine on this account"),
            many => bail!(
                "more than one machine on this account, set the serial number: {}",
                list_serials(many)
            ),
        },
    };
    let serial = machine.serial_number.clone();
    let coffees = client.last_coffees(&serial, 7).await?;
    let last = coffees.first().map(Idle::from);
    let subject = format!("{serial} · {}", machine.model_name);
    tracing::info!(
        "{TITLE}: connected to {subject}, {}",
        idle_line(last.as_ref())
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

fn list_serials(machines: &[&Thing]) -> String {
    machines
        .iter()
        .map(|t| format!("{} ({})", t.serial_number, t.model_name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn age(time_ms: i64) -> String {
    age_of(now_ms() - time_ms)
}

/// `45 s`, `12 min`, `5 h`, `2 days`.
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

/// Waits for knob presses and polls the coffee log during each brew.
async fn watch(
    link: &Link,
    cfg: &Config,
    mut client: CloudClient,
    machine: Machine,
) -> anyhow::Result<()> {
    let mut brews = link.grinder_brews();
    // Newest coffee we know of; only newer ones count as the shot.
    let mut newest_ms = now_ms();
    let mut idle = machine.last;
    let mut refresh = tokio::time::interval(IDLE_REFRESH);
    refresh.tick().await; // the first tick fires at once
    loop {
        tokio::select! {
            pressed = brews.next() => {
                if !pressed {
                    return Ok(());
                }
            }
            _ = refresh.tick() => {
                link.status.connected(idle_line(idle.as_ref()));
                continue;
            }
        }
        // Baseline: whatever the log holds at the knob press is not the shot.
        match client.last_coffees(&machine.serial, 1).await {
            Ok(list) => {
                if let Some(c) = list.first() {
                    newest_ms = newest_ms.max(c.time);
                }
            }
            Err(e) => tracing::warn!("{TITLE}: {e:#}"),
        }
        let poll_s = cfg.poll.as_secs();
        tracing::info!("{TITLE}: grinder is waiting, watching the coffee log every {poll_s} s");
        let waiting = format!("grinder is waiting · checking the coffee log every {poll_s} s");
        link.status.watching(&waiting);
        let mut errors = 0u32;
        loop {
            tokio::time::sleep(cfg.poll).await;
            if !link.grinder_waiting() {
                tracing::info!("{TITLE}: brew ended without a coffee from the log");
                break;
            }
            let list = match client.last_coffees(&machine.serial, 1).await {
                Ok(l) => {
                    if errors > 0 {
                        // Back from a failed poll: still watching this brew.
                        errors = 0;
                        link.status.watching(&waiting);
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
            let Some(c) = new_coffee(&list, newest_ms) else {
                if let Some(c) = list.iter().find(|c| c.time > newest_ms) {
                    tracing::info!(
                        "{TITLE}: ignoring coffee marked invalid ({}): {:.1} s",
                        c.invalid_reason.as_deref().unwrap_or("no reason"),
                        c.extraction_seconds
                    );
                    newest_ms = c.time;
                }
                continue;
            };
            newest_ms = c.time;
            let time =
                Duration::from_millis((c.extraction_seconds.max(0.0) * 1000.0).round() as u64);
            // The cloud sends float32 noise like 37.4000015; the scale reads 0.1 g.
            let weight = c.dose_value.map(|g| {
                (
                    (g * 10.0).round() / 10.0,
                    "weighed by the machine".to_owned(),
                )
            });
            link.report(time, weight);
            idle = Some(Idle::from(c));
            break;
        }
        link.status.connected(idle_line(idle.as_ref()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age_of(45_000), "45 s");
        assert_eq!(age_of(76 * 60_000), "76 min");
        assert_eq!(age_of(5 * 3_600_000), "5 h");
        assert_eq!(age_of(47 * 3_600_000), "47 h");
        assert_eq!(age_of(3 * 86_400_000 + 5), "3 days");
        assert_eq!(age_of(-5), "0 s");
        let line = idle_line(Some(&Idle {
            time_ms: now_ms() - 90_000,
            extraction_seconds: 25.1,
            grams: Some(38.0),
        }));
        assert_eq!(line, "last coffee 90 s ago: 25.1 s, 38.0 g");
        assert_eq!(idle_line(None), "no coffee in the last 7 days");
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
        let mut list: Vec<LastCoffee> =
            serde_json::from_value(raw["output"]["lastCoffees"].clone()).unwrap();
        list.sort_by_key(|c| std::cmp::Reverse(c.time));
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
}
