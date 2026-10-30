//! Admin-given names for the individual machines of a device group.
//!
//! A group with several nodes lists them by public IP and a random `node_id`,
//! which says little about which box is which. A label ("HK-1 backup", the
//! provider, a rack) is kept in the kvs table under `node_label:{group_id}:
//! {node_id}` — the same `{group_id}:{node_id}` suffix as the node's
//! `node_status:` row, but a separate key: the status row is rewritten by every
//! report, so a label stored inside it would be overwritten within seconds.
//!
//! Labels are an admin's own notes. Whether regular users see them is a site
//! setting (`SiteConfig::show_node_labels`, off by default), applied wherever a
//! node is described to someone who is not an admin.

use crate::api::AppState;
use crate::db::error::DbError;
use crate::db::repo::KvsRepository;
use crate::service::site::{SiteConfig, SITE_CONFIG_KEY};
use std::collections::HashMap;

pub const LABEL_PREFIX: &str = "node_label:";

/// Longest label, in characters (not bytes: labels are expected to be CJK).
pub const MAX_LABEL_CHARS: usize = 40;

/// The kvs key of one node's label.
pub fn label_key(group_id: i64, node_id: &str) -> String {
    format!("{LABEL_PREFIX}{group_id}:{node_id}")
}

/// Why a label was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum LabelError {
    TooLong,
    /// A control character (newline, tab, escape...) would let one label span
    /// several lines of an alert, an audit entry or a table cell.
    ControlChars,
}

/// Trim and check a label. `Ok(None)` means "no label": an empty or blank value
/// removes it.
pub fn normalize_label(raw: &str) -> Result<Option<String>, LabelError> {
    let label = raw.trim();
    if label.is_empty() {
        return Ok(None);
    }
    if label.chars().any(char::is_control) {
        return Err(LabelError::ControlChars);
    }
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(LabelError::TooLong);
    }
    Ok(Some(label.to_string()))
}

/// Every stored label, keyed `{group_id}:{node_id}` — the part after
/// `node_status:` in a status key, so a caller walking status rows looks each
/// one up directly.
pub async fn all_labels<R: KvsRepository + ?Sized>(
    db: &R,
) -> Result<HashMap<String, String>, DbError> {
    Ok(db
        .scan_prefix(LABEL_PREFIX)
        .await?
        .into_iter()
        .filter_map(|(k, v)| {
            let node_key = k.strip_prefix(LABEL_PREFIX)?.to_string();
            let v = v.trim().to_string();
            (!v.is_empty()).then_some((node_key, v))
        })
        .collect())
}

/// The labels the caller may see: all of them for an admin; for anyone else only
/// when the site setting lets users see them, otherwise none. Every place that
/// describes a node to a signed-in person goes through here, so the switch is
/// applied in one spot.
pub async fn visible_labels(state: &AppState, is_admin: bool) -> HashMap<String, String> {
    if !is_admin {
        let raw = state.db.get(SITE_CONFIG_KEY).await.ok().flatten();
        if !SiteConfig::from_json(raw.as_deref()).show_node_labels {
            return HashMap::new();
        }
    }
    match all_labels(state.db.as_ref()).await {
        Ok(labels) => labels,
        Err(e) => {
            tracing::warn!("node labels: read failed, showing none: {}", e);
            HashMap::new()
        }
    }
}

/// The labels of one group's nodes the caller may see, by node id — what a
/// diagnose or restart response carries so the page can name each node.
pub async fn visible_labels_of_group(
    state: &AppState,
    is_admin: bool,
    group_id: i64,
) -> HashMap<String, String> {
    let prefix = format!("{group_id}:");
    visible_labels(state, is_admin)
        .await
        .into_iter()
        .filter_map(|(k, v)| Some((k.strip_prefix(&prefix)?.to_string(), v)))
        .collect()
}

/// One node's label, if it has one.
pub async fn get_label<R: KvsRepository + ?Sized>(
    db: &R,
    group_id: i64,
    node_id: &str,
) -> Option<String> {
    db.get(&label_key(group_id, node_id))
        .await
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `节点「label」· ` for an audit line about a node, or nothing when it has no
/// label. The audit log records the node by its id; the label says which box
/// that was.
pub async fn audit_prefix<R: KvsRepository + ?Sized>(
    db: &R,
    group_id: i64,
    node_id: &str,
) -> String {
    match get_label(db, group_id, node_id).await {
        Some(l) => format!("节点「{l}」· "),
        None => String::new(),
    }
}

/// Store a label, or remove it when `label` is `None`.
pub async fn set_label<R: KvsRepository + ?Sized>(
    db: &R,
    group_id: i64,
    node_id: &str,
    label: Option<&str>,
) -> Result<(), DbError> {
    let key = label_key(group_id, node_id);
    match label {
        Some(l) => db.set(&key, l).await,
        None => db.delete(&key).await.map(|_| ()),
    }
}

/// How a node is named in text meant for a person: its label when it has one,
/// otherwise the given fallback (an id, an address).
pub fn display_name(label: Option<&str>, fallback: &str) -> String {
    match label {
        Some(l) if !l.is_empty() => l.to_string(),
        _ => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_means_no_label() {
        for raw in ["", "   ", "\u{3000}"] {
            assert_eq!(normalize_label(raw), Ok(None), "{raw:?}");
        }
    }

    #[test]
    fn a_label_is_trimmed_and_may_be_cjk_or_mixed() {
        assert_eq!(
            normalize_label("  香港 HK-1 (备用)  "),
            Ok(Some("香港 HK-1 (备用)".to_string()))
        );
    }

    #[test]
    fn the_cap_counts_characters_not_bytes() {
        let ok = "节".repeat(MAX_LABEL_CHARS);
        assert_eq!(normalize_label(&ok), Ok(Some(ok.clone())));
        assert_eq!(
            normalize_label(&"节".repeat(MAX_LABEL_CHARS + 1)),
            Err(LabelError::TooLong)
        );
    }

    #[test]
    fn control_characters_are_refused() {
        for raw in ["a\nb", "a\tb", "a\u{1b}[31mb", "a\u{0}b", "a\rb"] {
            assert_eq!(
                normalize_label(raw),
                Err(LabelError::ControlChars),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn keys_share_the_status_suffix() {
        assert_eq!(label_key(11, "abc"), "node_label:11:abc");
    }

    #[test]
    fn display_name_prefers_the_label() {
        assert_eq!(display_name(Some("HK-1"), "11:abc"), "HK-1");
        assert_eq!(display_name(Some(""), "11:abc"), "11:abc");
        assert_eq!(display_name(None, "11:abc"), "11:abc");
    }
}
