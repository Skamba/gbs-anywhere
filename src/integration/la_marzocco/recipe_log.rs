//! Read one recipe target from the configured grinder's ordinary log export.
//! Nothing is written to the grinder and no failed request is retried here.

use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure};
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, COOKIE, HeaderValue, SET_COOKIE};
use reqwest::{Client, Response, StatusCode, Url};
use serde_json::json;
use zip::{CompressionMethod, ZipArchive};

const ZIP_LIMIT: usize = 1024 * 1024;
const CSV_LIMIT: usize = 2 * 1024 * 1024;
const LOGIN_LIMIT: usize = 4096;
const ROW_LIMIT: usize = 5000;
const COLUMN_LIMIT: usize = 48;
const ROW_BYTES: usize = 4096;
const BUDGET: Duration = Duration::from_secs(6);
const REQUIRED: [&str; 6] = [
    "grind_uuid",
    "grind_date_time",
    "recipe_brew_weight",
    "grind_successfull",
    "recipe_gbs_enabled",
    "recipe_index",
];

/// The address must come from the current E65/E80 link, never a separate form
/// field. The ordinary web export uses the same host on HTTPS port 443.
/// The caller must discard the result if that link or the active grind changed.
pub async fn target_weight(
    address: &str,
    owner_password: &str,
    grind_uuid: &str,
) -> anyhow::Result<u16> {
    let base = web_base(address)?;
    fetch_target(&base, owner_password, grind_uuid, BUDGET).await
}

fn web_base(address: &str) -> anyhow::Result<Url> {
    let config = crate::server::wifi_grinder::WifiConfig::from_host(address)
        .map_err(|_| anyhow!("The grinder address is invalid"))?;
    let mut base =
        Url::parse(&config.base_url).map_err(|_| anyhow!("The grinder address is invalid"))?;
    ensure!(
        base.scheme() == "https"
            && base.host_str().is_some()
            && base.username().is_empty()
            && base.password().is_none()
            && base.path() == "/"
            && base.query().is_none()
            && base.fragment().is_none(),
        "The grinder address must identify one HTTPS host"
    );
    base.set_port(None)
        .map_err(|_| anyhow!("The grinder address is invalid"))?;
    base.set_path("/rest_api/");
    Ok(base)
}

async fn fetch_target(
    base: &Url,
    password: &str,
    grind_uuid: &str,
    budget: Duration,
) -> anyhow::Result<u16> {
    ensure!(
        !password.is_empty() && password.len() <= 64 && !password.contains('\0'),
        "The grinder Owner password must contain 1 to 64 bytes"
    );
    ensure!(valid_uuid(grind_uuid), "The grind identifier is invalid");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = Client::builder()
        // Match WifiGrinder: its self-signed certificate has no usable host
        // name and the existing pairing stores no certificate pin. Limit this
        // exception to the configured grinder; never proxy or follow redirects.
        .tls_danger_accept_invalid_certs(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .http1_title_case_headers()
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(2))
        .timeout(budget)
        .build()
        .map_err(|_| anyhow!("The grinder log client could not be created"))?;
    tokio::time::timeout(budget, async {
        let login_url = base
            .join("login")
            .map_err(|_| anyhow!("The grinder log address is invalid"))?;
        let response = client
            .post(login_url)
            .header(ACCEPT_ENCODING, "identity")
            .json(&json!({"userRole": "Owner", "password": password}))
            .send()
            .await
            .map_err(|_| anyhow!("The grinder log login could not be completed"))?;
        ensure!(
            response.status() == StatusCode::OK,
            "The grinder log login was refused"
        );
        let cookie = session_cookie(&response)?;
        let bytes = read_limited(response, LOGIN_LIMIT).await?;
        let reply: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow!("The grinder log login reply is invalid"))?;
        ensure!(
            reply.get("success").and_then(|v| v.as_bool()) == Some(true),
            "The grinder log login was refused"
        );
        let download_url = base
            .join("download_machine_logs")
            .map_err(|_| anyhow!("The grinder log address is invalid"))?;
        let response = client
            .get(download_url)
            .header(ACCEPT_ENCODING, "identity")
            .header(COOKIE, cookie)
            .send()
            .await
            .map_err(|_| anyhow!("The grinder log download could not be completed"))?;
        ensure!(
            response.status() == StatusCode::OK,
            "The grinder log download was refused"
        );
        let bytes = read_limited(response, ZIP_LIMIT).await?;
        archive_target(&bytes, grind_uuid)
    })
    .await
    .map_err(|_| anyhow!("The grinder log read timed out"))?
}

