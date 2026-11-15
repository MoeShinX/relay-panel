//! v1.2.21: latency monitor — the admin's list of targets, the plan pushed to
//! the nodes, and what the panel accepts back from them.
//!
//! The nodes do the measuring (see `crates/node/src/latency.rs`); the panel
//! only holds the configuration, validates and stores what nodes report, and
//! answers the admin's chart queries.
//!
//! Configuration lives in two places: the target list in `latency_targets`
//! (rows that history refers to), and the probe interval as a small JSON value
//! in the kvs table, like the site and notification settings — one number does
//! not earn a table.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::db::error::DbError;
use crate::db::repo::{
    LatencyBucket, LatencyGranularity, LatencyMinuteRow, LatencyTarget, LatencyTargetInput,
    Repository,
};
use relay_shared::protocol::{
    LatencyKind, LatencyProbeConfig, LatencyProbeTarget, LatencySample,
    LATENCY_DEFAULT_INTERVAL_SECS, LATENCY_MAX_INTERVAL_SECS, LATENCY_MAX_SENT_PER_MINUTE,
    LATENCY_MAX_TARGETS, LATENCY_MIN_INTERVAL_SECS,
};

/// kvs key of the global settings blob.
pub const CONFIG_KEY: &str = "latency:config";

/// Minute rows are kept 48 hours, hourly rows 30 days.
pub const MINUTE_RETENTION_SECS: i64 = 48 * 3600;
pub const HOURLY_RETENTION_SECS: i64 = 30 * 24 * 3600;

/// Longest target name, in characters (not bytes: names are expected in CJK).
pub const MAX_NAME_CHARS: usize = 32;
/// Longest host: a full DNS name.
pub const MAX_HOST_LEN: usize = 253;
pub const CARRIERS: [&str; 4] = ["telecom", "unicom", "mobile", "other"];

/// A reported minute is accepted when it started no later than now and no
/// more than this long ago. Wide enough for a node that was cut off from the
/// panel for a few minutes and re-sends what it kept; narrow enough that a
/// stale or forged timestamp cannot rewrite old history.
pub const ACCEPT_WINDOW_SECS: i64 = 600;

/// Slowest round trip a report may claim. The node gives up after 3 s; this
/// only rejects absurd values.
const MAX_RTT_MS: f64 = 60_000.0;

/// Most samples read from one report. 16 targets over the node's 5-minute
/// resend window is 80; this leaves room without letting a report be a lever.
const MAX_SAMPLES_PER_REPORT: usize = 256;

#[derive(Debug, Serialize, Deserialize)]
struct StoredConfig {
    interval_secs: u32,
}

/// The configured probe interval, or the default when none was ever saved (or
/// the stored value is damaged / out of range).
pub async fn get_interval(db: &dyn Repository) -> Result<u32, DbError> {
    let raw = db.get(CONFIG_KEY).await?;
    Ok(raw
        .and_then(|r| serde_json::from_str::<StoredConfig>(&r).ok())
        .map(|c| c.interval_secs)
        .filter(|s| (LATENCY_MIN_INTERVAL_SECS..=LATENCY_MAX_INTERVAL_SECS).contains(s))
        .unwrap_or(LATENCY_DEFAULT_INTERVAL_SECS))
}

async fn set_interval(db: &dyn Repository, secs: u32) -> Result<(), DbError> {
    let json = serde_json::to_string(&StoredConfig {
        interval_secs: secs,
    })
    .expect("a plain struct serialises");
    db.set(CONFIG_KEY, &json).await
}

/// What the admin sees and edits: the interval and the target list.
#[derive(Debug, Clone, Serialize)]
pub struct LatencyConfigView {
    pub interval_secs: u32,
    pub targets: Vec<LatencyTarget>,
}

pub async fn get_config(db: &dyn Repository) -> Result<LatencyConfigView, DbError> {
    Ok(LatencyConfigView {
        interval_secs: get_interval(db).await?,
        targets: db.list_latency_targets().await?,
    })
}

