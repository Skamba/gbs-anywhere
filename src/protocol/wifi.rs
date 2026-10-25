//! The link to a grinder of the E65/E80 Grind-by-Sync family over its
//! Local WiFi option. Unlike the E64 WS, which polls the machine, here the
//! machine is the client:
//!
//! ```text
//! machine                                   grinder (HTTPS, port 30001)
//!   pairingApplication {name, model, ...}   -> applicationId, pairingCode
//!   pairingStatus {applicationId}           -> accepted (once the person
//!                                              accepts it on the grinder)
//!   brewJobs {pairingId}  (every few s)     -> one job per GbS grind
//!   brewAccept {grindUuid, ...}             -> the machine takes the job
//!   brewDone {grindUuid, duration, mass}    -> the grinder adjusts
//! ```
//!
//! Every request is a `POST` with a JSON body that carries
//! [`API_VERSION`]; every reply has a `status` (`"Ok"` or an error name).
//! Plain types only, no I/O: [`crate::server::wifi_grinder`] does the talking.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The API version the grinder speaks.
pub const API_VERSION: u32 = 1;

/// The grinder's port for this API.
pub const DEFAULT_PORT: u16 = 30001;

pub const PATH_PAIRING_APPLICATION: &str = "/grinderApi/pairingApplication";
pub const PATH_PAIRING_STATUS: &str = "/grinderApi/pairingStatus";
pub const PATH_BREW_JOBS: &str = "/grinderApi/brewJobs";
pub const PATH_BREW_ACCEPT: &str = "/grinderApi/brewAccept";
pub const PATH_BREW_DONE: &str = "/grinderApi/brewDone";

/// `status` of a successful reply.
pub const STATUS_OK: &str = "Ok";
/// `pairingStatus`: the application expired or the grinder restarted.
pub const STATUS_APPLICATION_UNKNOWN: &str = "ClientErrorPairingAppIdNotFound";
/// `pairingApplication`: this device id is paired already.
pub const STATUS_ALREADY_PAIRED: &str = "ClientErrorDevAlreadyPaired";
/// `brewAccept`: the job is gone (taken, timed out, or never existed).
pub const STATUS_JOB_UNKNOWN: &str = "ClientErrorBrewJobNotExisting";
/// `brewDone`: no accepted job with that `grindUuid` (it timed out).
pub const STATUS_ONGOING_JOB_UNKNOWN: &str = "ClientErrorOngoingJobNotExisting";

/// Asks the grinder to pair. It lists the device under the available devices
/// while someone has pairing started on the grinder.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingApplication {
    pub version: u32,
    /// Shown on the grinder.
    pub name: String,
    pub model: String,
    pub manufacturer: String,
    /// Identifies this machine to the grinder; keep it across restarts.
    pub brew_device_id: String,
    pub num_of_group_heads: u32,
    /// Whether the machine sends `brewProgress` during a shot.
    pub progress_supported: bool,
}

impl PairingApplication {
    /// gbs-anywhere as a one-group machine without live progress.
    pub fn new(brew_device_id: &str) -> Self {
        Self {
            version: API_VERSION,
            name: "gbs-anywhere".to_owned(),
            model: "gbs-anywhere".to_owned(),
            manufacturer: "gbs-anywhere".to_owned(),
            brew_device_id: brew_device_id.to_owned(),
            num_of_group_heads: 1,
            progress_supported: false,
        }
    }
}

/// Reply to [`PATH_PAIRING_APPLICATION`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingApplicationRes {
    pub status: String,
    pub application_id: Option<u32>,
    /// The number the grinder shows next to the device, to tell it apart.
    pub pairing_code: Option<u32>,
}

/// Reply to [`PATH_PAIRING_STATUS`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStatusRes {
    pub status: String,
    #[serde(default)]
    pub accepted: bool,
    /// Sent once accepted. Also shown on the grinder under Configuration,
    /// once paired.
    pub pairing_id: Option<u32>,
}