fn session_cookie(response: &Response) -> anyhow::Result<HeaderValue> {
    let mut session = None;
    for header in response.headers().get_all(SET_COOKIE) {
        let text = header
            .to_str()
            .map_err(|_| anyhow!("The grinder log session is invalid"))?;
        let pair = text.split(';').next().unwrap_or_default();
        if let Some(value) = pair.strip_prefix("PHPSESSID=") {
            ensure!(
                session.is_none()
                    && !value.is_empty()
                    && pair.len() < 96
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b',')),
                "The grinder log session is invalid"
            );
            let mut cookie = HeaderValue::from_str(pair)
                .map_err(|_| anyhow!("The grinder log session is invalid"))?;
            cookie.set_sensitive(true);
            session = Some(cookie);
        }
    }
    session.ok_or_else(|| anyhow!("The grinder log session is missing"))
}

async fn read_limited(mut response: Response, limit: usize) -> anyhow::Result<Vec<u8>> {
    ensure!(
        response.content_length().is_none_or(|n| n <= limit as u64),
        "The grinder log reply is too large"
    );
    ensure!(
        response
            .headers()
            .get(CONTENT_ENCODING)
            .is_none_or(|v| v == "identity"),
        "The grinder log reply has an unsupported encoding"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("The grinder log reply is incomplete"))?
    {
        ensure!(
            chunk.len() <= limit - bytes.len(),
            "The grinder log reply is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && [8, 13, 18, 23]
            .iter()
            .all(|&at| value.as_bytes()[at] == b'-')
        && uuid::Uuid::parse_str(value).is_ok()
}

fn u16_at(bytes: &[u8], offset: usize) -> usize {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize
}

fn u32_at(bytes: &[u8], offset: usize) -> usize {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize
}

/// Bound entry allocation before the ZIP crate reads the directory. Only the
/// ordinary, single-disk format is needed; reject ZIP64 and appended data.
fn directory(bytes: &[u8]) -> anyhow::Result<(usize, usize)> {
    ensure!(
        (22..=ZIP_LIMIT).contains(&bytes.len()),
        "The grinder log archive size is invalid"
    );
    let end = (bytes.len().saturating_sub(65557)..=bytes.len() - 22)
        .rev()
        .find(|&at| {
            bytes[at..at + 4] == *b"PK\x05\x06" && at + 22 + u16_at(bytes, at + 20) == bytes.len()
        })
        .ok_or_else(|| anyhow!("The grinder log archive is invalid"))?;
    let entries = u16_at(bytes, end + 10);
    let start = u32_at(bytes, end + 16);
    let size = u32_at(bytes, end + 12);
    ensure!(
        (1..=16).contains(&entries)
            && u16_at(bytes, end + 4) == 0
            && u16_at(bytes, end + 6) == 0
            && u16_at(bytes, end + 8) == entries
            && start <= end
            && size == end - start,
        "The grinder log archive directory is invalid"
    );
    let mut at = start;
    let mut names = HashSet::new();
    for _ in 0..entries {
        ensure!(
            at + 46 <= end && bytes[at..at + 4] == *b"PK\x01\x02",
            "The grinder log archive directory is invalid"
        );
        let name_len = u16_at(bytes, at + 28);
        let next = at + 46 + name_len + u16_at(bytes, at + 30) + u16_at(bytes, at + 32);
        ensure!(
            next <= end && (1..160).contains(&name_len) && u16_at(bytes, at + 34) == 0,
            "The grinder log archive member is invalid"
        );
        let name = &bytes[at + 46..at + 46 + name_len];
        ensure!(
            names.insert(name)
                && !name.contains(&0)
                && !name.starts_with(b"/")
                && !name.contains(&b'\\')
                && !name.contains(&b':')
                && !name.split(|&byte| byte == b'/').any(|part| part == b".."),
            "The grinder log archive has duplicate or invalid names"
        );
        let flags = u16_at(bytes, at + 8);
        let method = u16_at(bytes, at + 10);
        let local = u32_at(bytes, at + 42);
        ensure!(
            flags & !(8 | 2048) == 0
                && matches!(method, 0 | 8)
                && local <= start
                && start - local >= 30
                && bytes[local..local + 4] == *b"PK\x03\x04"
                && u16_at(bytes, local + 6) == flags
                && u16_at(bytes, local + 8) == method
                && u16_at(bytes, local + 26) == name_len,
            "The grinder log archive member is invalid"
        );
        let data = local + 30 + name_len + u16_at(bytes, local + 28);
        ensure!(
            data <= start
                && bytes[local + 30..local + 30 + name_len] == *name
                && u32_at(bytes, at + 20) <= start - data,
            "The grinder log archive member is invalid"
        );
        if flags & 8 == 0 {
            ensure!(
                bytes[local + 14..local + 26] == bytes[at + 16..at + 28],
                "The grinder log archive headers disagree"
            );
        }
        at = next;
    }
    ensure!(at == end, "The grinder log archive directory is invalid");
    Ok((entries, start))
}

fn archive_target(bytes: &[u8], grind_uuid: &str) -> anyhow::Result<u16> {
    ensure!(valid_uuid(grind_uuid), "The grind identifier is invalid");
    let (entries, _) = directory(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| anyhow!("The grinder log archive is invalid"))?;
    ensure!(
        archive.len() == entries,
        "The grinder log archive has duplicate members"
    );
    let mut selected = None;
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|_| anyhow!("The grinder log archive member is invalid"))?;
        ensure!(
            file.enclosed_name().is_some() && !file.is_symlink() && !file.encrypted(),
            "The grinder log archive member is invalid"
        );
        let name = file
            .name()
            .map_err(|_| anyhow!("The grinder log archive member is invalid"))?;
        if name.contains("GrindLogs") && name.ends_with(".csv") {
            ensure!(
                selected.is_none()
                    && file.is_file()
                    && file.size() > 0
                    && file.size() <= CSV_LIMIT as u64
                    && matches!(
                        file.compression(),
                        CompressionMethod::Stored | CompressionMethod::Deflated
                    ),
                "The grinder log archive must contain one bounded grind CSV"
            );
            selected = Some(index);
        }
    }
    let index = selected.ok_or_else(|| anyhow!("The grinder log archive has no grind CSV"))?;
    let file = archive
        .by_index(index)
        .map_err(|_| anyhow!("The grinder log CSV cannot be read"))?;
    let expected = file.size() as usize;
    let mut csv = Vec::new();
    // Reading through EOF checks the member's CRC. One extra byte detects a
    // dishonest size without allowing an unbounded decompression allocation.
    file.take((CSV_LIMIT + 1) as u64)
        .read_to_end(&mut csv)
        .map_err(|_| anyhow!("The grinder log CSV is damaged"))?;
    ensure!(
        csv.len() == expected && csv.len() <= CSV_LIMIT,
        "The grinder log CSV size is invalid"
    );
    csv_target(&csv, grind_uuid)
}

