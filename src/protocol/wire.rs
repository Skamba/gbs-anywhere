//! JSON bodies of the mako link, with the exact keys the grinder uses.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    HEATED_TOLERANCE_C, PATH_BREWRATIO, PATH_MACHINE, PATH_MAKO, PATH_SCRIPTS_EXECUTE,
    TANK_LEVEL_FULL,
};

/// `MA_STATUS`, the machine state.
/// Serialized as its integer code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "i64", try_from = "i64")]
pub enum MachineStatus {
    Off = 0,
    On = 1,
    Eco = 2,
    /// A brew is running (the grinder shows "Xenia is brewing").
    ActiveServing = 3,
    /// The brew has ended and the machine is finishing up; the grinder takes
    /// the shot result on entering this state.
    ActiveFinishing = 4,
    Testing = 5,
    Fault = 6,
}

impl MachineStatus {
    pub const ALL: [MachineStatus; 7] = [
        Self::Off,
        Self::On,
        Self::Eco,
        Self::ActiveServing,
        Self::ActiveFinishing,
        Self::Testing,
        Self::Fault,
    ];

    pub fn code(self) -> i64 {
        self as i64
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.code() == code)
    }

    /// The grinder's own name for the state.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::On => "ON",
            Self::Eco => "ECO",
            Self::ActiveServing => "ACTIVE SERVING",
            Self::ActiveFinishing => "ACTIVE FINISHING",
            Self::Testing => "TESTING",
            Self::Fault => "FAULT",
        }
    }
}

impl From<MachineStatus> for i64 {
    fn from(s: MachineStatus) -> i64 {
        s.code()
    }
}

impl TryFrom<i64> for MachineStatus {
    type Error = String;
    fn try_from(code: i64) -> Result<Self, String> {
        Self::from_code(code).ok_or_else(|| format!("unknown MA_STATUS {code}"))
    }
}

/// Reply to `GET /api/v2/mako`, with the keys the grinder reads.
///
/// Deserializing is lenient (missing keys take the default) so the same type
/// can parse a partial reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MakoState {
    /// Measured shot time. The value the dial-in algorithm consumes.
    #[serde(rename = "REAL_EXTRACTION_TIME_MS")]
    pub real_extraction_time_ms: u32,
    /// Pump flow-meter volume, reported to the grinder as the brew volume.
    #[serde(rename = "PU_SENS_FLOW_METER_VOLUME")]
    pub flow_meter_volume_ml: f64,
    #[serde(rename = "BG_SET_TEMP")]
    pub bg_set_temp: f64,
    #[serde(rename = "BG_SENS_TEMP")]
    pub bg_sens_temp: f64,
    /// `1` means full; anything else blocks grinding ("tank is empty").
    #[serde(rename = "TANK_LEVEL")]
    pub tank_level: i64,
    /// Name of the running machine script. Stored by the grinder, not acted on.
    #[serde(rename = "script")]
    pub script: String,
    /// Machine timestamp. Stored by the grinder, not acted on; format unknown.
    #[serde(rename = "TIMESTAMP")]
    pub timestamp: String,
    /// Extraction phase. Only `2` (user abort) is special to the grinder.
    #[serde(rename = "MA_EXTRACTION_STATUS")]
    pub extraction_status: i64,
    #[serde(rename = "MA_STATUS")]
    pub status: MachineStatus,
}

impl Default for MakoState {
    /// A switched-on, heated machine with a full tank: grinding allowed.
    fn default() -> Self {
        Self {
            real_extraction_time_ms: 0,
            flow_meter_volume_ml: 0.0,
            bg_set_temp: 93.0,
            bg_sens_temp: 93.0,
            tank_level: TANK_LEVEL_FULL,
            script: String::new(),
            timestamp: String::new(),
            extraction_status: 0,
            status: MachineStatus::On,
        }
    }
}

impl MakoState {
    /// Heated enough for the grinder to allow grinding.
    pub fn is_ready_heated(&self) -> bool {
        self.bg_sens_temp > self.bg_set_temp - HEATED_TOLERANCE_C
    }

    /// Tank full, as the grinder sees it.
    pub fn is_tank_full(&self) -> bool {
        self.tank_level == TANK_LEVEL_FULL
    }