/// One job per Grind-by-Sync grind: the grinder wants this shot's numbers.
/// Numbers are read as floats, so `28000.0` works as well as `28000`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrewJob {
    pub grind_uuid: String,
    /// Counted from 0 (group 1 on the grinder's screen is 0); absent for a
    /// one-group machine.
    pub group_head_id: Option<u32>,
    /// The dose (single, double, ...) as a number; echoed back in
    /// [`BrewAccept`].
    pub dose_type_expected: Option<Value>,
    /// The recipe's target time.
    pub duration_expected_ms: Option<f64>,
    /// The recipe's target weight in the cup; 0 when the recipe has none.
    pub brew_weight_expected_mg: Option<f64>,
}

impl BrewJob {
    /// The recipe's target weight in grams, if any.
    pub fn target_grams(&self) -> Option<f64> {
        let mg = self.brew_weight_expected_mg?;
        (mg.is_finite() && mg > 0.0).then(|| mg / 1000.0)
    }

    /// The recipe's target time in milliseconds, if any (at least 1 ms).
    pub fn target_ms(&self) -> Option<u32> {
        let ms = self.duration_expected_ms?;
        (ms.is_finite() && ms >= 1.0).then(|| ms.min(f64::from(u32::MAX)) as u32)
    }
}

/// Reply to [`PATH_BREW_JOBS`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrewJobsRes {
    pub status: String,
    #[serde(default)]
    pub brew_jobs: Vec<BrewJob>,
}

/// The machine takes a job: the shot is on its way.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrewAccept {
    pub version: u32,
    pub pairing_id: u32,
    pub grind_uuid: String,
    pub group_head_id: u32,
    pub dose_type: Option<Value>,
}

impl BrewAccept {
    /// Takes the job for its group, or the first group (0) when it names
    /// none. Grind-by-Sync only learns from shots of the group selected on the
    /// grinder ("Grouphead Selection"), shown there counted from 1.
    pub fn new(pairing_id: u32, job: &BrewJob) -> Self {
        Self {
            version: API_VERSION,
            pairing_id,
            grind_uuid: job.grind_uuid.clone(),
            group_head_id: job.group_head_id.unwrap_or(0),
            dose_type: job.dose_type_expected.clone(),
        }
    }
}

/// How a shot ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BrewResult {
    Success,
    /// The grinder skips it.
    Canceled,
}

/// The finished shot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrewDone {
    pub version: u32,
    pub pairing_id: u32,
    pub grind_uuid: String,
    pub duration_final_ms: u32,
    pub mass_final_mg: u64,
    /// New for every shot.
    pub brew_event_uuid: String,
    pub result: BrewResult,
}

impl BrewDone {
    /// Most a shot can weigh when sent: a typo, not coffee, beyond this.
    pub const MAX_MASS_MG: u64 = 5_000_000;

    /// The answer to `job`: `time_ms` and `grams` as measured (`grams` is
    /// kept between 0 and [`Self::MAX_MASS_MG`]).
    pub fn new(
        pairing_id: u32,
        job: &BrewJob,
        result: BrewResult,
        time_ms: u32,
        grams: f64,
        brew_event_uuid: String,
    ) -> Self {
        let mg = if grams.is_finite() && grams > 0.0 {
            ((grams * 1000.0).round() as u64).min(Self::MAX_MASS_MG)
        } else {
            0
        };
        Self {
            version: API_VERSION,
            pairing_id,
            grind_uuid: job.grind_uuid.clone(),
            duration_final_ms: time_ms,
            mass_final_mg: mg,
            brew_event_uuid,
            result,
        }
    }
}

/// Any request that only needs the pairing ([`PATH_BREW_JOBS`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paired {
    pub version: u32,
    pub pairing_id: u32,
}

impl Paired {
    pub fn new(pairing_id: u32) -> Self {
        Self {
            version: API_VERSION,
            pairing_id,
        }
    }
}