/// The csv crate deliberately tolerates bad quoting. Validate the ordinary CSV
/// grammar first so a damaged quote cannot change which grind a row describes.
fn strict_csv(bytes: &[u8]) -> anyhow::Result<()> {
    #[derive(Clone, Copy, PartialEq)]
    enum Field {
        Start,
        Plain,
        Quoted,
        Closed,
    }
    let mut state = Field::Start;
    let mut row_bytes = 0;
    let mut after_cr = false;
    for &byte in bytes {
        if after_cr && byte == b'\n' {
            after_cr = false;
            continue;
        }
        after_cr = false;
        row_bytes += 1;
        ensure!(
            byte != 0 && row_bytes <= ROW_BYTES,
            "The grinder log CSV row is invalid"
        );
        if state == Field::Quoted {
            if byte == b'"' {
                state = Field::Closed;
            }
            continue;
        }
        match byte {
            b'"' if state == Field::Start || state == Field::Closed => state = Field::Quoted,
            b'"' => bail!("The grinder log CSV quoting is invalid"),
            b',' => state = Field::Start,
            b'\r' | b'\n' => {
                state = Field::Start;
                row_bytes = 0;
                after_cr = byte == b'\r';
            }
            _ if state == Field::Closed => bail!("The grinder log CSV quoting is invalid"),
            _ => state = Field::Plain,
        }
    }
    ensure!(
        state != Field::Quoted,
        "The grinder log CSV quoting is incomplete"
    );
    Ok(())
}