    /// Why the grinder would refuse to grind with this reply, or `None` if it
    /// allows it (for a connected machine).
    pub fn grind_blocked(&self) -> Option<GrindBlocked> {
        use MachineStatus::*;
        match self.status {
            On | ActiveFinishing if !self.is_ready_heated() => Some(GrindBlocked::WarmingUp),
            On | ActiveFinishing if !self.is_tank_full() => Some(GrindBlocked::TankEmpty),
            On | ActiveFinishing => None,
            Off => Some(GrindBlocked::Standby),
            Eco => Some(GrindBlocked::Eco),
            ActiveServing => Some(GrindBlocked::Brewing),
            Testing => Some(GrindBlocked::Testing),
            Fault => Some(GrindBlocked::Fault),
        }
    }
}

/// Reasons the grinder blocks grinding, with its on-screen text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrindBlocked {
    NotConnected,
    Standby,
    Eco,
    Brewing,
    Testing,
    Fault,
    WarmingUp,
    TankEmpty,
}

impl GrindBlocked {
    pub fn grinder_message(self) -> &'static str {
        match self {
            Self::NotConnected => "Xenia is off. Please turn it on.",
            Self::Standby => "Xenia is in standby. Please wake it up.",
            Self::Eco => "Xenia is in ECO state. Please wake it up.",
            Self::Brewing => "Xenia is brewing. Please wait until it's finished.",
            Self::Testing => "Xenia is in testing state. Please wait until it's finished.",
            Self::Fault => "Xenia has an error. Please check the machine.",
            Self::WarmingUp => {
                "Xenia is warming up. Please wait for the cup symbol to stop blinking."
            }
            Self::TankEmpty => "Xenia tank is empty. Please refill it.",
        }
    }
}

/// Reply to `GET /api/v2/machine`. The grinder reads `MA_TYPE`, `MA_SN`,
/// `FW_VERSION_*` and `ESP_FW_*`; the rest makes the reply look like a real
/// Xenia's. Defaults are those of a current Xenia (MA 4.226 / ESP 3.87).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MachineIdentity {
    #[serde(rename = "MA_TYPE")]
    pub machine_type: i64,
    #[serde(rename = "MA_SUBTYPE")]
    pub machine_subtype: i64,
    #[serde(rename = "MA_SN")]
    pub serial: String,
    #[serde(rename = "FW_VERSION_MAJOR")]
    pub fw_major: i64,
    #[serde(rename = "FW_VERSION_MINOR")]
    pub fw_minor: i64,
    #[serde(rename = "ESP_FW_MAJOR")]
    pub esp_fw_major: i64,
    #[serde(rename = "ESP_FW_MINOR")]
    pub esp_fw_minor: i64,
    #[serde(rename = "MA_MAX_AMPERE")]
    pub max_ampere: i64,
    #[serde(rename = "MA_FIX_WATER_SUPPLY")]
    pub fix_water_supply: i64,
    #[serde(rename = "BLE")]
    pub ble: i64,
    #[serde(rename = "MA_DELAY")]
    pub delay: i64,
}

impl Default for MachineIdentity {
    fn default() -> Self {
        Self {
            machine_type: 1,
            machine_subtype: 0,
            serial: "0000000000".into(),
            fw_major: 4,
            fw_minor: 226,
            esp_fw_major: 3,
            esp_fw_minor: 87,
            max_ampere: 13,
            fix_water_supply: 0,
            ble: 0,
            delay: -1,
        }
    }
}

/// Body of `POST /api/v2/brewratio`, sent by the grinder after a grind with a
/// Grind-by-Sync recipe.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GrindResult {
    /// Target beverage weight of the recipe, grams.
    pub beverage_weight_g: Option<f64>,
    /// Filter (basket) id, as sent. Type on the wire not yet observed, so both
    /// numbers and strings are accepted and kept as text.
    pub filter: Option<String>,
    /// The whole body, for anything not modelled yet.
    pub raw: Value,
}

impl GrindResult {
    pub fn parse(body: &[u8]) -> Result<Self, serde_json::Error> {
        let raw: Value = serde_json::from_slice(body)?;
        Ok(Self {
            beverage_weight_g: raw.get("SYNC_BEVERAGE_WEIGHT").and_then(number_like),
            filter: raw.get("SYNC_FILTER").and_then(text_like),
            raw,
        })
    }
}

/// Body of `POST /api/v2/scripts/execute`: `{"ID": 9}` to start a brew.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StartBrew {
    pub script_id: Option<i64>,
    pub raw: Value,
}

impl StartBrew {
    pub fn parse(body: &[u8]) -> Result<Self, serde_json::Error> {
        let raw: Value = serde_json::from_slice(body)?;
        Ok(Self {
            script_id: raw.get("ID").and_then(number_like).map(|n| n as i64),
            raw,
        })
    }
}