/// One target of a PUT body. `port` is wide on purpose: an out-of-range value
/// must reach validation and be refused with a message, not fail to decode.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetRequest {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    pub carrier: String,
    pub kind: String,
    pub host: String,
    #[serde(default)]
    pub port: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PutConfigRequest {
    pub interval_secs: i64,
    pub targets: Vec<TargetRequest>,
}

/// An IP address, or a DNS name made of letter-digit-hyphen labels whose last
/// label has a letter (so `999.1.1.1` and `1.2.3` are refused, not read as
/// names). No scheme, port, path, brackets or spaces: the node connects to
/// exactly this string.
pub fn valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > MAX_HOST_LEN {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let name = host.strip_suffix('.').unwrap_or(host);
    if name.is_empty() {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    let ok_label = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    labels.iter().all(ok_label)
        && labels
            .last()
            .is_some_and(|l| l.bytes().any(|b| b.is_ascii_alphabetic()))
}

/// Check a PUT body and turn it into repository input. The messages go to the
/// admin as they are.
pub fn validate_config(req: &PutConfigRequest) -> Result<(u32, Vec<LatencyTargetInput>), String> {
    let interval = u32::try_from(req.interval_secs)
        .ok()
        .filter(|s| (LATENCY_MIN_INTERVAL_SECS..=LATENCY_MAX_INTERVAL_SECS).contains(s))
        .ok_or_else(|| {
            format!(
                "探测间隔必须在 {} 到 {} 秒之间",
                LATENCY_MIN_INTERVAL_SECS, LATENCY_MAX_INTERVAL_SECS
            )
        })?;
    if req.targets.len() > LATENCY_MAX_TARGETS {
        return Err(format!("最多只能配置 {} 个目标", LATENCY_MAX_TARGETS));
    }

    let mut seen_ids = BTreeSet::new();
    let mut out = Vec::with_capacity(req.targets.len());
    for (i, t) in req.targets.iter().enumerate() {
        let n = i + 1;
        if let Some(id) = t.id {
            if !seen_ids.insert(id) {
                return Err(format!("第 {n} 个目标的 id 重复"));
            }
        }
        let name = t.name.trim();
        if name.is_empty() {
            return Err(format!("第 {n} 个目标的名称不能为空"));
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(format!(
                "第 {n} 个目标的名称不能超过 {MAX_NAME_CHARS} 个字符"
            ));
        }
        if !CARRIERS.contains(&t.carrier.as_str()) {
            return Err(format!("第 {n} 个目标的运营商无效"));
        }
        let kind = LatencyKind::from_db_str(&t.kind)
            .ok_or_else(|| format!("第 {n} 个目标的探测类型必须是 tcp 或 icmp"))?;
        let host = t.host.trim();
        if !valid_host(host) {
            return Err(format!("第 {n} 个目标的主机必须是 IP 地址或域名"));
        }
        let port = match (kind, t.port) {
            (LatencyKind::Tcp, Some(p)) if (1..=65535).contains(&p) => Some(p as i32),
            (LatencyKind::Tcp, _) => {
                return Err(format!(
                    "第 {n} 个目标是 TCP 探测，端口必须在 1 到 65535 之间"
                ))
            }
            (LatencyKind::Icmp, None) => None,
            (LatencyKind::Icmp, Some(_)) => {
                return Err(format!("第 {n} 个目标是 ICMP 探测，不能填写端口"))
            }
        };
        out.push(LatencyTargetInput {
            id: t.id,
            name: name.to_string(),
            carrier: t.carrier.clone(),
            kind: kind.to_db_str().to_string(),
            host: host.to_string(),
            port,
        });
    }
    Ok((interval, out))
}

/// The plan a node is given: nothing when there are no targets (the node then
/// probes nothing), otherwise the interval and what to probe. Names, carriers
/// and ordering stay on the panel.
pub fn probe_plan(interval_secs: u32, targets: &[LatencyTarget]) -> Option<LatencyProbeConfig> {
    let targets: Vec<LatencyProbeTarget> = targets
        .iter()
        .filter_map(|t| {
            Some(LatencyProbeTarget {
                id: t.id,
                kind: LatencyKind::from_db_str(&t.kind)?,
                host: t.host.clone(),
                port: t.port.and_then(|p| u16::try_from(p).ok()),
            })
        })
        .collect();
    // Sorted by id: the order the admin shows them in is a panel concern, and a
    // plan that only re-orders must compare equal to the one before it.
    let mut targets = targets;
    targets.sort_by_key(|t| t.id);
    (!targets.is_empty()).then_some(LatencyProbeConfig {
        interval_secs,
        targets,
    })
}

/// The plan for the current configuration (what `build_node_config` attaches).
pub async fn current_probe_plan(
    db: &dyn Repository,
) -> Result<Option<LatencyProbeConfig>, DbError> {
    let targets = db.list_latency_targets().await?;
    if targets.is_empty() {
        return Ok(None);
    }
    Ok(probe_plan(get_interval(db).await?, &targets))
}

#[derive(Debug)]
pub enum ConfigError {
    Invalid(String),
    /// A target id that no longer exists (deleted since the admin loaded the
    /// page); nothing was changed.
    UnknownTarget,
    Db(DbError),
}

/// The outcome of a successful PUT.
pub struct ConfigSaved {
    pub view: LatencyConfigView,
    /// Whether what the nodes probe changed (targets' kind / host / port, which
    /// ones exist, or the interval) — the only case that needs them told.
    /// Renaming a target or re-ordering the list does not.
    pub plan_changed: bool,
    pub added: usize,
    pub removed: usize,
}

/// Replace the whole configuration. Validates first; the target list and the
/// interval are then written, and the caller broadcasts when `plan_changed`.
pub async fn save_config(
    db: &dyn Repository,
    now: &str,
    req: &PutConfigRequest,
) -> Result<ConfigSaved, ConfigError> {
    let (interval, inputs) = validate_config(req).map_err(ConfigError::Invalid)?;

    let before = current_probe_plan(db).await.map_err(ConfigError::Db)?;
    let before_ids: BTreeSet<i64> = db
        .list_latency_targets()
        .await
        .map_err(ConfigError::Db)?
        .iter()
        .map(|t| t.id)
        .collect();

    let targets = db
        .replace_latency_targets(now, &inputs)
        .await
        .map_err(ConfigError::Db)?
        .ok_or(ConfigError::UnknownTarget)?;
    set_interval(db, interval).await.map_err(ConfigError::Db)?;

    let after = probe_plan(interval, &targets);
    let after_ids: BTreeSet<i64> = targets.iter().map(|t| t.id).collect();
    Ok(ConfigSaved {
        plan_changed: before != after,
        added: after_ids.difference(&before_ids).count(),
        removed: before_ids.difference(&after_ids).count(),
        view: LatencyConfigView {
            interval_secs: interval,
            targets,
        },
    })
}

/// What a report's latency samples turned into.
#[derive(Debug, Default, PartialEq)]
pub struct ScreenedSamples {
    /// Rows to store.
    pub accepted: Vec<LatencyMinuteRow>,
    /// Targets (that exist) the node says it cannot probe.
    pub unsupported: Vec<i64>,
}

/// Decide which of a node's reported samples to keep. Nothing here is trusted:
/// a sample is dropped unless its target exists, its minute is aligned and
/// recent, and its numbers are finite, non-negative and consistent. One bad
/// sample never costs the others in the same report.
pub fn screen_samples(
    targets: &[LatencyTarget],
    samples: &[LatencySample],
    now_ts: i64,
) -> ScreenedSamples {
    let known: BTreeSet<i64> = targets.iter().map(|t| t.id).collect();
    let mut unsupported = BTreeSet::new();
    // Keyed so a minute named twice in one report keeps the later one — the
    // same rule the store applies across reports.
    let mut accepted: BTreeMap<(i64, i64), LatencyMinuteRow> = BTreeMap::new();

    for s in samples.iter().take(MAX_SAMPLES_PER_REPORT) {
        if !known.contains(&s.target_id) {
            continue;
        }
        if s.unsupported {
            unsupported.insert(s.target_id);
            continue;
        }
        if s.minute_ts % 60 != 0
            || s.minute_ts > now_ts
            || s.minute_ts < now_ts - ACCEPT_WINDOW_SECS
        {
            continue;
        }
        if s.sent == 0 || s.sent > LATENCY_MAX_SENT_PER_MINUTE || s.lost > s.sent {
            continue;
        }
        let numbers = [s.rtt_sum_ms, s.rtt_min_ms, s.rtt_max_ms];
        if numbers
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0 || *v > MAX_RTT_MS * f64::from(s.sent))
        {
            continue;
        }
        let answered = s.sent - s.lost;
        let (sum, min, max) = if answered == 0 {
            // Nothing answered, so there is no round trip to keep, whatever
            // the node put in the fields.
            (0.0, None, None)
        } else {
            let avg = s.rtt_sum_ms / f64::from(answered);
            // The average of the answered probes has to sit between their
            // minimum and maximum (a hair of slack for float rounding).
            let slack = 1e-6 * (1.0 + s.rtt_max_ms);
            if s.rtt_min_ms > s.rtt_max_ms
                || s.rtt_max_ms > MAX_RTT_MS
                || avg < s.rtt_min_ms - slack
                || avg > s.rtt_max_ms + slack
            {
                continue;
            }
            (s.rtt_sum_ms, Some(s.rtt_min_ms), Some(s.rtt_max_ms))
        };
        accepted.insert(
            (s.target_id, s.minute_ts),
            LatencyMinuteRow {
                target_id: s.target_id,
                minute_ts: s.minute_ts,
                sent: i64::from(s.sent),
                lost: i64::from(s.lost),
                rtt_sum_ms: sum,
                rtt_min_ms: min,
                rtt_max_ms: max,
            },
        );
    }
    ScreenedSamples {
        accepted: accepted.into_values().collect(),
        unsupported: unsupported.into_iter().collect(),
    }
}

