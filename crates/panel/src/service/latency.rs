//! v1.2.21: latency monitor — each device group's list of targets, the plan
//! pushed to its nodes, and what the panel accepts back from them.
//!
//! The nodes do the measuring (see `crates/node/src/latency.rs`); the panel
//! only holds the configuration, validates and stores what nodes report, and
//! answers the admin's chart queries.
//!
//! Configuration is per device group and lives in two places: the group's
//! target list in `latency_targets` (rows that history refers to), and its
//! probe interval as a small JSON value in the kvs table under
//! `latency:interval:<group_id>`, like the site and notification settings —
//! one number does not earn a column. A group with no targets probes nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::db::error::DbError;
use crate::db::repo::{
    LatencyBucket, LatencyGranularity, LatencyMinuteRow, LatencyNodeBucket, LatencyNodeTotal,
    LatencyTarget, LatencyTargetInput, Repository,
};
use relay_shared::protocol::{
    LatencyKind, LatencyProbeConfig, LatencyProbeTarget, LatencySample,
    LATENCY_DEFAULT_INTERVAL_SECS, LATENCY_MAX_INTERVAL_SECS, LATENCY_MAX_SENT_PER_MINUTE,
    LATENCY_MAX_TARGETS, LATENCY_MIN_INTERVAL_SECS, LATENCY_RETRANSMIT_MS,
};

/// kvs key prefix of a group's probe interval.
pub const INTERVAL_KEY_PREFIX: &str = "latency:interval:";

/// kvs key of one group's probe interval.
pub fn interval_key(group_id: i64) -> String {
    format!("{INTERVAL_KEY_PREFIX}{group_id}")
}

/// kvs key prefix of a node's "latency monitor off" switch. Same
/// `{group_id}:{node_id}` suffix as `node_label:` and `node_status:`.
pub const OFF_PREFIX: &str = "latency_off:";

/// The kvs key of one node's switch. The key being present (value "1") means
/// the panel ignores the node for latency; absent means on, the default — so
/// no migration and a node record that is gone takes its switch with it.
pub fn off_key(group_id: i64, node_id: &str) -> String {
    format!("{OFF_PREFIX}{group_id}:{node_id}")
}

/// Whether the monitor is on for one node (true unless its key exists). A read
/// error counts as on: the switch only ever hides data, never loses it.
pub async fn node_enabled<R: crate::db::repo::KvsRepository + ?Sized>(
    db: &R,
    group_id: i64,
    node_id: &str,
) -> bool {
    match db.get(&off_key(group_id, node_id)).await {
        Ok(v) => v.is_none(),
        Err(e) => {
            tracing::warn!("latency switch: read failed, treating as on: {}", e);
            true
        }
    }
}

/// Every node with the monitor off, as `{group_id}:{node_id}` — the part after
/// `node_status:` in a status key, like `node_labels::all_labels`.
pub async fn all_off<R: crate::db::repo::KvsRepository + ?Sized>(
    db: &R,
) -> Result<BTreeSet<String>, DbError> {
    Ok(db
        .scan_prefix(OFF_PREFIX)
        .await?
        .into_iter()
        .filter_map(|(k, _)| k.strip_prefix(OFF_PREFIX).map(str::to_string))
        .collect())
}

/// Switch one node's monitor on or off.
pub async fn set_node_enabled<R: crate::db::repo::KvsRepository + ?Sized>(
    db: &R,
    group_id: i64,
    node_id: &str,
    enabled: bool,
) -> Result<(), DbError> {
    let key = off_key(group_id, node_id);
    if enabled {
        db.delete(&key).await.map(|_| ())
    } else {
        db.set(&key, "1").await
    }
}

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

/// A group's configured probe interval, or the default when none was ever
/// saved (or the stored value is damaged / out of range).
pub async fn get_interval(db: &dyn Repository, group_id: i64) -> Result<u32, DbError> {
    let raw = db.get(&interval_key(group_id)).await?;
    Ok(raw
        .and_then(|r| serde_json::from_str::<StoredConfig>(&r).ok())
        .map(|c| c.interval_secs)
        .filter(|s| (LATENCY_MIN_INTERVAL_SECS..=LATENCY_MAX_INTERVAL_SECS).contains(s))
        .unwrap_or(LATENCY_DEFAULT_INTERVAL_SECS))
}

async fn set_interval(db: &dyn Repository, group_id: i64, secs: u32) -> Result<(), DbError> {
    let json = serde_json::to_string(&StoredConfig {
        interval_secs: secs,
    })
    .expect("a plain struct serialises");
    db.set(&interval_key(group_id), &json).await
}

/// Drop a deleted group's interval. Its targets and history are removed by the
/// repository in the same transaction as the group itself; this is the one
/// leftover that lives in the kvs table. Best-effort for the caller: a stale
/// interval is harmless (it is only read for a group that has targets, and
/// saving targets always writes the interval again).
pub async fn forget_group(db: &dyn Repository, group_id: i64) -> Result<(), DbError> {
    db.delete(&interval_key(group_id)).await.map(|_| ())
}

/// What the admin sees and edits for one group: the interval and the target
/// list.
#[derive(Debug, Clone, Serialize)]
pub struct LatencyConfigView {
    pub interval_secs: u32,
    pub targets: Vec<LatencyTarget>,
}

