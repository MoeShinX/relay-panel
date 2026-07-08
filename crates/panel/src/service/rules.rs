//! Rule + shared rule/group/profile validation service.
//!
//! Houses the pure, DB-agnostic validators shared by the rule, group and
//! profile handlers (protocol/transport rules, target normalization, port
//! auto-assignment) plus the `create_rule` / `update_rule` business flows.
//! Extracted from `api/admin` so the validation lives behind the `Repository`
//! trait and is unit-testable without the HTTP layer.

use crate::db::error::DbError;
use crate::db::repo::{GroupRepository, ProfileScope, Repository, ResourceScope};
use relay_shared::protocol::{
    CreateRuleRequest, GroupType, Protocol, PublicTransport, RuleTargetRequest, UpdateRuleRequest,
};

/// v0.4.20: forward_mode is locked to "direct" at the API boundary
/// (create_rule / update_rule reject group/chain). This validator is retained
/// for potential future re-enablement and for config-generation compatibility.
#[allow(dead_code)]
pub fn validate_forward_mode(mode: &str) -> bool {
    matches!(mode, "group" | "direct")
}

/// Is `transport` accepted by the admin API in the current release?
///
/// v0.4.1: `Raw` + `Ws` + `TlsSimple` (node terminates TLS via rustls).
/// `Wss` is deprecated — existing wss rules are migrated to ws by Migration 18,
/// and the admin API no longer accepts creating new wss rules.
///
/// Single source of truth for "what public_transport values may a rule store" —
/// both create_rule and update_rule call this so they can't drift.
pub fn is_public_transport_accepted(transport: PublicTransport) -> bool {
    matches!(
        transport,
        PublicTransport::Raw | PublicTransport::Ws | PublicTransport::TlsSimple
    )
}

/// Validate the protocol × public_transport combination for v0.4.0.
///
/// Two symmetric constraints (a rule must satisfy BOTH):
///   (a) any UDP-bearing protocol (udp OR tcp_udp) ⇒ transport must be Raw
///       (WS/WSS are TCP-only).
///   (b) WS/WSS transport ⇒ protocol must be TCP (WS carries TCP only).
///
/// Pure function (no DB) so create_rule and update_rule can both resolve their
/// EFFECTIVE protocol/transport strings and call this. Returns Some(error_msg)
/// when the combination is invalid.
///
/// `protocol` / `transport` are the stable DB strings ("tcp"|"udp"|"tcp_udp" and
/// "raw"|"ws"|"wss"|"tls_simple"). Unknown values are not rejected here —
/// they're handled by their own field validation.
pub fn validate_protocol_transport(protocol: &str, transport: &str) -> Option<&'static str> {
    // WS and TLS Simple are TCP-only transports.
    if (transport == "ws" || transport == "tls_simple") && protocol != "tcp" {
        return Some(
            "This transport (ws/tls_simple) currently carries TCP forwarding only; \
             UDP / TCP+UDP are not supported.",
        );
    }
    // any UDP-bearing protocol (udp OR tcp_udp) ⇒ transport must be Raw.
    let is_udp_bearing = matches!(protocol, "udp" | "tcp_udp");
    if is_udp_bearing && transport != "raw" {
        return Some("UDP rules only support 'raw' transport");
    }
    None
}

/// Map Protocol enum to stable DB string.
pub fn protocol_to_str(p: &Protocol) -> &'static str {
    match p {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
        Protocol::TcpUdp => "tcp_udp",
    }
}

pub fn is_plausible_target_host(host: &str) -> bool {
    let h = host.trim();
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    if h.contains("://") || h.contains('/') || h.chars().any(char::is_whitespace) {
        return false;
    }
    h.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

pub fn normalize_rule_targets(
    targets: Option<Vec<RuleTargetRequest>>,
    legacy_host: &str,
    legacy_port: u16,
) -> Result<Vec<RuleTargetRequest>, &'static str> {
    let mut out = targets.unwrap_or_else(|| {
        vec![RuleTargetRequest {
            host: legacy_host.to_string(),
            port: legacy_port,
            enabled: true,
        }]
    });
    if out.is_empty() {
        return Err("At least one target is required");
    }
    if out.len() > 32 {
        return Err("A rule can have at most 32 targets");
    }
    let mut enabled = 0usize;
    for target in &mut out {
        target.host = target.host.trim().to_string();
        if !is_plausible_target_host(&target.host) {
            return Err("Target host must be an IP address or domain without scheme/path/spaces");
        }
        if target.port == 0 {
            return Err("Target port must be between 1 and 65535");
        }
        if target.enabled {
            enabled += 1;
        }
    }
    if enabled == 0 {
        return Err("At least one target must be enabled");
    }
    Ok(out)
}