// ── Reading a node's series back ──

/// The time ranges the chart offers.
pub fn range_secs(range: &str) -> Option<i64> {
    match range {
        "1h" => Some(3600),
        "6h" => Some(6 * 3600),
        "24h" => Some(24 * 3600),
        "7d" => Some(7 * 24 * 3600),
        "30d" => Some(30 * 24 * 3600),
        _ => None,
    }
}

/// Resolve the `granularity` query value for a range: `auto` picks what the
/// range reads best at, and a combination the data cannot answer is refused —
/// minute rows only go back 48 hours, so 7 and 30 days are hourly only.
pub fn resolve_granularity(range: &str, granularity: &str) -> Result<LatencyGranularity, String> {
    let long_range = matches!(range, "7d" | "30d");
    let wanted = match granularity {
        "auto" => None,
        "1m" => Some(LatencyGranularity::Minute1),
        "5m" => Some(LatencyGranularity::Minute5),
        "1h" => Some(LatencyGranularity::Hour1),
        other => {
            return Err(format!(
                "granularity 只能是 auto / 1m / 5m / 1h，收到: {other}"
            ))
        }
    };
    match wanted {
        None => Ok(match range {
            "1h" | "6h" => LatencyGranularity::Minute1,
            "24h" => LatencyGranularity::Minute5,
            _ => LatencyGranularity::Hour1,
        }),
        Some(g) if long_range && g != LatencyGranularity::Hour1 => {
            Err(format!("{range} 的数据只保留小时粒度，请选择 1h 或 auto"))
        }
        Some(g) => Ok(g),
    }
}