pub async fn get_config(db: &dyn Repository, group_id: i64) -> Result<LatencyConfigView, DbError> {
    Ok(LatencyConfigView {
        interval_secs: get_interval(db, group_id).await?,
        targets: db.list_latency_targets(group_id).await?,
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

/// The plan for one group's current configuration (what `build_node_config`
/// attaches to that group's nodes).
pub async fn current_probe_plan(
    db: &dyn Repository,
    group_id: i64,
) -> Result<Option<LatencyProbeConfig>, DbError> {
    let targets = db.list_latency_targets(group_id).await?;
    if targets.is_empty() {
        return Ok(None);
    }
    Ok(probe_plan(get_interval(db, group_id).await?, &targets))
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

/// Replace one group's whole configuration. Validates first; the target list
/// and the interval are then written, and the caller tells the group's nodes
/// when `plan_changed`.
pub async fn save_config(
    db: &dyn Repository,
    group_id: i64,
    now: &str,
    req: &PutConfigRequest,
) -> Result<ConfigSaved, ConfigError> {
    let (interval, inputs) = validate_config(req).map_err(ConfigError::Invalid)?;

    let before = current_probe_plan(db, group_id)
        .await
        .map_err(ConfigError::Db)?;
    let before_ids: BTreeSet<i64> = db
        .list_latency_targets(group_id)
        .await
        .map_err(ConfigError::Db)?
        .iter()
        .map(|t| t.id)
        .collect();

    let targets = db
        .replace_latency_targets(group_id, now, &inputs)
        .await
        .map_err(ConfigError::Db)?
        .ok_or(ConfigError::UnknownTarget)?;
    set_interval(db, group_id, interval)
        .await
        .map_err(ConfigError::Db)?;

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

/// The retransmit figures of a sample that passed the other checks, or
/// `(None, None)` ("not reported") when they are missing, only half there, or
/// inconsistent. A bad retransmit figure never costs the sample itself: the
/// round trips are still good, only this extra is dropped.
///
/// Consistent means: no more retransmits than answered probes, a finite
/// non-negative sum that is no larger than the answered probes' total
/// (`rtt_sum`, as stored: zero for a fully lost minute), and a sum that is at
/// least `LATENCY_RETRANSMIT_MS` per retransmit (a retransmit takes that long
/// by definition) — with a hair of slack for float rounding. No retransmits
/// means a zero sum. A fully lost minute can only carry zero, which is stored
/// as 0 rather than NULL.
fn screen_retrans(s: &LatencySample, answered: u32, rtt_sum: f64) -> (Option<i64>, Option<f64>) {
    let (Some(n), Some(sum)) = (s.retrans, s.retrans_sum_ms) else {
        return (None, None);
    };
    let slack = 1e-6 * (1.0 + rtt_sum);
    let consistent = n <= answered
        && sum.is_finite()
        && sum >= 0.0
        && sum <= rtt_sum + slack
        && sum >= LATENCY_RETRANSMIT_MS * f64::from(n) - slack
        && (n > 0 || sum <= slack);
    if consistent {
        (Some(i64::from(n)), Some(sum))
    } else {
        (None, None)
    }
}

/// The retransmit-free maximum of a sample that passed the other checks, or
/// None ("not reported") when missing or inconsistent; the sample is kept
/// either way. Consistent means finite, no smaller than the minimum and no
/// larger than the maximum of the answered probes (a hair of slack for float
/// rounding), and below `LATENCY_RETRANSMIT_MS` — a slower probe would itself
/// be a retransmit. Nothing answered has no maximum at all.
fn screen_normal_max(s: &LatencySample, answered: u32) -> Option<f64> {
    let v = s.normal_max_ms?;
    let slack = 1e-6 * (1.0 + s.rtt_max_ms);
    (answered > 0
        && v.is_finite()
        && v >= s.rtt_min_ms - slack
        && v <= s.rtt_max_ms + slack
        && v < LATENCY_RETRANSMIT_MS + slack)
        .then_some(v)
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
        let (retrans, retrans_sum_ms) = screen_retrans(s, answered, sum);
        let normal_max_ms = screen_normal_max(s, answered);
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
                retrans,
                retrans_sum_ms,
                normal_max_ms,
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
    /// Mean of every answered probe, slow handshakes included (`avg` leaves
    /// them out); equal to `avg` when the data does not say which were slow.
    pub raw_avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Slowest answered probe, slow handshakes included (`max` leaves them out).
    pub raw_max: Option<f64>,
    /// Percent of probes lost, 0 to 100.
    pub loss: f64,
    /// Percent of answered TCP probes whose connect took a second or more
    /// ("slow handshakes"), 0 to 100; null when none of the bucket's data
    /// carries the figure (an older node, ICMP).
    pub retrans_rate: Option<f64>,
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
    /// The newest minute's mean round trip (null when that minute lost every
    /// probe, or when the node never reported the target within the minute
    /// table's 48 hours) and when it started. The minute may be old: a node that
    /// stopped reporting keeps its last minute here, and the page decides from
    /// `latest_ts` whether that is still "current" (see LATENCY_STALE_SECS).
    pub latest: Option<f64>,
    pub latest_ts: Option<i64>,
    pub latest_loss: Option<f64>,
    /// The newest minute's share of slow handshakes (see `retrans_rate`).
    pub latest_retrans_rate: Option<f64>,
    /// The newest minute's mean with slow handshakes included.
    pub latest_raw_avg: Option<f64>,
    /// Mean round trip over the whole range, and percent lost over it. Null
    /// when the range holds no data / no answered probe.
    pub avg: Option<f64>,
    /// The range's mean with slow handshakes included.
    pub raw_avg: Option<f64>,
    pub loss: Option<f64>,
    /// Percent of answered TCP probes that were slow handshakes over the range;
    /// null when no row of the range carries the figure.
    pub retrans_rate: Option<f64>,
    /// The fastest and slowest answered probe of the range: the smallest of the
    /// buckets' minimums, and the largest of their slow-handshake-free maximums
    /// (see `bucket_max` / `range_max`). Null when no probe was answered.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// The slowest answered probe of the range, slow handshakes included.
    pub raw_max: Option<f64>,
}

fn loss_percent(sent: i64, lost: i64) -> f64 {
    if sent <= 0 {
        0.0
    } else {
        lost as f64 * 100.0 / sent as f64
    }
}

/// The retransmit figures of a stored period, as the repository returns them.
/// All None when no row of the period carried them. `base` is the answered
/// probes of the rows that did, which is what a rate divides by.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Retrans {
    count: Option<i64>,
    sum_ms: Option<f64>,
    base: Option<i64>,
}

impl Retrans {
    /// A single minute row: its base is simply its answered probes.
    fn of_row(r: &LatencyMinuteRow) -> Self {
        Self {
            count: r.retrans,
            sum_ms: r.retrans_sum_ms,
            base: r.retrans.map(|_| r.sent - r.lost),
        }
    }
}

/// The maximum a bucket shows: the slowest answered probe that was not a TCP
/// retransmit. Rows of a node that does not report retransmits (older node, ICMP
/// target) count with their plain maximum, which is what the repository's
/// `normal_max_ms` already folds in. When the bucket has no retransmit-free
/// maximum at all — every answered probe of every reporting row was a retransmit
/// — the plain maximum is shown instead, so a line whose every handshake took a
/// second or more reads as that, not as "no data". The minimum is untouched.
fn bucket_max(normal_max: Option<f64>, rtt_max: Option<f64>) -> Option<f64> {
    normal_max.or(rtt_max)
}

/// The maximum of a whole range: the largest retransmit-free maximum of its
/// buckets; only when no bucket has one, the largest plain maximum (the
/// fallback of `bucket_max` for the range as a whole). A bucket that was all
/// retransmits does not lift the range's maximum while other buckets have a
/// normal one.
#[derive(Default)]
struct RangeMax {
    normal: Option<f64>,
    plain: Option<f64>,
}

impl RangeMax {
    fn add(&mut self, normal_max: Option<f64>, rtt_max: Option<f64>) {
        let hi = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.normal = hi(self.normal, normal_max);
        self.plain = hi(self.plain, rtt_max);
    }

    fn get(&self) -> Option<f64> {
        self.normal.or(self.plain)
    }

    /// The largest answered round trip of the range, slow handshakes included.
    fn raw(&self) -> Option<f64> {
        self.plain
    }
}

/// The plain (raw) mean round trip of the answered probes, slow handshakes
/// included: what the API calls `raw_avg`.
fn plain_mean(sum: f64, answered: i64) -> Option<f64> {
    (answered > 0).then(|| sum / answered as f64)
}

/// The mean round trip of the answered probes, leaving out the handshakes that
/// needed a TCP retransmit where the data says which they were: the retransmits'
/// count and round trips come off both the total and the number of probes.
/// Rows that do not carry the figures (an older node, before the upgrade)
/// contribute their whole sum and count, i.e. the old behaviour.
///
/// When every answered probe was a retransmit nothing is left to average, and
/// the plain mean is returned instead of dividing by zero: a line whose every
/// handshake took a second or more should show that, not "no data".
fn mean(sum: f64, sent: i64, lost: i64, retrans: Retrans) -> Option<f64> {
    let answered = sent - lost;
    if let (Some(n), Some(rsum)) = (retrans.count, retrans.sum_ms) {
        let normal = answered - n;
        if n > 0 && normal > 0 {
            return Some(((sum - rsum) / normal as f64).max(0.0));
        }
    }
    plain_mean(sum, answered)
}

/// Percent of the answered probes (of the rows that report retransmits) that
/// were retransmits. Null when no row reports them, and also when the rows that
/// do had no answered probe to be a retransmit of.
fn retrans_rate(retrans: Retrans) -> Option<f64> {
    match (retrans.count, retrans.base) {
        (Some(n), Some(base)) if base > 0 => Some(n as f64 * 100.0 / base as f64),
        _ => None,
    }
}

/// Adds up the retransmit figures of several rows. A figure stays None until a
/// row carries it, so "nothing reported" survives the addition.
#[derive(Default)]
struct RetransTotal(Retrans);

impl RetransTotal {
    fn add(&mut self, r: Retrans) {
        fn plus<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) => Some(a + b),
                (a, None) => a,
                (None, b) => b,
            }
        }
        self.0.count = plus(self.0.count, r.count);
        self.0.sum_ms = plus(self.0.sum_ms, r.sum_ms);
        self.0.base = plus(self.0.base, r.base);
    }

    fn get(&self) -> Retrans {
        self.0
    }
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
        let mut total = RetransTotal::default();
        let mut min: Option<f64> = None;
        let mut max = RangeMax::default();
        let points: Vec<SeriesPoint> = mine
            .iter()
            .map(|b| {
                sent += b.sent;
                lost += b.lost;
                sum += b.rtt_sum_ms;
                let r = Retrans {
                    count: b.retrans,
                    sum_ms: b.retrans_sum_ms,
                    base: b.retrans_base,
                };
                total.add(r);
                min = match (min, b.rtt_min_ms) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                max.add(b.normal_max_ms, b.rtt_max_ms);
                SeriesPoint {
                    ts: b.ts,
                    avg: mean(b.rtt_sum_ms, b.sent, b.lost, r),
                    raw_avg: plain_mean(b.rtt_sum_ms, b.sent - b.lost),
                    min: b.rtt_min_ms,
                    max: bucket_max(b.normal_max_ms, b.rtt_max_ms),
                    raw_max: b.rtt_max_ms,
                    loss: loss_percent(b.sent, b.lost),
                    retrans_rate: retrans_rate(r),
                }
            })
            .collect();
        let total = total.get();
        let last = latest.iter().find(|r| r.target_id == t.id);
        summary.push(TargetSummary {
            target_id: t.id,
            latest: last.and_then(|r| mean(r.rtt_sum_ms, r.sent, r.lost, Retrans::of_row(r))),
            latest_ts: last.map(|r| r.minute_ts),
            latest_loss: last.map(|r| loss_percent(r.sent, r.lost)),
            latest_retrans_rate: last.and_then(|r| retrans_rate(Retrans::of_row(r))),
            latest_raw_avg: last.and_then(|r| plain_mean(r.rtt_sum_ms, r.sent - r.lost)),
            avg: mean(sum, sent, lost, total),
            raw_avg: plain_mean(sum, sent - lost),
            loss: (sent > 0).then(|| loss_percent(sent, lost)),
            retrans_rate: retrans_rate(total),
            min,
            max: max.get(),
            raw_max: max.raw(),
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

/// A node's stored status JSON and the group its record sits under, found by
/// node id alone (the key `node_status:<group>:<node>` carries both). When the
/// node has records under several groups the freshest report wins.
pub async fn find_node_status(
    db: &dyn Repository,
    node_id: &str,
) -> Result<Option<(i64, serde_json::Value)>, DbError> {
    let rows = db.scan_prefix("node_status:").await?;
    let mut best: Option<(String, i64, serde_json::Value)> = None;
    for (key, value) in rows {
        let Some(rest) = key.strip_prefix("node_status:") else {
            continue;
        };
        let Some((gid, nid)) = rest.split_once(':') else {
            continue;
        };
        let Ok(group_id) = gid.parse::<i64>() else {
            continue;
        };
        if nid != node_id {
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
        if best.as_ref().is_none_or(|(b, _, _)| seen > *b) {
            best = Some((seen, group_id, v));
        }
    }
    Ok(best.map(|(_, g, v)| (g, v)))
}

/// One node of a group as the comparison page lists it.
#[derive(Debug, Serialize, PartialEq)]
pub struct GroupNode {
    pub node_id: String,
    /// The admin's name for the node, when it has one.
    pub label: Option<String>,
    pub public_ip: Option<String>,
    pub node_version: Option<String>,
    /// Whether the node's version runs the monitor at all (node-v1.2.12+).
    pub supported: bool,
    /// The group's targets this node reported it cannot probe.
    pub unsupported_targets: Vec<i64>,
    pub last_seen: Option<String>,
}

/// Every node with a status record under `group_id` and its latency monitor on,
/// ordered by node id. The legacy per-group record (a node that never sent an id) has no series to
/// show and is left out.
pub async fn list_group_nodes(
    db: &dyn Repository,
    group_id: i64,
    targets: &[LatencyTarget],
) -> Result<Vec<GroupNode>, DbError> {
    let prefix = format!("node_status:{group_id}:");
    let labels = crate::service::node_labels::all_labels(db).await?;
    let off = all_off(db).await?;
    let mut nodes: Vec<GroupNode> = Vec::new();
    for (key, value) in db.scan_prefix(&prefix).await? {
        let Some(node_id) = key.strip_prefix(&prefix) else {
            continue;
        };
        if node_id.is_empty() {
            continue;
        }
        // A node switched off is left out entirely (its history stays stored).
        if off.contains(&format!("{group_id}:{node_id}")) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&value) else {
            continue;
        };
        let text = |name: &str| {
            v.get(name)
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        };
        let node_version = text("node_version");
        let unsupported_targets = v
            .get("latency_unsupported")
            .and_then(|x| x.as_array())
            .map(|ids| {
                ids.iter()
                    .filter_map(|i| i.as_i64())
                    .filter(|id| targets.iter().any(|t| t.id == *id))
                    .collect()
            })
            .unwrap_or_default();
        nodes.push(GroupNode {
            node_id: node_id.to_string(),
            label: labels.get(&format!("{group_id}:{node_id}")).cloned(),
            public_ip: text("public_ipv4").or_else(|| text("public_ip")),
            supported: relay_shared::protocol::node_supports_latency(node_version.as_deref()),
            node_version,
            unsupported_targets,
            last_seen: text("last_seen"),
        });
    }
    nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    Ok(nodes)
}

/// One cell of the comparison table: a node's figures for one target over the
/// range. Absent from the list when the node has no data for the target.
#[derive(Debug, Serialize, PartialEq)]
pub struct MatrixCell {
    pub node_id: String,
    pub target_id: i64,
    /// Mean round trip of the answered probes; null when all were lost.
    pub avg: Option<f64>,
    /// Percent of probes lost, 0 to 100.
    pub loss: f64,
    /// The range's mean with slow handshakes included.
    pub raw_avg: Option<f64>,
    /// Percent of answered TCP probes that were slow handshakes (connect took
    /// a second or more); null when no data of the range carries the figure.
    pub retrans_rate: Option<f64>,
    pub sent: i64,
}

pub fn build_matrix(totals: &[LatencyNodeTotal]) -> Vec<MatrixCell> {
    totals
        .iter()
        .filter(|t| t.sent > 0)
        .map(|t| {
            let r = Retrans {
                count: t.retrans,
                sum_ms: t.retrans_sum_ms,
                base: t.retrans_base,
            };
            MatrixCell {
                node_id: t.node_id.clone(),
                target_id: t.target_id,
                avg: mean(t.rtt_sum_ms, t.sent, t.lost, r),
                raw_avg: plain_mean(t.rtt_sum_ms, t.sent - t.lost),
                loss: loss_percent(t.sent, t.lost),
                retrans_rate: retrans_rate(r),
                sent: t.sent,
            }
        })
        .collect()
}

/// One node's line for the selected target.
#[derive(Debug, Serialize, PartialEq)]
pub struct NodeSeries {
    pub node_id: String,
    /// The newest minute (unix seconds) this node reported for the target, or
    /// null when it reported none within the minute table's retention. The real
    /// sample time of the node's "current" reading: the last bucket of an
    /// hourly line says only which hour it fell in.
    pub latest_ts: Option<i64>,
    pub points: Vec<SeriesPoint>,
}

/// One line per node (a node with no data still gets an empty one), points
/// oldest first.
pub fn build_group_series(
    nodes: &[GroupNode],
    buckets: &[LatencyNodeBucket],
    latest: &[(String, i64)],
) -> Vec<NodeSeries> {
    nodes
        .iter()
        .map(|n| NodeSeries {
            node_id: n.node_id.clone(),
            latest_ts: latest
                .iter()
                .find(|(id, _)| *id == n.node_id)
                .map(|(_, ts)| *ts),
            points: buckets
                .iter()
                .filter(|b| b.node_id == n.node_id)
                .map(|b| {
                    let r = Retrans {
                        count: b.retrans,
                        sum_ms: b.retrans_sum_ms,
                        base: b.retrans_base,
                    };
                    SeriesPoint {
                        ts: b.ts,
                        avg: mean(b.rtt_sum_ms, b.sent, b.lost, r),
                        raw_avg: plain_mean(b.rtt_sum_ms, b.sent - b.lost),
                        min: b.rtt_min_ms,
                        max: bucket_max(b.normal_max_ms, b.rtt_max_ms),
                        raw_max: b.rtt_max_ms,
                        loss: loss_percent(b.sent, b.lost),
                        retrans_rate: retrans_rate(r),
                    }
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: i64) -> LatencyTarget {
        LatencyTarget {
            id,
            group_id: 1,
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
            retrans: None,
            retrans_sum_ms: None,
            normal_max_ms: None,
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
        let r = screen_samples(&targets, std::slice::from_ref(&good), NOW);
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
                retrans: None,
                retrans_sum_ms: None,
                normal_max_ms: None,
                retrans_base: None,
            },
            LatencyBucket {
                target_id: 1,
                ts: 660,
                sent: 6,
                lost: 6,
                rtt_sum_ms: 0.0,
                rtt_min_ms: None,
                rtt_max_ms: None,
                retrans: None,
                retrans_sum_ms: None,
                normal_max_ms: None,
                retrans_base: None,
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
            retrans: None,
            retrans_sum_ms: None,
            normal_max_ms: None,
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
        assert_eq!((summary[1].min, summary[1].max), (None, None));
        // the all-lost bucket has no extremes, the answered one does
        assert_eq!((summary[0].min, summary[0].max), (Some(8.0), Some(14.0)));
    }

    fn rt_sample(retrans: Option<u32>, retrans_sum_ms: Option<f64>) -> LatencySample {
        // 6 sent, 1 lost, 5 answered: 4 normal ones and (maybe) one retransmit.
        LatencySample {
            target_id: 1,
            minute_ts: NOW - 120,
            sent: 6,
            lost: 1,
            rtt_sum_ms: 4.0 * 20.0 + 1100.0,
            rtt_min_ms: 18.0,
            rtt_max_ms: 1100.0,
            retrans,
            retrans_sum_ms,
            normal_max_ms: None,
            unsupported: false,
        }
    }

    #[test]
    fn screening_stores_consistent_retransmit_figures() {
        let t = [target(1)];
        let stored = |s: LatencySample| {
            let r = screen_samples(&t, &[s], NOW);
            assert_eq!(r.accepted.len(), 1, "the sample itself is always kept");
            (r.accepted[0].retrans, r.accepted[0].retrans_sum_ms)
        };
        assert_eq!(
            stored(rt_sample(Some(1), Some(1100.0))),
            (Some(1), Some(1100.0))
        );
        // a node that reports none, or zero
        assert_eq!(stored(rt_sample(None, None)), (None, None));
        let mut none = rt_sample(Some(0), Some(0.0));
        none.rtt_sum_ms = 100.0;
        none.rtt_max_ms = 22.0;
        assert_eq!(stored(none), (Some(0), Some(0.0)));
        // every answered probe a retransmit
        let mut all = rt_sample(Some(5), Some(5500.0));
        all.rtt_sum_ms = 5500.0;
        assert_eq!(stored(all), (Some(5), Some(5500.0)));
    }

    #[test]
    fn screening_drops_only_the_retransmit_fields_when_they_are_inconsistent() {
        let t = [target(1)];
        let bad: Vec<(&str, LatencySample)> = vec![
            ("more than answered", rt_sample(Some(6), Some(6600.0))),
            ("NaN sum", rt_sample(Some(1), Some(f64::NAN))),
            ("infinite sum", rt_sample(Some(1), Some(f64::INFINITY))),
            ("negative sum", rt_sample(Some(1), Some(-1100.0))),
            (
                "sum above the round trips",
                rt_sample(Some(1), Some(1500.0)),
            ),
            ("under a second each", rt_sample(Some(1), Some(900.0))),
            (
                "two of them need two seconds",
                rt_sample(Some(2), Some(1100.0)),
            ),
            ("none but a sum", rt_sample(Some(0), Some(50.0))),
            ("count without sum", rt_sample(Some(1), None)),
            ("sum without count", rt_sample(None, Some(1100.0))),
        ];
        for (why, s) in bad {
            let r = screen_samples(&t, &[s], NOW);
            assert_eq!(r.accepted.len(), 1, "{why}: the sample is kept");
            assert_eq!(r.accepted[0].rtt_sum_ms, 4.0 * 20.0 + 1100.0, "{why}");
            assert_eq!(
                (r.accepted[0].retrans, r.accepted[0].retrans_sum_ms),
                (None, None),
                "{why}: stored as not reported"
            );
        }
    }

    #[test]
    fn screening_keeps_a_consistent_normal_max_and_drops_only_it_otherwise() {
        let t = [target(1)];
        // 5 answered, min 18, max 1100, one retransmit: a normal max of 22 fits
        let with = |v: Option<f64>| LatencySample {
            normal_max_ms: v,
            ..rt_sample(Some(1), Some(1100.0))
        };
        let stored = |s: LatencySample| {
            let r = screen_samples(&t, &[s], NOW);
            assert_eq!(r.accepted.len(), 1, "the sample itself is always kept");
            // the retransmit figures are not collateral damage either
            assert_eq!(r.accepted[0].retrans, Some(1));
            r.accepted[0].normal_max_ms
        };
        assert_eq!(stored(with(Some(22.0))), Some(22.0));
        assert_eq!(stored(with(None)), None);
        // exactly the minimum and just under a second are fine
        assert_eq!(stored(with(Some(18.0))), Some(18.0));
        assert_eq!(stored(with(Some(999.0))), Some(999.0));
        for (why, v) in [
            ("NaN", f64::NAN),
            ("infinite", f64::INFINITY),
            ("below the minimum", 10.0),
            ("negative", -1.0),
            ("a second or more is a retransmit", 1000.5),
        ] {
            assert_eq!(stored(with(Some(v))), None, "{why}");
        }
        // above rtt_max (a max of 800 cannot hold a larger normal probe)
        let low_max = LatencySample {
            rtt_max_ms: 800.0,
            rtt_sum_ms: 4.0 * 20.0 + 800.0,
            normal_max_ms: Some(900.0),
            retrans: None,
            retrans_sum_ms: None,
            ..rt_sample(None, None)
        };
        let r = screen_samples(&t, &[low_max], NOW);
        assert_eq!(r.accepted[0].normal_max_ms, None, "above rtt_max");
        // nothing answered has no maximum at all
        let lost = LatencySample {
            sent: 6,
            lost: 6,
            rtt_sum_ms: 0.0,
            rtt_min_ms: 0.0,
            rtt_max_ms: 0.0,
            retrans: None,
            retrans_sum_ms: None,
            normal_max_ms: Some(0.0),
            ..rt_sample(None, None)
        };
        let r = screen_samples(&t, &[lost], NOW);
        assert_eq!(r.accepted[0].normal_max_ms, None);
    }

    #[test]
    fn the_shown_maximum_leaves_out_retransmits_with_the_fallbacks_of_the_card() {
        // a bucket with a retransmit-free maximum shows it, not the 1100 ms probe
        assert_eq!(bucket_max(Some(22.0), Some(1100.0)), Some(22.0));
        // none known (older node, ICMP, or every answer a retransmit): plain max
        assert_eq!(bucket_max(None, Some(1100.0)), Some(1100.0));
        assert_eq!(bucket_max(None, None), None);
        // a range prefers retransmit-free maximums over the plain one ...
        let mut m = RangeMax::default();
        m.add(Some(22.0), Some(1100.0));
        m.add(None, Some(1300.0)); // an all-retransmit bucket does not lift it
        m.add(Some(30.0), Some(40.0));
        assert_eq!(m.get(), Some(30.0));
        // ... and falls back to the plain one only when there is none at all
        let mut all = RangeMax::default();
        all.add(None, Some(1100.0));
        all.add(None, Some(1300.0));
        assert_eq!(all.get(), Some(1300.0));
        assert_eq!(RangeMax::default().get(), None);
    }

    #[test]
    fn series_and_summary_show_the_retransmit_free_maximum() {
        let t = [target(1)];
        let bucket = |ts, nm| LatencyBucket {
            target_id: 1,
            ts,
            sent: 6,
            lost: 0,
            rtt_sum_ms: 1220.0,
            rtt_min_ms: Some(18.0),
            rtt_max_ms: Some(1100.0),
            normal_max_ms: nm,
            retrans: Some(1),
            retrans_sum_ms: Some(1100.0),
            retrans_base: Some(6),
        };
        let (series, summary) =
            build_series(&t, &[bucket(600, Some(22.0)), bucket(660, None)], &[]);
        assert_eq!(series[0].points[0].max, Some(22.0));
        assert_eq!(
            series[0].points[1].max,
            Some(1100.0),
            "every answer a retransmit"
        );
        assert_eq!(summary[0].max, Some(22.0));
        assert_eq!(summary[0].min, Some(18.0), "min is untouched");
        let (_, only_retrans) = build_series(&t, &[bucket(600, None)], &[]);
        assert_eq!(only_retrans[0].max, Some(1100.0));
    }

    #[test]
    fn a_fully_lost_minute_keeps_a_reported_zero_and_drops_anything_else() {
        let t = [target(1)];
        let lost = |retrans, sum| LatencySample {
            target_id: 1,
            minute_ts: NOW - 60,
            sent: 6,
            lost: 6,
            rtt_sum_ms: 0.0,
            rtt_min_ms: 0.0,
            rtt_max_ms: 0.0,
            retrans,
            retrans_sum_ms: sum,
            normal_max_ms: None,
            unsupported: false,
        };
        let r = screen_samples(&t, &[lost(Some(0), Some(0.0))], NOW);
        assert_eq!(
            (r.accepted[0].retrans, r.accepted[0].retrans_sum_ms),
            (Some(0), Some(0.0)),
            "zero is stored as 0, not NULL"
        );
        let r = screen_samples(&t, &[lost(Some(1), Some(1100.0))], NOW);
        assert_eq!(r.accepted.len(), 1);
        assert_eq!(
            (r.accepted[0].retrans, r.accepted[0].retrans_sum_ms),
            (None, None)
        );
    }

    fn r(count: Option<i64>, sum_ms: Option<f64>, base: Option<i64>) -> Retrans {
        Retrans {
            count,
            sum_ms,
            base,
        }
    }

    #[test]
    fn the_average_leaves_out_retransmits_where_they_are_known() {
        // 10 answered, 2 of them retransmits of 1100 ms; 8 normal ones of 20 ms
        let sum = 8.0 * 20.0 + 2.0 * 1100.0;
        let known = r(Some(2), Some(2200.0), Some(10));
        assert_eq!(mean(sum, 12, 2, known), Some(20.0));
        assert_eq!(retrans_rate(known), Some(20.0));
        // no retransmits reported: the plain mean, no rate
        assert_eq!(
            mean(sum, 12, 2, Retrans::default()),
            Some(sum / 10.0),
            "old data keeps the old average"
        );
        assert_eq!(retrans_rate(Retrans::default()), None);
        // reported as zero: same as the plain mean, and a real 0 %
        let zero = r(Some(0), Some(0.0), Some(10));
        assert_eq!(mean(200.0, 10, 0, zero), Some(20.0));
        assert_eq!(retrans_rate(zero), Some(0.0));
        // every answered probe was a retransmit: the plain mean, not 0 / 0
        let all = r(Some(4), Some(4400.0), Some(4));
        assert_eq!(mean(4400.0, 4, 0, all), Some(1100.0));
        assert_eq!(retrans_rate(all), Some(100.0));
        // nothing answered: no mean and no rate, with or without the figures
        let none = r(Some(0), Some(0.0), Some(0));
        assert_eq!(mean(0.0, 6, 6, none), None);
        assert_eq!(retrans_rate(none), None);
        assert_eq!(mean(0.0, 0, 0, Retrans::default()), None);
        // a mixed period: 6 answered probes from a reporting node (1 retransmit
        // of 1100 ms) plus 4 from an old one (40 ms each); the rate only counts
        // the 6 that could say.
        let mixed = r(Some(1), Some(1100.0), Some(6));
        let mixed_sum = 5.0 * 20.0 + 1100.0 + 4.0 * 40.0;
        assert_eq!(retrans_rate(mixed), Some(100.0 / 6.0));
        let avg = mean(mixed_sum, 10, 0, mixed).unwrap();
        assert!((avg - (5.0 * 20.0 + 4.0 * 40.0) / 9.0).abs() < 1e-9);
        // float noise never makes the average negative
        assert_eq!(
            mean(1100.0, 3, 1, r(Some(1), Some(1100.0000001), Some(2))),
            Some(0.0)
        );
    }

    #[test]
    fn totals_stay_unreported_until_a_row_reports_and_add_up_after() {
        let mut t = RetransTotal::default();
        assert_eq!(t.get(), Retrans::default());
        t.add(Retrans::default());
        assert_eq!(t.get(), Retrans::default(), "an old row adds nothing");
        t.add(r(Some(1), Some(1100.0), Some(6)));
        t.add(Retrans::default());
        t.add(r(Some(0), Some(0.0), Some(6)));
        assert_eq!(t.get(), r(Some(1), Some(1100.0), Some(12)));
    }

    #[test]
    fn series_carry_the_retransmit_rate_and_a_retransmit_free_average() {
        let t = [target(1)];
        let bucket = |ts, sent, lost, sum, retrans, rsum, base| LatencyBucket {
            target_id: 1,
            ts,
            sent,
            lost,
            rtt_sum_ms: sum,
            rtt_min_ms: Some(18.0),
            rtt_max_ms: Some(1100.0),
            retrans,
            retrans_sum_ms: rsum,
            normal_max_ms: None,
            retrans_base: base,
        };
        let buckets = vec![
            // 5 answered, one a retransmit
            bucket(600, 6, 1, 1180.0, Some(1), Some(1100.0), Some(5)),
            // an old row
            bucket(660, 6, 0, 120.0, None, None, None),
        ];
        let latest = vec![LatencyMinuteRow {
            target_id: 1,
            minute_ts: 660,
            sent: 6,
            lost: 1,
            rtt_sum_ms: 1180.0,
            rtt_min_ms: Some(18.0),
            rtt_max_ms: Some(1100.0),
            retrans: Some(1),
            retrans_sum_ms: Some(1100.0),
            normal_max_ms: None,
        }];
        let (series, summary) = build_series(&t, &buckets, &latest);
        let p = &series[0].points;
        assert_eq!(p[0].avg, Some(20.0));
        assert_eq!(p[0].retrans_rate, Some(20.0));
        assert_eq!(p[1].avg, Some(20.0), "no figures: the plain mean");
        assert_eq!(p[1].retrans_rate, None, "not reported is not 0 %");
        assert_eq!(summary[0].latest, Some(20.0));
        assert_eq!(summary[0].latest_retrans_rate, Some(20.0));
        // the range's fastest and slowest answered probe, retransmit included
        assert_eq!((summary[0].min, summary[0].max), (Some(18.0), Some(1100.0)));
        // range: (1300 - 1100) / (11 - 1) = 20, rate over the 5 that could say
        assert_eq!(summary[0].avg, Some(20.0));
        assert_eq!(summary[0].retrans_rate, Some(20.0));
        // loss is untouched by all of this
        assert_eq!(summary[0].loss.map(|l| (l * 10.0).round()), Some(83.0));
    }

    #[test]
    fn the_matrix_cell_has_the_retransmit_free_average_and_the_rate() {
        let total = |node: &str, retrans, rsum, base| LatencyNodeTotal {
            node_id: node.into(),
            target_id: 1,
            sent: 12,
            lost: 2,
            rtt_sum_ms: 2360.0,
            retrans,
            retrans_sum_ms: rsum,
            retrans_base: base,
        };
        let cells = build_matrix(&[
            total("new", Some(2), Some(2200.0), Some(10)),
            total("old", None, None, None),
        ]);
        assert_eq!(cells[0].avg, Some(20.0));
        assert_eq!(cells[0].retrans_rate, Some(20.0));
        assert_eq!(cells[1].avg, Some(236.0), "an old node keeps the old mean");
        assert_eq!(cells[1].retrans_rate, None);
        let json = serde_json::to_value(&cells[1]).unwrap();
        assert!(json["retrans_rate"].is_null(), "serialised as null");
    }

    /// One probe of 20 ms and nine slow handshakes of 1100 ms, all answered:
    /// the filtered figures read 20 ms, the raw ones tell the truth (992 ms /
    /// 1100 ms), and 90 % of the probes were slow.
    #[test]
    fn raw_figures_keep_the_slow_handshakes_the_filtered_ones_leave_out() {
        let t = [target(1)];
        // (retrans, retrans_sum, normal_max, retrans_base)
        let bucket =
            |sum, max, extra: (Option<i64>, Option<f64>, Option<f64>, Option<i64>)| LatencyBucket {
                target_id: 1,
                ts: 600,
                sent: 10,
                lost: 0,
                rtt_sum_ms: sum,
                rtt_min_ms: Some(20.0),
                rtt_max_ms: Some(max),
                retrans: extra.0,
                retrans_sum_ms: extra.1,
                normal_max_ms: extra.2,
                retrans_base: extra.3,
            };
        let minute = |sum, max, retrans, rsum, normal_max| LatencyMinuteRow {
            target_id: 1,
            minute_ts: 600,
            sent: 10,
            lost: 0,
            rtt_sum_ms: sum,
            rtt_min_ms: Some(20.0),
            rtt_max_ms: Some(max),
            retrans,
            retrans_sum_ms: rsum,
            normal_max_ms: normal_max,
        };

        // the example: 20 + 9 * 1100 = 9920
        let buckets = [bucket(
            9920.0,
            1100.0,
            (Some(9), Some(9900.0), Some(20.0), Some(10)),
        )];
        let latest = [minute(9920.0, 1100.0, Some(9), Some(9900.0), Some(20.0))];
        let (series, summary) = build_series(&t, &buckets, &latest);
        let p = &series[0].points[0];
        assert_eq!((p.avg, p.max), (Some(20.0), Some(20.0)));
        assert_eq!((p.raw_avg, p.raw_max), (Some(992.0), Some(1100.0)));
        assert_eq!(p.retrans_rate, Some(90.0));
        let s = &summary[0];
        assert_eq!((s.avg, s.max), (Some(20.0), Some(20.0)));
        assert_eq!((s.raw_avg, s.raw_max), (Some(992.0), Some(1100.0)));
        assert_eq!((s.latest, s.latest_raw_avg), (Some(20.0), Some(992.0)));
        assert_eq!(s.latest_retrans_rate, Some(90.0));

        // matrix cell and group line
        let cells = build_matrix(&[LatencyNodeTotal {
            node_id: "n1".into(),
            target_id: 1,
            sent: 10,
            lost: 0,
            rtt_sum_ms: 9920.0,
            retrans: Some(9),
            retrans_sum_ms: Some(9900.0),
            retrans_base: Some(10),
        }]);
        assert_eq!((cells[0].avg, cells[0].raw_avg), (Some(20.0), Some(992.0)));
        assert_eq!(cells[0].retrans_rate, Some(90.0));
        let nodes = [GroupNode {
            node_id: "n1".into(),
            label: None,
            public_ip: None,
            node_version: None,
            supported: true,
            unsupported_targets: vec![],
            last_seen: None,
        }];
        let lines = build_group_series(
            &nodes,
            &[LatencyNodeBucket {
                node_id: "n1".into(),
                ts: 600,
                sent: 10,
                lost: 0,
                rtt_sum_ms: 9920.0,
                rtt_min_ms: Some(20.0),
                rtt_max_ms: Some(1100.0),
                normal_max_ms: Some(20.0),
                retrans: Some(9),
                retrans_sum_ms: Some(9900.0),
                retrans_base: Some(10),
            }],
            &[("n1".to_string(), 660)],
        );
        assert_eq!(lines[0].latest_ts, Some(660));
        let g = &lines[0].points[0];
        assert_eq!((g.avg, g.max), (Some(20.0), Some(20.0)));
        assert_eq!((g.raw_avg, g.raw_max), (Some(992.0), Some(1100.0)));

        // rows without retransmit data: raw equals filtered
        let buckets = [bucket(120.0, 40.0, (None, None, None, None))];
        let latest = [minute(120.0, 40.0, None, None, None)];
        let (series, summary) = build_series(&t, &buckets, &latest);
        let p = &series[0].points[0];
        assert_eq!((p.raw_avg, p.raw_max), (p.avg, p.max));
        assert_eq!((p.raw_avg, p.raw_max), (Some(12.0), Some(40.0)));
        assert_eq!(p.retrans_rate, None);
        let s = &summary[0];
        assert_eq!((s.raw_avg, s.raw_max), (s.avg, s.max));
        assert_eq!(s.latest_raw_avg, s.latest);

        // every answered probe slow: the filtered mean already falls back to
        // the plain one, and so does the maximum
        let buckets = [bucket(
            11000.0,
            1100.0,
            (Some(10), Some(11000.0), None, Some(10)),
        )];
        let latest = [minute(11000.0, 1100.0, Some(10), Some(11000.0), None)];
        let (series, summary) = build_series(&t, &buckets, &latest);
        let p = &series[0].points[0];
        assert_eq!((p.avg, p.max), (Some(1100.0), Some(1100.0)));
        assert_eq!((p.raw_avg, p.raw_max), (Some(1100.0), Some(1100.0)));
        assert_eq!(p.retrans_rate, Some(100.0));
        assert_eq!(summary[0].raw_avg, summary[0].avg);
        assert_eq!(summary[0].latest_raw_avg, Some(1100.0));

        // a fully lost minute has no raw figure either
        let buckets = [LatencyBucket {
            lost: 10,
            rtt_sum_ms: 0.0,
            rtt_min_ms: None,
            rtt_max_ms: None,
            ..bucket(0.0, 0.0, (None, None, None, None))
        }];
        let (series, _) = build_series(&t, &buckets, &[]);
        let p = &series[0].points[0];
        assert_eq!((p.raw_avg, p.raw_max), (None, None));
        let json = serde_json::to_value(p).unwrap();
        assert!(json["raw_avg"].is_null() && json["raw_max"].is_null());
    }

    #[test]
    fn alignment_floors_to_the_bucket() {
        assert_eq!(align_down(1_000, 300), 900);
        assert_eq!(align_down(900, 300), 900);
        assert_eq!(align_down(7_199, 3600), 3600);
    }
}