/// Map GroupType enum to stable DB string.
pub fn group_type_to_str(gt: &GroupType) -> &'static str {
    match gt {
        GroupType::In => "in",
        GroupType::Out => "out",
        GroupType::Monitor => "monitor",
    }
}

/// Auto-assign a free listen port from 10000-65535, scoped to the rule's
/// inbound group and socket type.
///
/// v0.4.11 PR4: port occupancy is per (device_group_in, port, socket type).
/// We only need to avoid ports already used ON THIS GROUP that conflict with
/// the candidate's socket type: a TCP-bearing candidate (tcp / tcp_udp) avoids
/// this group's tcp / tcp_udp ports, and a UDP-bearing candidate (udp /
/// tcp_udp) avoids its udp / tcp_udp ports. A pure-TCP candidate may reuse a
/// port held by a pure-UDP rule, and vice versa. Different groups have
/// independent pools.
pub async fn auto_assign_port(
    db: &dyn Repository,
    device_group_in: i64,
    protocol: &str,
) -> Result<u16, String> {
    let needs_tcp = matches!(protocol, "tcp" | "tcp_udp");
    let needs_udp = matches!(protocol, "udp" | "tcp_udp");

    // (port, protocol) pairs already in use on this group.
    let group_ports: Vec<(i32, String)> = db
        .list_group_port_protocols(device_group_in)
        .await
        .map_err(|e| e.to_string())?;

    // Build the occupied set: only ports whose socket type overlaps the
    // candidate's.
    let used: std::collections::HashSet<u16> = group_ports
        .into_iter()
        .filter_map(|(p, proto)| {
            let occupies_tcp = matches!(proto.as_str(), "tcp" | "tcp_udp");
            let occupies_udp = matches!(proto.as_str(), "udp" | "tcp_udp");
            let conflicts = (needs_tcp && occupies_tcp) || (needs_udp && occupies_udp);
            if conflicts {
                u16::try_from(p).ok()
            } else {
                None
            }
        })
        .collect();

    // Try pseudo-random ports in the 10000-65535 range
    let mut rng = 10000u16;
    for _ in 0..1000 {
        let candidate = rng.wrapping_add(7919).wrapping_rem(55535) + 10000;
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
        rng = candidate;
    }
    // Fallback: linear scan
    for p in 10000u16..=65535 {
        if !used.contains(&p) {
            return Ok(p);
        }
    }
    Err("No free port available in 10000-65535".into())
}

#[derive(Debug)]
pub enum CreateRuleError {
    BadRequest(String),
    PortConflict(u16),
    Database(DbError),
}

#[derive(Debug)]
pub enum UpdateRuleError {
    BadRequest(String),
    NotFound,
    PortConflict,
    Internal(String),
    Database(DbError),
}

async fn validate_admin_owned_inbound_group(
    db: &dyn Repository,
    gid: i64,
    context: &str,
) -> Result<(), CreateRuleError> {
    match GroupRepository::find_by_id(db, gid, &ResourceScope::All).await {
        Ok(Some(g)) => {
            let owner_is_admin = match db.is_admin(g.uid).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("{}: group_in is_admin failed: {}", context, e);
                    return Err(CreateRuleError::Database(e));
                }
            };
            if g.group_type != "in" || !owner_is_admin {
                return Err(CreateRuleError::BadRequest(
                    "device_group_in not found".into(),
                ));
            }
            Ok(())
        }
        Ok(None) => Err(CreateRuleError::BadRequest(
            "device_group_in not found".into(),
        )),
        Err(e) => {
            tracing::error!("{}: group_in find_by_id failed: {}", context, e);
            Err(CreateRuleError::Database(e))
        }
    }
}