pub fn granularity_name(g: LatencyGranularity) -> &'static str {
    match g {
        LatencyGranularity::Minute1 => "1m",
        LatencyGranularity::Minute5 => "5m",
        LatencyGranularity::Hour1 => "1h",
    }
}

#[derive(Debug, Serialize, PartialEq)]
pub struct SeriesPoint {
    /// Unix seconds of the bucket start.
    pub ts: i64,
    /// Mean round trip of the answered probes; null when all were lost.
    pub avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Percent of probes lost, 0 to 100.
    pub loss: f64,
}

#[derive(Debug, Serialize)]
pub struct TargetSeries {
    pub target_id: i64,
    pub name: String,
    pub carrier: String,
    pub kind: String,
    pub points: Vec<SeriesPoint>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TargetSummary {
    pub target_id: i64,
    /// The newest minute's mean round trip (null when none in the last ten
    /// minutes, or when that minute lost every probe) and when it started.
    pub latest: Option<f64>,
    pub latest_ts: Option<i64>,
    pub latest_loss: Option<f64>,
    /// Mean round trip over the whole range, and percent lost over it. Null
    /// when the range holds no data / no answered probe.
    pub avg: Option<f64>,
    pub loss: Option<f64>,
}

fn loss_percent(sent: i64, lost: i64) -> f64 {
    if sent <= 0 {
        0.0
    } else {
        lost as f64 * 100.0 / sent as f64
    }
}

fn mean(sum: f64, sent: i64, lost: i64) -> Option<f64> {
    let answered = sent - lost;
    (answered > 0).then(|| sum / answered as f64)
}

/// Shape stored buckets into one series per configured target (a target with
/// no data still gets an empty one, so the page can show its card) and a
/// summary for each.
pub fn build_series(
    targets: &[LatencyTarget],
    buckets: &[LatencyBucket],
    latest: &[LatencyMinuteRow],
) -> (Vec<TargetSeries>, Vec<TargetSummary>) {
    let mut series = Vec::with_capacity(targets.len());
    let mut summary = Vec::with_capacity(targets.len());
    for t in targets {
        let mine: Vec<&LatencyBucket> = buckets.iter().filter(|b| b.target_id == t.id).collect();
        let (mut sent, mut lost, mut sum) = (0_i64, 0_i64, 0.0_f64);
        let points: Vec<SeriesPoint> = mine
            .iter()
            .map(|b| {
                sent += b.sent;
                lost += b.lost;
                sum += b.rtt_sum_ms;
                SeriesPoint {
                    ts: b.ts,
                    avg: mean(b.rtt_sum_ms, b.sent, b.lost),
                    min: b.rtt_min_ms,
                    max: b.rtt_max_ms,
                    loss: loss_percent(b.sent, b.lost),
                }
            })
            .collect();
        let last = latest.iter().find(|r| r.target_id == t.id);
        summary.push(TargetSummary {
            target_id: t.id,
            latest: last.and_then(|r| mean(r.rtt_sum_ms, r.sent, r.lost)),
            latest_ts: last.map(|r| r.minute_ts),
            latest_loss: last.map(|r| loss_percent(r.sent, r.lost)),
            avg: mean(sum, sent, lost),
            loss: (sent > 0).then(|| loss_percent(sent, lost)),
        });
        series.push(TargetSeries {
            target_id: t.id,
            name: t.name.clone(),
            carrier: t.carrier.clone(),
            kind: t.kind.clone(),
            points,
        });
    }
    (series, summary)
}

/// Align the start of a query down to its bucket, so the first bucket is
/// whole rather than a stub of whatever fell after `now - range`.
pub fn align_down(ts: i64, bucket_secs: i64) -> i64 {
    ts.div_euclid(bucket_secs) * bucket_secs
}

/// A node's stored status JSON, found by node id alone (the key also carries
/// the group). When the node has records under several groups the freshest
/// report wins.
pub async fn find_node_status(
    db: &dyn Repository,
    node_id: &str,
) -> Result<Option<serde_json::Value>, DbError> {
    let rows = db.scan_prefix("node_status:").await?;
    let mut best: Option<(String, serde_json::Value)> = None;
    for (key, value) in rows {
        let Some(rest) = key.strip_prefix("node_status:") else {
            continue;
        };
        if rest.split_once(':').map(|(_, n)| n) != Some(node_id) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&value) else {
            continue;
        };
        let seen = v
            .get("last_seen")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        if best.as_ref().is_none_or(|(b, _)| seen > *b) {
            best = Some((seen, v));
        }
    }
    Ok(best.map(|(_, v)| v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: i64) -> LatencyTarget {
        LatencyTarget {
            id,
            name: format!("t{id}"),
            carrier: "telecom".into(),
            kind: "tcp".into(),
            host: "192.0.2.1".into(),
            port: Some(443),
            sort_order: 0,
            created_at: "2026-01-01 00:00:00".into(),
        }
    }

    fn req(targets: Vec<TargetRequest>) -> PutConfigRequest {
        PutConfigRequest {
            interval_secs: 10,
            targets,
        }
    }

    fn tcp(name: &str, host: &str, port: Option<i64>) -> TargetRequest {
        TargetRequest {
            id: None,
            name: name.into(),
            carrier: "telecom".into(),
            kind: "tcp".into(),
            host: host.into(),
            port,
        }
    }

    #[test]
    fn hosts_must_be_an_address_or_a_name() {
        for ok in [
            "192.0.2.1",
            "2001:db8::1",
            "example.com",
            "a-b.example.com",
            "example.com.",
            "localhost2x",
        ] {
            assert!(valid_host(ok), "{ok} should be accepted");
        }
        for bad in [
            "",
            "999.1.1.1",
            "1.2.3",
            "http://example.com",
            "example.com:443",
            "exa mple.com",
            "-a.example.com",
            "a..b",
            "[2001:db8::1]",
            "example.com/path",
        ] {
            assert!(!valid_host(bad), "{bad} should be refused");
        }
        assert!(!valid_host(&"a".repeat(64)), "label over 63");
        assert!(
            !valid_host(&format!("{}.com", "a.".repeat(130))),
            "name over 253"
        );
    }

    #[test]
    fn config_validation_matches_the_card() {
        // happy path, with the name trimmed
        let (iv, out) = validate_config(&req(vec![tcp(" 电信 ", "192.0.2.1", Some(443))])).unwrap();
        assert_eq!(iv, 10);
        assert_eq!(out[0].name, "电信");
        assert_eq!(out[0].port, Some(443));

        // interval bounds
        for bad in [4, 61, 0, -1, i64::MAX] {
            let mut r = req(vec![]);
            r.interval_secs = bad;
            assert!(validate_config(&r).is_err(), "interval {bad}");
        }
        for ok in [5, 60] {
            let mut r = req(vec![]);
            r.interval_secs = ok;
            assert!(validate_config(&r).is_ok(), "interval {ok}");
        }

        // name: empty, too long (counted in characters)
        assert!(validate_config(&req(vec![tcp("  ", "192.0.2.1", Some(80))])).is_err());
        assert!(validate_config(&req(vec![tcp(&"电".repeat(32), "192.0.2.1", Some(80))])).is_ok());
        assert!(validate_config(&req(vec![tcp(&"电".repeat(33), "192.0.2.1", Some(80))])).is_err());

        // tcp needs a port in range; icmp must have none
        for bad in [None, Some(0), Some(65536), Some(-1)] {
            assert!(validate_config(&req(vec![tcp("a", "192.0.2.1", bad)])).is_err());
        }
        let mut icmp = tcp("a", "192.0.2.1", None);
        icmp.kind = "icmp".into();
        assert!(validate_config(&req(vec![icmp.clone()])).is_ok());
        icmp.port = Some(443);
        assert!(validate_config(&req(vec![icmp])).is_err());

        // carrier and kind are closed sets
        let mut bad = tcp("a", "192.0.2.1", Some(80));
        bad.carrier = "cmcc".into();
        assert!(validate_config(&req(vec![bad])).is_err());
        let mut bad = tcp("a", "192.0.2.1", Some(80));
        bad.kind = "http".into();
        assert!(validate_config(&req(vec![bad])).is_err());

        // at most 16, ids not repeated
        let many: Vec<_> = (0..17).map(|_| tcp("a", "192.0.2.1", Some(80))).collect();
        assert!(validate_config(&req(many[..16].to_vec())).is_ok());
        assert!(validate_config(&req(many)).is_err());
        let mut a = tcp("a", "192.0.2.1", Some(80));
        a.id = Some(3);
        assert!(validate_config(&req(vec![a.clone(), a])).is_err());
    }

    #[test]
    fn the_plan_is_empty_without_targets_and_carries_only_what_a_node_needs() {
        assert_eq!(probe_plan(10, &[]), None);
        let plan = probe_plan(15, &[target(7)]).unwrap();
        assert_eq!(plan.interval_secs, 15);
        assert_eq!(
            plan.targets,
            vec![LatencyProbeTarget {
                id: 7,
                kind: LatencyKind::Tcp,
                host: "192.0.2.1".into(),
                port: Some(443),
            }]
        );
    }

    fn sample(target_id: i64, minute_ts: i64) -> LatencySample {
        LatencySample {
            target_id,
            minute_ts,
            sent: 6,
            lost: 1,
            rtt_sum_ms: 50.0,
            rtt_min_ms: 8.0,
            rtt_max_ms: 12.0,
            unsupported: false,
        }
    }

    /// A minute boundary (1_700_000_100 = 28_333_335 * 60).
    const NOW: i64 = 1_700_000_100;

    #[test]
    fn screening_keeps_good_samples_and_drops_each_kind_of_bad_one() {
        assert_eq!(NOW % 60, 0);
        let targets = [target(1)];
        let good = sample(1, NOW - 120);
        let r = screen_samples(&targets, &[good.clone()], NOW);
        assert_eq!(r.accepted.len(), 1);
        assert_eq!(r.accepted[0].lost, 1);
        assert_eq!(r.accepted[0].rtt_min_ms, Some(8.0));

        let drops: Vec<(&str, LatencySample)> = vec![
            ("unknown target", sample(2, NOW - 120)),
            ("not aligned", sample(1, NOW - 119)),
            ("in the future", sample(1, NOW + 60)),
            ("older than ten minutes", sample(1, NOW - 660)),
            (
                "lost > sent",
                LatencySample {
                    lost: 7,
                    ..good.clone()
                },
            ),
            (
                "sent 0",
                LatencySample {
                    sent: 0,
                    lost: 0,
                    ..good.clone()
                },
            ),
            (
                "sent > 60",
                LatencySample {
                    sent: 61,
                    ..good.clone()
                },
            ),
            (
                "NaN",
                LatencySample {
                    rtt_sum_ms: f64::NAN,
                    ..good.clone()
                },
            ),
            (
                "infinite",
                LatencySample {
                    rtt_max_ms: f64::INFINITY,
                    ..good.clone()
                },
            ),
            (
                "negative",
                LatencySample {
                    rtt_min_ms: -1.0,
                    ..good.clone()
                },
            ),
            (
                "min > max",
                LatencySample {
                    rtt_min_ms: 13.0,
                    ..good.clone()
                },
            ),
            (
                "mean above max",
                LatencySample {
                    rtt_sum_ms: 500.0,
                    ..good.clone()
                },
            ),
            (
                "mean below min",
                LatencySample {
                    rtt_sum_ms: 5.0,
                    ..good.clone()
                },
            ),
        ];
        for (why, s) in drops {
            assert!(
                screen_samples(&targets, &[s], NOW).accepted.is_empty(),
                "{why} must be dropped"
            );
        }
        // the edges of the window are inside it
        assert_eq!(
            screen_samples(&targets, &[sample(1, NOW)], NOW)
                .accepted
                .len(),
            1
        );
        assert_eq!(
            screen_samples(&targets, &[sample(1, NOW - 600)], NOW)
                .accepted
                .len(),
            1
        );
        // one bad sample does not cost the good one beside it
        let mixed = screen_samples(&targets, &[sample(2, NOW - 60), good], NOW);
        assert_eq!(mixed.accepted.len(), 1);
    }

    #[test]
    fn a_fully_lost_minute_keeps_no_round_trip() {
        let all_lost = LatencySample {
            lost: 6,
            rtt_sum_ms: 99.0,
            rtt_min_ms: 1.0,
            rtt_max_ms: 2.0,
            ..sample(1, NOW - 60)
        };
        let r = screen_samples(&[target(1)], &[all_lost], NOW);
        assert_eq!(r.accepted.len(), 1);
        assert_eq!(r.accepted[0].rtt_sum_ms, 0.0);
        assert_eq!(r.accepted[0].rtt_min_ms, None);
        assert_eq!(r.accepted[0].rtt_max_ms, None);
    }

    #[test]
    fn unsupported_is_recorded_for_known_targets_only_and_stores_nothing() {
        let unsupported = |id| LatencySample {
            unsupported: true,
            sent: 0,
            lost: 0,
            rtt_sum_ms: 0.0,
            rtt_min_ms: 0.0,
            rtt_max_ms: 0.0,
            ..sample(id, 0)
        };
        let r = screen_samples(&[target(1)], &[unsupported(1), unsupported(9)], NOW);
        assert!(r.accepted.is_empty());
        assert_eq!(r.unsupported, vec![1]);
    }

    #[test]
    fn a_minute_named_twice_in_one_report_keeps_the_later_one() {
        let first = sample(1, NOW - 60);
        let second = LatencySample {
            lost: 0,
            ..first.clone()
        };
        let r = screen_samples(&[target(1)], &[first, second], NOW);
        assert_eq!(r.accepted.len(), 1);
        assert_eq!(r.accepted[0].lost, 0);
    }

    #[test]
    fn granularity_follows_the_card() {
        use LatencyGranularity::*;
        for (range, auto) in [
            ("1h", Minute1),
            ("6h", Minute1),
            ("24h", Minute5),
            ("7d", Hour1),
            ("30d", Hour1),
        ] {
            assert_eq!(resolve_granularity(range, "auto").unwrap(), auto, "{range}");
        }
        // minute data only goes back 48 hours
        for range in ["7d", "30d"] {
            assert!(resolve_granularity(range, "1m").is_err());
            assert!(resolve_granularity(range, "5m").is_err());
            assert_eq!(resolve_granularity(range, "1h").unwrap(), Hour1);
        }
        for range in ["1h", "6h", "24h"] {
            for g in ["1m", "5m", "1h"] {
                assert!(resolve_granularity(range, g).is_ok(), "{range}/{g}");
            }
        }
        assert!(resolve_granularity("1h", "2m").is_err());
        assert_eq!(range_secs("24h"), Some(86_400));
        assert_eq!(range_secs("2h"), None);
    }

    #[test]
    fn series_summarise_per_target_with_loss_and_all_lost_gaps() {
        let t = [target(1), target(2)];
        let buckets = vec![
            LatencyBucket {
                target_id: 1,
                ts: 600,
                sent: 6,
                lost: 0,
                rtt_sum_ms: 60.0,
                rtt_min_ms: Some(8.0),
                rtt_max_ms: Some(14.0),
            },
            LatencyBucket {
                target_id: 1,
                ts: 660,
                sent: 6,
                lost: 6,
                rtt_sum_ms: 0.0,
                rtt_min_ms: None,
                rtt_max_ms: None,
            },
        ];
        let latest = vec![LatencyMinuteRow {
            target_id: 1,
            minute_ts: 660,
            sent: 6,
            lost: 6,
            rtt_sum_ms: 0.0,
            rtt_min_ms: None,
            rtt_max_ms: None,
        }];
        let (series, summary) = build_series(&t, &buckets, &latest);
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].points.len(), 2);
        assert_eq!(series[0].points[0].avg, Some(10.0));
        assert_eq!(series[0].points[0].max, Some(14.0));
        assert_eq!(
            series[0].points[1].avg, None,
            "an all-lost bucket has no mean"
        );
        assert_eq!(series[0].points[1].loss, 100.0);
        // the range average is over answered probes only; loss over all
        assert_eq!(summary[0].avg, Some(10.0));
        assert_eq!(summary[0].loss, Some(50.0));
        assert_eq!(summary[0].latest, None);
        assert_eq!(summary[0].latest_ts, Some(660));
        // a target without data still has an (empty) series and a blank summary
        assert!(series[1].points.is_empty());
        assert_eq!(summary[1].avg, None);
        assert_eq!(summary[1].loss, None);
        assert_eq!(summary[1].latest_ts, None);
    }

    #[test]
    fn alignment_floors_to_the_bucket() {
        assert_eq!(align_down(1_000, 300), 900);
        assert_eq!(align_down(900, 300), 900);
        assert_eq!(align_down(7_199, 3600), 3600);
    }
}