/// A request from the grinder, classified by method and path.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrinderRequest {
    /// `GET /api/v2/machine`.
    Identity,
    /// `GET /api/v2/mako`.
    PollMako,
    /// `POST /api/v2/brewratio`.
    GrindResult(GrindResult),
    /// `POST /api/v2/scripts/execute`.
    StartBrew(StartBrew),
    /// A known write whose body did not parse as JSON.
    Malformed { path: String, error: String },
    /// Anything else (logged, answered generically).
    Other { method: String, path: String },
}

impl GrinderRequest {
    pub fn classify(method: &str, path: &str, body: &[u8]) -> Self {
        let p = path.trim_end_matches('/');
        let malformed = |e: serde_json::Error| Self::Malformed {
            path: p.to_owned(),
            error: e.to_string(),
        };
        match (method.to_ascii_uppercase().as_str(), p) {
            ("GET", PATH_MACHINE) => Self::Identity,
            ("GET", PATH_MAKO) => Self::PollMako,
            ("POST", PATH_BREWRATIO) => GrindResult::parse(body)
                .map(Self::GrindResult)
                .unwrap_or_else(malformed),
            ("POST", PATH_SCRIPTS_EXECUTE) => StartBrew::parse(body)
                .map(Self::StartBrew)
                .unwrap_or_else(malformed),
            (m, _) => Self::Other {
                method: m.to_owned(),
                path: p.to_owned(),
            },
        }
    }
}

fn number_like(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text_like(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mako_serializes_with_grinder_keys() {
        let v = serde_json::to_value(MakoState::default()).unwrap();
        for key in [
            "REAL_EXTRACTION_TIME_MS",
            "PU_SENS_FLOW_METER_VOLUME",
            "BG_SET_TEMP",
            "BG_SENS_TEMP",
            "TANK_LEVEL",
            "script",
            "TIMESTAMP",
            "MA_EXTRACTION_STATUS",
            "MA_STATUS",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert_eq!(v["MA_STATUS"], 1);
    }

    #[test]
    fn mako_parses_partial_reply() {
        let s: MakoState = serde_json::from_str(r#"{"MA_STATUS":3,"TANK_LEVEL":0}"#).unwrap();
        assert_eq!(s.status, MachineStatus::ActiveServing);
        assert!(!s.is_tank_full());
    }

    #[test]
    fn grind_gate_matches_update_grind_allowed() {
        let ok = MakoState::default();
        assert_eq!(ok.grind_blocked(), None);
        let finishing = MakoState {
            status: MachineStatus::ActiveFinishing,
            ..ok.clone()
        };
        assert_eq!(finishing.grind_blocked(), None);
        let cold = MakoState {
            bg_sens_temp: 90.9,
            ..ok.clone()
        };
        assert_eq!(cold.grind_blocked(), Some(GrindBlocked::WarmingUp));
        let almost = MakoState {
            bg_sens_temp: 91.1,
            ..ok.clone()
        };
        assert_eq!(almost.grind_blocked(), None);
        let empty = MakoState {
            tank_level: 2,
            ..ok.clone()
        };
        assert_eq!(empty.grind_blocked(), Some(GrindBlocked::TankEmpty));
        let brewing = MakoState {
            status: MachineStatus::ActiveServing,
            ..ok
        };
        assert_eq!(brewing.grind_blocked(), Some(GrindBlocked::Brewing));
    }

    #[test]
    fn classify_grinder_writes() {
        let r = GrinderRequest::classify(
            "POST",
            "/api/v2/brewratio",
            br#"{"SYNC_BEVERAGE_WEIGHT":36.5,"SYNC_FILTER":"2"}"#,
        );
        match r {
            GrinderRequest::GrindResult(g) => {
                assert_eq!(g.beverage_weight_g, Some(36.5));
                assert_eq!(g.filter.as_deref(), Some("2"));
            }
            other => panic!("{other:?}"),
        }
        let r = GrinderRequest::classify("POST", "/api/v2/scripts/execute", br#"{"ID":9}"#);
        assert!(matches!(
            r,
            GrinderRequest::StartBrew(StartBrew {
                script_id: Some(9),
                ..
            })
        ));
        assert_eq!(
            GrinderRequest::classify("get", "/api/v2/mako/", b""),
            GrinderRequest::PollMako
        );
        assert!(matches!(
            GrinderRequest::classify("POST", "/api/v2/brewratio", b"nope"),
            GrinderRequest::Malformed { .. }
        ));
    }
}