async fn validate_owner_outbound_group(
    db: &dyn Repository,
    gid_out: i64,
    owner_scope: &ResourceScope,
    context: &str,
) -> Result<(), CreateRuleError> {
    match GroupRepository::find_by_id(db, gid_out, owner_scope).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(CreateRuleError::BadRequest(
            "device_group_out does not belong to the rule owner".into(),
        )),
        Err(e) => {
            tracing::error!("{}: group_out find_by_id failed: {}", context, e);
            Err(CreateRuleError::Database(e))
        }
    }
}

pub async fn create_rule(
    db: &dyn Repository,
    caller_user_id: i64,
    caller_admin: bool,
    req: &CreateRuleRequest,
) -> Result<(), CreateRuleError> {
    // v0.4.10: resolve the rule's owner. An admin may specify owner_uid to
    // create on behalf of another user; a non-admin's owner_uid is IGNORED and
    // the rule is attributed to themselves (defense against forgery).
    let owner_uid = if caller_admin {
        req.owner_uid.unwrap_or(caller_user_id)
    } else {
        caller_user_id
    };

    // If the admin is creating on behalf of another user, validate that user
    // exists and is not banned (a banned/deleted owner can't own new rules).
    if owner_uid != caller_user_id {
        match db.find_banned_by_id(owner_uid).await {
            Ok(Some(false)) => {}
            Ok(Some(true)) => return Err(CreateRuleError::BadRequest("owner is banned".into())),
            Ok(None) => return Err(CreateRuleError::BadRequest("owner does not exist".into())),
            Err(e) => {
                tracing::error!("create_rule: owner find_banned_by_id failed: {}", e);
                return Err(CreateRuleError::Database(e));
            }
        }
    }

    // The scope for validating referenced groups = the FINAL owner.
    let owner_scope = ResourceScope::Owner(owner_uid);

    // v0.4.20: only direct forward_mode is supported. Group/chain forwarding
    // is no longer exposed in the UI and is rejected at the API boundary.
    // Existing rules with group forwarding still generate valid config, but
    // new rules must use direct.
    if req.forward_mode != "direct" {
        return Err(CreateRuleError::BadRequest(
            "forward_mode: only 'direct' is supported; group/chain forwarding is no longer available"
                .into(),
        ));
    }
    if req.device_group_out.is_some() {
        return Err(CreateRuleError::BadRequest(
            "device_group_out: outbound-group forwarding is no longer supported; remove device_group_out"
                .into(),
        ));
    }

    // v0.4.12 PR1: device_group_in MUST be an inbound group (`group_type='in'`)
    // owned by an ADMIN.
    validate_admin_owned_inbound_group(db, req.device_group_in, "create_rule").await?;

    // Only validate device_group_out ownership (outbound is user-specific).
    if let Some(gid_out) = req.device_group_out {
        validate_owner_outbound_group(db, gid_out, &owner_scope, "create_rule").await?;
    }

    if !is_public_transport_accepted(req.public_transport) {
        return Err(CreateRuleError::BadRequest(
            "public_transport: only 'raw', 'ws' and 'tls_simple' are supported".into(),
        ));
    }

    if let Some(msg) = validate_protocol_transport(
        protocol_to_str(&req.protocol),
        req.public_transport.to_db_str(),
    ) {
        return Err(CreateRuleError::BadRequest(msg.into()));
    }

    let targets = normalize_rule_targets(req.targets.clone(), &req.target_addr, req.target_port)
        .map_err(|msg| CreateRuleError::BadRequest(msg.into()))?;
    let primary_target = &targets[0];

    // v0.4.11 PR1: strong validation for transport/profile binding:
    // - Raw: tunnel_profile_id must be NULL
    // - WS: must bind a ws transport template
    // - TLS Simple: must bind a tls_simple transport template
    let public_transport = &req.public_transport;
    if let Some(pid) = req.tunnel_profile_id {
        if public_transport == &PublicTransport::Raw {
            return Err(CreateRuleError::BadRequest(
                "tunnel_profile_id must be null for Raw transport".into(),
            ));
        }
        match db
            .find_profile_by_id(pid, &ProfileScope::AvailableTemplates)
            .await
        {
            Ok(None) => {
                return Err(CreateRuleError::BadRequest(
                    "tunnel_profile_id: no such profile".into(),
                ));
            }
            Ok(Some(profile)) => {
                let expected_transport = match public_transport {
                    PublicTransport::Ws => "ws",
                    PublicTransport::TlsSimple => "tls_simple",
                    PublicTransport::Raw => {
                        return Err(CreateRuleError::BadRequest(
                            "tunnel_profile_id must be null for Raw transport".into(),
                        ));
                    }
                };
                if profile.transport != expected_transport {
                    return Err(CreateRuleError::BadRequest(format!(
                        "tunnel_profile_id: profile transport '{}' does not match '{}' transport",
                        profile.transport, expected_transport
                    )));
                }
                if let Some(msg) = validate_protocol_transport(
                    protocol_to_str(&req.protocol),
                    profile.transport.as_str(),
                ) {
                    return Err(CreateRuleError::BadRequest(msg.into()));
                }
            }
            Err(e) => {
                tracing::error!("create_rule: find_profile_by_id failed: {}", e);
                return Err(CreateRuleError::Database(e));
            }
        }
    } else {
        if public_transport == &PublicTransport::Ws {
            return Err(CreateRuleError::BadRequest(
                "tunnel_profile_id is required for WebSocket transport".into(),
            ));
        }
        if public_transport == &PublicTransport::TlsSimple {
            return Err(CreateRuleError::BadRequest(
                "tunnel_profile_id is required for TLS Simple transport".into(),
            ));
        }
    }

    let protocol_str = protocol_to_str(&req.protocol);
    let public_str = req.public_transport.to_db_str();
    let node_str = req.public_transport.derive_node_transport().to_db_str();
    let route_str = req.route_mode.to_db_str();
    let ws_path: Option<String> = if req.public_transport == PublicTransport::Ws {
        req.ws_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    } else {
        None
    };

    let lb_db = req.load_balance_strategy.to_db_str();
    let up_mbps = req.upload_limit_mbps.unwrap_or(0).max(0);
    let down_mbps = req.download_limit_mbps.unwrap_or(0).max(0);

    let mut attempt = 0u32;
    let max_attempts = if req.listen_port.is_some() { 1 } else { 8 };
    let mut last_port: Option<u16> = req.listen_port;
    // v1.2: create_rule_full does the rule INSERT + targets + LB + rate limits
    // + tunnel profile in ONE transaction and returns the new rule's id
    // directly (SQLite last_insert_rowid / PG RETURNING id). This replaces the
    // old insert_quota_guarded-then-list_rules-by-(owner,listen_port)-lookup,
    // which wrote the side-tables to the WRONG rule when two inbound groups
    // reused the same listen_port (the per-group unique index makes that legal
    // but the lookup ignored device_group_in). Atomicity also guarantees a
    // mid-create failure leaves no half-rule.
    let result: Result<Option<i64>, DbError> = loop {
        let port = match last_port {
            Some(p) => p,
            None => match auto_assign_port(db, req.device_group_in, protocol_str).await {
                Ok(p) => p,
                Err(e) => break Err(DbError::Other(sqlx::Error::Configuration(e.into()))),
            },
        };
        last_port = Some(port);

        match db
            .create_rule_full(
                &req.name,
                owner_uid,
                port as i32,
                protocol_str,
                public_str,
                node_str,
                route_str,
                public_str,
                ws_path.as_deref(),
                req.device_group_in,
                req.device_group_out,
                &req.forward_mode,
                &primary_target.host,
                primary_target.port as i32,
                &targets,
                lb_db,
                up_mbps,
                down_mbps,
                req.tunnel_profile_id,
            )
            .await
        {
            Ok(opt) => break Ok(opt),
            Err(DbError::PortConflict | DbError::UniqueViolation)
                if req.listen_port.is_none() && attempt + 1 < max_attempts =>
            {
                attempt += 1;
                last_port = None;
                tracing::debug!(
                    "create_rule: listen_port {} taken on group {}; retry {}",
                    port,
                    req.device_group_in,
                    attempt
                );
                continue;
            }
            Err(e) => break Err(e),
        }
    };

    match result {
        Ok(None) => {
            // Quota exhausted: the guarded INSERT matched 0 rows.
            let current_count: i64 = db.count_by_uid(owner_uid).await.unwrap_or(0);
            let max_rules: i32 = db.max_rules_for_uid(owner_uid).await.unwrap_or(0);
            Err(CreateRuleError::BadRequest(format!(
                "Rule limit reached: you have {} rules, max is {}",
                current_count, max_rules
            )))
        }
        Ok(Some(_)) => Ok(()),
        Err(DbError::PortConflict | DbError::UniqueViolation) => {
            Err(CreateRuleError::PortConflict(last_port.unwrap_or(0)))
        }
        Err(e) => {
            tracing::error!("create_rule: create_rule_full failed: {}", e);
            Err(CreateRuleError::Database(e))
        }
    }
}