/// A reply body as text. The grinder labels its replies UTF-8 but sends
/// some text (its own name: "Mahlkönig") as Latin-1, so bytes that are not
/// valid UTF-8 are read as Latin-1 instead of failing.
pub fn decode(body: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(body) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(body.iter().map(|&b| char::from(b)).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_use_the_grinders_field_names() {
        let app = PairingApplication::new("dev-1");
        assert_eq!(
            serde_json::to_value(&app).unwrap(),
            json!({
                "version": 1, "name": "gbs-anywhere", "model": "gbs-anywhere",
                "manufacturer": "gbs-anywhere", "brewDeviceId": "dev-1",
                "numOfGroupHeads": 1, "progressSupported": false
            })
        );

        let job: BrewJob = serde_json::from_value(json!({
            "grindUuid": "g-1", "doseTypeExpected": 2,
            "durationExpectedMs": 28000, "brewWeightExpectedMg": 0
        }))
        .unwrap();
        assert_eq!(job.target_grams(), None, "0 mg means no target");
        assert_eq!(job.target_ms(), Some(28_000));
        assert_eq!(
            serde_json::to_value(BrewAccept::new(7, &job)).unwrap(),
            json!({
                "version": 1, "pairingId": 7, "grindUuid": "g-1",
                "groupHeadId": 0, "doseType": 2
            })
        );
        let with_weight: BrewJob = serde_json::from_value(json!({
            "grindUuid": "g-1", "brewWeightExpectedMg": 40000.0
        }))
        .unwrap();
        assert_eq!(with_weight.target_grams(), Some(40.0), "floats are fine");
        assert_eq!(with_weight.target_ms(), None, "no target time");
        for none in [0.0, 0.5, -1.0] {
            let job: BrewJob =
                serde_json::from_value(json!({ "grindUuid": "g-1", "durationExpectedMs": none }))
                    .unwrap();
            assert_eq!(job.target_ms(), None, "{none}");
        }

        let done = BrewDone::new(7, &job, BrewResult::Success, 27_600, 39.9, "e-1".into());
        assert_eq!(
            serde_json::to_value(done).unwrap(),
            json!({
                "version": 1, "pairingId": 7, "grindUuid": "g-1",
                "durationFinalMs": 27600, "massFinalMg": 39900,
                "brewEventUuid": "e-1", "result": "Success"
            })
        );
        let mass =
            |g: f64| BrewDone::new(7, &job, BrewResult::Success, 0, g, String::new()).mass_final_mg;
        assert_eq!(mass(-1.0), 0);
        assert_eq!(mass(f64::NAN), 0);
        assert_eq!(mass(1e300), BrewDone::MAX_MASS_MG, "no wrap-around");
    }

    #[test]
    fn replies_parse_with_missing_fields() {
        let res: BrewJobsRes =
            serde_json::from_str(r#"{"brewJobs": [], "msgType": "brewJobsRes", "status": "Ok"}"#)
                .unwrap();
        assert_eq!(res.status, STATUS_OK);
        assert!(res.brew_jobs.is_empty());

        let res: PairingStatusRes = serde_json::from_str(
            r#"{"msgType": "pairingStatusRes", "status": "ClientErrorPairingAppIdNotFound"}"#,
        )
        .unwrap();
        assert_eq!(res.status, STATUS_APPLICATION_UNKNOWN);
        assert!(!res.accepted);

        let res: PairingApplicationRes = serde_json::from_str(
            r#"{"applicationId": 5, "msgType": "pairingApplicationRes", "pairingCode": 73, "status": "Ok"}"#,
        )
        .unwrap();
        assert_eq!((res.application_id, res.pairing_code), (Some(5), Some(73)));
    }

    #[test]
    fn latin1_text_is_read_not_rejected() {
        assert_eq!(decode("Mahlkönig".as_bytes()), "Mahlkönig");
        assert_eq!(decode(b"Mahlk\xf6nig"), "Mahlkönig");
    }
}