fn csv_target(bytes: &[u8], grind_uuid: &str) -> anyhow::Result<u16> {
    strict_csv(bytes)?;
    let mut reader = csv::ReaderBuilder::new().flexible(false).from_reader(bytes);
    let headers = reader
        .headers()
        .map_err(|_| anyhow!("The grinder log CSV header is invalid"))?
        .clone();
    ensure!(
        headers.len() <= COLUMN_LIMIT
            && headers.iter().all(|h| h.len() <= 80)
            && headers.iter().collect::<HashSet<_>>().len() == headers.len(),
        "The grinder log CSV header is invalid"
    );
    let mut columns = [0; 6];
    for (i, name) in REQUIRED.iter().enumerate() {
        columns[i] = headers
            .iter()
            .position(|h| h == *name)
            .ok_or_else(|| anyhow!("The grinder log CSV header is incomplete"))?;
    }
    let mut target = None;
    for (index, row) in reader.records().enumerate() {
        ensure!(index < ROW_LIMIT, "The grinder log CSV has too many rows");
        let row = row.map_err(|_| anyhow!("The grinder log CSV row is invalid"))?;
        ensure!(
            columns.iter().all(|&i| row[i].len() <= 80),
            "The grinder log CSV field is too long"
        );
        if &row[columns[0]] != grind_uuid {
            continue;
        }
        ensure!(
            target.is_none(),
            "The grinder log contains the grind more than once"
        );
        ensure!(
            &row[columns[3]] == "1" && &row[columns[4]] == "1",
            "The grind did not complete successfully with Grind-by-Sync enabled"
        );
        let number = |text: &str| -> Option<u32> {
            (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
                .then(|| text.parse().ok())
                .flatten()
        };
        let mg = number(&row[columns[2]]).ok_or_else(|| anyhow!("The recipe target is invalid"))?;
        ensure!(
            (1000..=200000).contains(&mg) && mg % 1000 == 0,
            "The recipe target must be a whole number of grams from 1 to 200"
        );
        ensure!(
            number(&row[columns[5]]).is_some_and(|n| n <= 255) && row[columns[1]].len() == 20,
            "The matching grinder log row is invalid"
        );
        target = Some((mg / 1000) as u16);
    }
    target.ok_or_else(|| anyhow!("The active grind is not in the grinder log"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::{Request, State};
    use axum::response::IntoResponse;
    use axum::routing::any;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use zip::write::SimpleFileOptions;

    const UUID: &str = "11111111-1111-4111-8111-111111111111";
    const OTHER: &str = "22222222-2222-4222-8222-222222222222";
    const HEADER: &str = "grind_uuid,grind_date_time,recipe_brew_weight,grind_successfull,recipe_gbs_enabled,recipe_index,grind_debug_data\r\n";

    fn row(id: &str, weight: &str, success: &str, enabled: &str) -> String {
        format!(
            "{id},2026-10-01T00:00:00Z,{weight},{success},{enabled},1,\"quoted,\ntext\"\"here\"\r\n"
        )
    }

    fn csv() -> String {
        format!("{HEADER}{}", row(UUID, "30000", "1", "1"))
    }

    fn archive(members: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in members {
            zip.start_file(
                *name,
                SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn packed(csv: &[u8]) -> Vec<u8> {
        archive(
            &[
                ("HemroGrindLogs_test.csv", csv),
                ("ServiceLogs.csv", b"ignored"),
            ],
            CompressionMethod::Deflated,
        )
    }

    #[test]
    fn exact_successful_grind_and_whole_gram_boundaries() {
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let text = format!(
                "{HEADER}{}{}",
                row(OTHER, "200000", "1", "1"),
                row(UUID, "30000", "1", "1")
            );
            assert_eq!(
                archive_target(
                    &archive(&[("GrindLogs.csv", text.as_bytes())], method),
                    UUID
                )
                .unwrap(),
                30
            );
        }
        for (mg, grams) in [("1000", 1), ("200000", 200)] {
            let text = format!("{HEADER}{}", row(UUID, mg, "1", "1"));
            assert_eq!(
                archive_target(&packed(text.as_bytes()), UUID).unwrap(),
                grams
            );
        }
        assert!(archive_target(&packed(csv().as_bytes()), OTHER).is_err());
        assert!(archive_target(&packed(csv().as_bytes()), "invalid").is_err());
    }

    #[test]
    fn invalid_or_ambiguous_matching_rows_fail_closed() {
        for weight in [
            "0",
            "999",
            "30500",
            "200001",
            "-30000",
            "+30000",
            "30g",
            "30.0",
            "99999999999999999999",
        ] {
            let text = format!("{HEADER}{}", row(UUID, weight, "1", "1"));
            assert!(
                archive_target(&packed(text.as_bytes()), UUID).is_err(),
                "{weight}"
            );
        }
        for (success, enabled) in [("0", "1"), ("1", "0"), ("true", "1")] {
            let text = format!("{HEADER}{}", row(UUID, "30000", success, enabled));
            assert!(archive_target(&packed(text.as_bytes()), UUID).is_err());
        }
        let repeated = format!("{}{}", csv(), row(UUID, "30000", "1", "1"));
        assert!(archive_target(&packed(repeated.as_bytes()), UUID).is_err());
        let repeated = format!("{}{}", csv(), row(UUID, "20000", "0", "1"));
        assert!(archive_target(&packed(repeated.as_bytes()), UUID).is_err());
    }

    #[test]
    fn csv_grammar_headers_and_size_limits_are_checked_after_the_match_too() {
        let good = csv();
        let mut invalid = vec![
            good.replace("recipe_brew_weight", "missing"),
            good.replace("grind_debug_data", "recipe_brew_weight"),
            good.replace("\"quoted,\ntext\"\"here\"", "unquoted\"quote"),
            good.replace("\"quoted,\ntext\"\"here\"", "\"closed\"junk"),
            good.replace("\"quoted,\ntext\"\"here\"", "\"unclosed"),
            format!("{good}missing,columns\n"),
            format!("{good}{}", "x".repeat(ROW_BYTES + 1)),
            good.replace("30000", "30\0"),
            good.replace(",1,\"quoted", ",256,\"quoted"),
            good.replace("2026-10-01T00:00:00Z", "bad date"),
        ];
        let mut bytes = good.as_bytes().to_vec();
        bytes.push(0xff);
        assert!(archive_target(&packed(&bytes), UUID).is_err());
        let header = format!(
            "{},{}\n",
            HEADER.trim(),
            (0..42)
                .map(|i| format!("extra{i}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        invalid.push(header);
        for text in invalid {
            assert!(archive_target(&packed(text.as_bytes()), UUID).is_err());
        }
        let mut many = HEADER.to_owned();
        for _ in 0..ROW_LIMIT - 1 {
            many.push_str(&row(OTHER, "30000", "1", "1"));
        }
        many.push_str(&row(UUID, "30000", "1", "1"));
        assert_eq!(archive_target(&packed(many.as_bytes()), UUID).unwrap(), 30);
        many.push_str(&row(OTHER, "30000", "1", "1"));
        assert!(archive_target(&packed(many.as_bytes()), UUID).is_err());
    }

    #[test]
    fn damaged_ambiguous_and_oversized_archives_fail_closed() {
        let good = csv();
        let duplicate = archive(
            &[
                ("GrindLogs_A.csv", good.as_bytes()),
                ("GrindLogs_B.csv", good.as_bytes()),
            ],
            CompressionMethod::Stored,
        );
        assert!(archive_target(&duplicate, UUID).is_err());
        let mut duplicate_name = duplicate;
        for at in 0..duplicate_name.len() - b"GrindLogs_B.csv".len() {
            if &duplicate_name[at..at + 14] == b"GrindLogs_B.csv" {
                duplicate_name[at + 10] = b'A';
            }
        }
        assert!(archive_target(&duplicate_name, UUID).is_err());
        for name in [
            "ServiceLogs.csv",
            "../GrindLogs.csv",
            "/GrindLogs.csv",
            "folder/../GrindLogs.csv",
            "C:/GrindLogs.csv",
            "folder\\GrindLogs.csv",
        ] {
            assert!(
                archive_target(
                    &archive(&[(name, good.as_bytes())], CompressionMethod::Stored),
                    UUID
                )
                .is_err(),
                "{name}"
            );
        }
        let mut damaged = archive(
            &[("GrindLogs.csv", good.as_bytes())],
            CompressionMethod::Stored,
        );
        let data = 30 + u16_at(&damaged, 26) + u16_at(&damaged, 28);
        damaged[data + 10] ^= 1;
        assert!(archive_target(&damaged, UUID).is_err());
        let clean = packed(good.as_bytes());
        for size in [0, 20, clean.len() - 1, clean.len() - 8] {
            assert!(archive_target(&clean[..size], UUID).is_err());
        }
        let mut trailer = clean.clone();
        trailer.push(0);
        assert!(archive_target(&trailer, UUID).is_err());
        let mut bad_local_name = clean.clone();
        bad_local_name[30] ^= 1;
        assert!(archive_target(&bad_local_name, UUID).is_err());
        let mut too_many = clean.clone();
        let end = too_many.len() - 22;
        too_many[end + 8..end + 12].copy_from_slice(&[17, 0, 17, 0]);
        assert!(archive_target(&too_many, UUID).is_err());
        assert!(archive_target(&vec![0; ZIP_LIMIT + 1], UUID).is_err());
        let bomb = packed(&vec![b'x'; CSV_LIMIT + 1]);
        assert!(bomb.len() < ZIP_LIMIT);
        assert!(archive_target(&bomb, UUID).is_err());
    }

    #[test]
    fn log_address_uses_only_the_configured_https_host() {
        for address in [
            "grinder.local",
            "grinder.local:30001",
            "https://grinder.local:4433/",
        ] {
            assert_eq!(
                web_base(address).unwrap().as_str(),
                "https://grinder.local/rest_api/"
            );
        }
        assert_eq!(
            web_base("[::1]:30001").unwrap().as_str(),
            "https://[::1]/rest_api/"
        );
        for address in [
            "http://grinder.local",
            "https://user:secret@grinder.local",
            "https://grinder.local/path",
            "https://grinder.local?secret=bad",
            "https://grinder.local/#bad",
        ] {
            let error = format!("{:#}", web_base(address).unwrap_err());
            assert!(!error.contains("secret") && !error.contains("grinder.local"));
        }
    }

    #[derive(Clone)]
    struct Fake {
        calls: Arc<Mutex<Vec<Call>>>,
        login: Vec<u8>,
        cookies: Vec<&'static str>,
        archive: Vec<u8>,
        status: StatusCode,
        download_status: StatusCode,
        streamed: bool,
        encoding: Option<&'static str>,
        delay: Duration,
    }

    type Call = (String, String, Vec<u8>);

    impl Fake {
        fn new() -> Self {
            Self {
                calls: Arc::default(),
                login: br#"{"success":true}"#.to_vec(),
                cookies: vec!["PHPSESSID=synthetic-session; Path=/; HttpOnly"],
                archive: packed(csv().as_bytes()),
                status: StatusCode::OK,
                download_status: StatusCode::OK,
                streamed: false,
                encoding: None,
                delay: Duration::ZERO,
            }
        }

        async fn serve(&self) -> (Url, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = Url::parse(&format!(
                "http://{}/rest_api/",
                listener.local_addr().unwrap()
            ))
            .unwrap();
            let app = axum::Router::new()
                .fallback(any(fake_request))
                .with_state(self.clone());
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            (base, task)
        }
    }

    async fn fake_request(State(fake): State<Fake>, request: Request) -> axum::response::Response {
        let path = request.uri().path().to_owned();
        let method = request.method().to_string();
        let cookie = request
            .headers()
            .get(COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = to_bytes(request.into_body(), 4096).await.unwrap().to_vec();
        fake.calls
            .lock()
            .unwrap()
            .push((format!("{method} {path}"), cookie, body));
        tokio::time::sleep(fake.delay).await;
        let login = path.ends_with("/login");
        let bytes = if login { fake.login } else { fake.archive };
        let body = if fake.streamed {
            Body::from_stream(futures_util::stream::iter([Ok::<_, std::io::Error>(bytes)]))
        } else {
            Body::from(bytes)
        };
        let mut response = (
            if login {
                fake.status
            } else {
                fake.download_status
            },
            body,
        )
            .into_response();
        if login {
            for cookie in fake.cookies {
                response
                    .headers_mut()
                    .append(SET_COOKIE, HeaderValue::from_static(cookie));
            }
        }
        response.headers_mut().insert(
            reqwest::header::LOCATION,
            HeaderValue::from_static("/unexpected"),
        );
        if let Some(encoding) = fake.encoding {
            response
                .headers_mut()
                .insert(CONTENT_ENCODING, HeaderValue::from_static(encoding));
        }
        response
    }

    #[tokio::test]
    async fn ordinary_login_download_and_cookie_are_sent_exactly_once() {
        let fake = Fake::new();
        let (base, server) = fake.serve().await;
        assert_eq!(
            fetch_target(&base, "synthetic-owner-password", UUID, BUDGET)
                .await
                .unwrap(),
            30
        );
        server.abort();
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "POST /rest_api/login");
        assert!(calls[0].1.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&calls[0].2).unwrap(),
            json!({"userRole":"Owner", "password":"synthetic-owner-password"})
        );
        assert_eq!(calls[1].0, "GET /rest_api/download_machine_logs");
        assert_eq!(calls[1].1, "PHPSESSID=synthetic-session");
        assert!(calls[1].2.is_empty());
    }

    #[tokio::test]
    async fn login_errors_redirects_and_invalid_sessions_are_not_retried_or_leaked() {
        for case in 0..8 {
            let mut fake = Fake::new();
            match case {
                0 => fake.status = StatusCode::FOUND,
                1 => fake.status = StatusCode::UNAUTHORIZED,
                2 => {
                    fake.login = br#"{"success":false,"error":"synthetic-owner-password"}"#.to_vec()
                }
                3 => fake.cookies.clear(),
                4 => fake.cookies.push("PHPSESSID=another"),
                5 => fake.cookies = vec!["PHPSESSID=invalid value"],
                6 => fake.login = vec![b'x'; LOGIN_LIMIT + 1],
                _ => fake.encoding = Some("gzip"),
            }
            let (base, server) = fake.serve().await;
            let error = fetch_target(&base, "synthetic-owner-password", UUID, BUDGET)
                .await
                .unwrap_err();
            server.abort();
            let text = format!("{error:#}");
            assert!(
                !text.contains("synthetic")
                    && !text.contains("127.0.0.1")
                    && !text.contains("http")
            );
            assert_eq!(fake.calls.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn download_size_and_total_time_are_bounded() {
        for case in 0..4 {
            let mut fake = Fake::new();
            match case {
                0 | 1 => {
                    fake.archive = vec![0; ZIP_LIMIT + 1];
                    fake.streamed = case == 1;
                }
                2 => fake.download_status = StatusCode::FOUND,
                _ => fake.download_status = StatusCode::UNAUTHORIZED,
            }
            let (base, server) = fake.serve().await;
            let error = fetch_target(&base, "synthetic", UUID, BUDGET)
                .await
                .unwrap_err();
            server.abort();
            assert_eq!(fake.calls.lock().unwrap().len(), 2);
            let text = format!("{error:#}");
            assert!(!text.contains("synthetic") && !text.contains("http"));
        }
        let mut fake = Fake::new();
        fake.delay = Duration::from_millis(100);
        let (base, server) = fake.serve().await;
        let result = fetch_target(&base, "synthetic", UUID, Duration::from_millis(20)).await;
        server.abort();
        assert!(result.is_err());
        assert!(fake.calls.lock().unwrap().len() <= 1);
    }
}