fn map_create_rule_validation_error(err: CreateRuleError) -> UpdateRuleError {
    match err {
        CreateRuleError::BadRequest(msg) => UpdateRuleError::BadRequest(msg),
        CreateRuleError::PortConflict(_) => UpdateRuleError::PortConflict,
        CreateRuleError::Database(e) => UpdateRuleError::Database(e),
    }
}

pub async fn update_rule(
    db: &dyn Repository,
    id: i64,
    scope: &ResourceScope,
    req: &UpdateRuleRequest,
) -> Result<(), UpdateRuleError> {
    // v0.4.20: only direct forward_mode is supported.
    if let Some(ref mode) = req.forward_mode {
        if mode != "direct" {
            return Err(UpdateRuleError::BadRequest(
                "forward_mode: only 'direct' is supported; group/chain forwarding is no longer available"
                    .into(),
            ));
        }
    }
    if req.device_group_out.is_some() {
        return Err(UpdateRuleError::BadRequest(
            "device_group_out: outbound-group forwarding is no longer supported; remove device_group_out"
                .into(),
        ));
    }

    if let Some(ref transport) = req.public_transport {
        if !is_public_transport_accepted(*transport) {
            return Err(UpdateRuleError::BadRequest(
                "public_transport: only 'raw', 'ws' and 'tls_simple' are supported".into(),
            ));
        }
    }

    // Load the existing rule once and reuse it for stored protocol/profile/owner.
    let existing = match db.find_rule_by_id(id, scope).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(UpdateRuleError::NotFound),
        Err(e) => {
            tracing::error!("update_rule {}: find_rule_by_id failed: {}", id, e);
            return Err(UpdateRuleError::Database(e));
        }
    };
    let owner_scope = ResourceScope::Owner(existing.uid);

    if let Some(gid_in) = req.device_group_in {
        validate_admin_owned_inbound_group(db, gid_in, "update_rule")
            .await
            .map_err(map_create_rule_validation_error)?;
    }
    if let Some(gid_out) = req.device_group_out {
        validate_owner_outbound_group(db, gid_out, &owner_scope, "update_rule")
            .await
            .map_err(map_create_rule_validation_error)?;
    }

    // Effective protocol×transport cross-check.
    let stored: Option<(String, String)> = match db.find_transport_by_id(id, scope).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("update_rule {}: find_transport_by_id failed: {}", id, e);
            return Err(UpdateRuleError::Database(e));
        }
    };
    let effective_protocol = req
        .protocol
        .as_ref()
        .map(protocol_to_str)
        .map(str::to_string)
        .or_else(|| stored.as_ref().map(|(p, _)| p.clone()));
    let effective_transport = req
        .public_transport
        .map(|t| t.to_db_str().to_string())
        .or_else(|| stored.as_ref().map(|(_, t)| t.clone()));
    if let (Some(proto), Some(transport)) = (effective_protocol, effective_transport) {
        if let Some(msg) = validate_protocol_transport(&proto, &transport) {
            return Err(UpdateRuleError::BadRequest(msg.into()));
        }
    }

    let switching_to_direct =
        req.forward_mode.as_deref() == Some("direct") && req.device_group_out.is_none();
    let device_group_out_arg: Option<Option<i64>> = if switching_to_direct {
        Some(None)
    } else {
        req.device_group_out.map(Some)
    };

    let has_field = req.name.is_some()
        || req.listen_port.is_some()
        || req.protocol.is_some()
        || req.public_transport.is_some()
        || req.route_mode.is_some()
        || req.ws_path.is_some()
        || req.device_group_in.is_some()
        || req.device_group_out.is_some()
        || req.forward_mode.is_some()
        || req.target_addr.is_some()
        || req.target_port.is_some()
        || req.targets.is_some()
        || req.load_balance_strategy.is_some()
        || req.upload_limit_mbps.is_some()
        || req.download_limit_mbps.is_some()
        || req.tunnel_profile_id.is_some()
        || req.paused.is_some();
    let has_scalar_field = req.name.is_some()
        || req.listen_port.is_some()
        || req.protocol.is_some()
        || req.public_transport.is_some()
        || req.route_mode.is_some()
        || req.ws_path.is_some()
        || req.device_group_in.is_some()
        || req.device_group_out.is_some()
        || req.forward_mode.is_some()
        || req.target_addr.is_some()
        || req.target_port.is_some()
        || req.paused.is_some();
    if !has_field {
        return Err(UpdateRuleError::BadRequest("No fields to update".into()));
    }

    let normalized_targets = if let Some(targets) = req.targets.clone() {
        let legacy_host = req.target_addr.as_deref().unwrap_or("127.0.0.1");
        let legacy_port = req.target_port.unwrap_or(1);
        Some(
            normalize_rule_targets(Some(targets), legacy_host, legacy_port)
                .map_err(|msg| UpdateRuleError::BadRequest(msg.into()))?,
        )
    } else {
        None
    };

    let existing_transport = match existing.public_transport.as_str() {
        "raw" => PublicTransport::Raw,
        "ws" => PublicTransport::Ws,
        "tls_simple" => PublicTransport::TlsSimple,
        _ => {
            tracing::error!(
                "update_rule {}: unknown existing public_transport '{}'",
                id,
                existing.public_transport
            );
            return Err(UpdateRuleError::Internal(
                "internal error: unknown transport".into(),
            ));
        }
    };
    let effective_transport = req
        .public_transport
        .as_ref()
        .copied()
        .unwrap_or(existing_transport);
    let effective_pid = match req.tunnel_profile_id {
        Some(pid_opt) => pid_opt,
        None => existing.tunnel_profile_id,
    };

    match (effective_transport, effective_pid) {
        (PublicTransport::Raw, Some(_)) => {
            return Err(UpdateRuleError::BadRequest(
                "tunnel_profile_id must be null for Raw transport".into(),
            ));
        }
        (PublicTransport::Ws, None) | (PublicTransport::TlsSimple, None) => {
            let transport_name = match effective_transport {
                PublicTransport::Ws => "WebSocket",
                PublicTransport::TlsSimple => "TLS Simple",
                PublicTransport::Raw => unreachable!(),
            };
            return Err(UpdateRuleError::BadRequest(format!(
                "tunnel_profile_id is required for {} transport",
                transport_name
            )));
        }
        (PublicTransport::Ws | PublicTransport::TlsSimple, Some(pid)) => {
            let expected_transport = match effective_transport {
                PublicTransport::Ws => "ws",
                PublicTransport::TlsSimple => "tls_simple",
                PublicTransport::Raw => unreachable!(),
            };
            match db
                .find_profile_by_id(pid, &ProfileScope::AvailableTemplates)
                .await
            {
                Ok(None) => {
                    return Err(UpdateRuleError::BadRequest(
                        "tunnel_profile_id: no such profile".into(),
                    ));
                }
                Ok(Some(profile)) => {
                    if profile.transport != expected_transport {
                        return Err(UpdateRuleError::BadRequest(format!(
                            "tunnel_profile_id: profile transport '{}' does not match '{}' transport",
                            profile.transport, expected_transport
                        )));
                    }
                    let proto_to_check = match req.protocol.as_ref() {
                        Some(p) => protocol_to_str(p),
                        None => existing.protocol.as_str(),
                    };
                    if let Some(msg) =
                        validate_protocol_transport(proto_to_check, profile.transport.as_str())
                    {
                        return Err(UpdateRuleError::BadRequest(msg.into()));
                    }
                }
                Err(e) => {
                    tracing::error!("update_rule {}: find_profile_by_id failed: {}", id, e);
                    return Err(UpdateRuleError::Database(e));
                }
            }
        }
        (PublicTransport::Raw, None) => {}
    }

    if let Some(new_proto) = req.protocol.as_ref() {
        let effective_pid = match req.tunnel_profile_id {
            Some(pid_opt) => pid_opt,
            None => existing.tunnel_profile_id,
        };
        if let Some(pid) = effective_pid {
            match db.find_profile_by_id(pid, &ProfileScope::All).await {
                Ok(Some(profile)) => {
                    if validate_protocol_transport(
                        protocol_to_str(new_proto),
                        profile.transport.as_str(),
                    )
                    .is_some()
                    {
                        return Err(UpdateRuleError::BadRequest(
                            "the existing tunnel profile is incompatible with the requested protocol"
                                .into(),
                        ));
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::error!("update_rule {}: find_profile_by_id failed: {}", id, e);
                    return Err(UpdateRuleError::Database(e));
                }
            }
        }
    }

    let (public, node, entry) = match req.public_transport {
        Some(v) => {
            let p = v.to_db_str();
            let n = v.derive_node_transport().to_db_str();
            (Some(p), Some(n), Some(p))
        }
        None => (None, None, None),
    };
    let ws_path: Option<Option<&str>> = req.ws_path.as_ref().map(|v| {
        v.as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s as &str)
    });

    let update_result = if has_scalar_field {
        db.update_rule_fields(
            id,
            scope,
            req.name.as_deref(),
            req.listen_port.map(|p| p as i32),
            req.protocol.as_ref().map(protocol_to_str),
            public,
            node,
            entry,
            req.route_mode.as_ref().map(|r| r.to_db_str()),
            ws_path,
            req.device_group_in,
            device_group_out_arg,
            req.forward_mode.as_deref(),
            req.target_addr.as_deref(),
            req.target_port.map(|p| p as i32),
            req.paused,
        )
        .await
    } else {
        Ok(1)
    };

    match update_result {
        Ok(0) => Err(UpdateRuleError::NotFound),
        Ok(_) => {
            if let Some(targets) = normalized_targets.as_ref() {
                if let Err(e) = db.replace_rule_targets(id, scope, targets).await {
                    tracing::error!("update_rule {}: replace_rule_targets failed: {}", id, e);
                    return Err(UpdateRuleError::Database(e));
                }
            }
            if let Some(strategy) = req.load_balance_strategy {
                if let Err(e) = db
                    .set_rule_load_balance_strategy(id, scope, strategy.to_db_str())
                    .await
                {
                    tracing::error!(
                        "update_rule {}: set_rule_load_balance_strategy failed: {}",
                        id,
                        e
                    );
                    return Err(UpdateRuleError::Database(e));
                }
            }
            if req.upload_limit_mbps.is_some() || req.download_limit_mbps.is_some() {
                let up_mbps = req.upload_limit_mbps.unwrap_or(0).max(0);
                let down_mbps = req.download_limit_mbps.unwrap_or(0).max(0);
                if let Err(e) = db.set_rule_rate_limits(id, scope, up_mbps, down_mbps).await {
                    tracing::error!("update_rule {}: set_rule_rate_limits failed: {}", id, e);
                    return Err(UpdateRuleError::Database(e));
                }
            }
            if let Some(pid_opt) = req.tunnel_profile_id {
                if let Err(e) = db.set_rule_tunnel_profile(id, scope, pid_opt).await {
                    tracing::error!("update_rule {}: set_rule_tunnel_profile failed: {}", id, e);
                    return Err(UpdateRuleError::Database(e));
                }
            }
            Ok(())
        }
        Err(DbError::UniqueViolation | DbError::PortConflict) => Err(UpdateRuleError::PortConflict),
        Err(e) => {
            tracing::error!("update_rule {}: update_rule_fields failed: {}", id, e);
            Err(UpdateRuleError::Database(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The valid combinations must all pass (return None). These are the ones
    /// the UI and the node actually support in v0.3.0-alpha.
    #[test]
    fn valid_combinations_pass() {
        // The overwhelmingly common case: raw TCP.
        assert!(validate_protocol_transport("tcp", "raw").is_none());
        // raw UDP — the only valid UDP combination.
        assert!(validate_protocol_transport("udp", "raw").is_none());
        // raw TCP+UDP — both listeners, raw transport.
        assert!(validate_protocol_transport("tcp_udp", "raw").is_none());
        // v0.3.0-alpha headline: WS over TCP.
        assert!(validate_protocol_transport("tcp", "ws").is_none());
    }

    /// UDP / TCP+UDP over WS must be rejected (WS carries TCP only in alpha).
    /// This is the constraint the frontend enforces by disabling the protocol
    /// picker — the API must reject it independently for direct/import callers.
    #[test]
    fn ws_rejects_udp_and_tcp_udp() {
        assert!(validate_protocol_transport("udp", "ws").is_some());
        assert!(validate_protocol_transport("tcp_udp", "ws").is_some());
        // And the error message mentions TCP-only so the caller knows why.
        let msg = validate_protocol_transport("udp", "ws").unwrap();
        assert!(
            msg.contains("TCP forwarding only"),
            "error should explain TCP-only: got {:?}",
            msg
        );
    }

    /// UDP-bearing protocols (udp OR tcp_udp) are rejected for ANY non-raw
    /// transport, not just ws. tls_simple would also be caught here (though
    /// that transport is rejected earlier by is_public_transport_accepted).
    #[test]
    fn udp_bearing_requires_raw_transport() {
        // tcp_udp includes a UDP listener → same rule as pure udp.
        assert!(validate_protocol_transport("tcp_udp", "ws").is_some());
        assert!(validate_protocol_transport("tcp_udp", "tls").is_some());
        assert!(validate_protocol_transport("udp", "wss").is_some());
        // But tcp_udp + raw is fine (both listeners, raw ingress).
        assert!(validate_protocol_transport("tcp_udp", "raw").is_none());
    }

    /// WS over TCP is the ONLY valid ws combination. Make sure the boundary is
    /// exactly at protocol=tcp — anything else is rejected, tcp passes.
    #[test]
    fn ws_accepts_only_tcp() {
        assert!(validate_protocol_transport("tcp", "ws").is_none());
        // Every other protocol string with ws is rejected.
        for proto in ["udp", "tcp_udp", "quic", ""] {
            assert!(
                validate_protocol_transport(proto, "ws").is_some(),
                "ws + {:?} should be rejected",
                proto,
            );
        }
    }

    /// Target normalization: a missing targets list falls back to the legacy
    /// host:port; an empty list is rejected; >32 is rejected; all-disabled is
    /// rejected; a bad host is rejected.
    #[test]
    fn target_normalization_rules() {
        // Fallback to legacy single target.
        let out = normalize_rule_targets(None, "1.2.3.4", 80).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].host, "1.2.3.4");
        assert_eq!(out[0].port, 80);

        // Empty explicit list → rejected.
        assert!(normalize_rule_targets(Some(vec![]), "1.2.3.4", 80).is_err());

        // >32 targets → rejected.
        let many: Vec<RuleTargetRequest> = (0..33)
            .map(|_| RuleTargetRequest {
                host: "1.2.3.4".into(),
                port: 80,
                enabled: true,
            })
            .collect();
        assert!(normalize_rule_targets(Some(many), "x", 1).is_err());

        // All-disabled → rejected.
        let disabled = vec![RuleTargetRequest {
            host: "1.2.3.4".into(),
            port: 80,
            enabled: false,
        }];
        assert!(normalize_rule_targets(Some(disabled), "x", 1).is_err());

        // Bad host (has scheme) → rejected.
        let bad = vec![RuleTargetRequest {
            host: "http://x".into(),
            port: 80,
            enabled: true,
        }];
        assert!(normalize_rule_targets(Some(bad), "x", 1).is_err());
    }
}
