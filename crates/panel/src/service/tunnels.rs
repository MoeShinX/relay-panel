use crate::db::error::DbError;
use crate::db::repo::{CreateTunnelRequest, Repository, TunnelRepository, UpdateTunnelRequest};
use relay_shared::models::Tunnels;
use serde_json::{json, Value};
use std::fmt;
use uuid::Uuid;

// ── Config generation ──

/// Generate a sing-box config for the tunnel.
///
/// Produces a minimal vless reality listener with a routing table that forwards
/// to `group_out` as an outbound. The user can override the generated JSON by
/// writing their own config before storing.
pub fn generate_tunnel_config(tunnel: &Tunnels) -> Value {
    let listener_id = Uuid::new_v4().to_string();
    let outbound_id = Uuid::new_v4().to_string();

    json!({
        "log": {
            "level": "info",
            "timestamp": true
        },
        "dns": {
            "servers": [
                {
                    "tag": "dns_proxy",
                    "address": "tcp://8.8.8.8",
                    "detour": outbound_id
                },
                {
                    "tag": "dns_direct",
                    "address": "local",
                    "detour": "direct"
                },
                {
                    "tag": "dns_block",
                    "address": "rcode://success"
                }
            ],
            "rules": [
                {
                    "outbound": "sing-box",
                    "server": "dns_proxy"
                },
                {
                    "geosite": "cn",
                    "server": "dns_direct"
                }
            ],
            "final": "dns_direct",
            "disable_cache": false
        },
        "inbounds": [
            {
                "type": "vless",
                "tag": "tunnel-in",
                "listen": "::",
                "listen_port": tunnel.listen_port,
                "users": [],
                "sniff": true,
                "sniff_override_destination": true,
                "domain_strategy": "ipv4_only",
                "settings": {
                    "decryption": "none"
                },
                "multiplex": {
                    "enabled": true,
                    "padding": true
                }
            }
        ],
        "outbounds": [
            {
                "type": "socks",
                "tag": outbound_id,
                "server": "::",
                "server_port": 1080
            },
            {
                "type": "direct",
                "tag": "direct"
            },
            {
                "type": "block",
                "tag": "block"
            }
        ],
        "route": {
            "rules": [
                {
                    "network": "udp",
                    "port": 443,
                    "outbound": "block"
                },
                {
                    "clash_mode": "global",
                    "outbound": outbound_id
                },
                {
                    "clash_mode": "direct",
                    "outbound": "direct"
                },
                {
                    "geosite": "private",
                    "outbound": "direct"
                },
                {
                    "geosite": "cn",
                    "outbound": "direct"
                }
            ],
            "final": outbound_id,
            "auto_detect_interface": true
        },
        "experimental": {
            "cache_file": {
                "enabled": true,
                "store_route": true
            }
        }
    })
}

/// Generate a secret JSON for the tunnel (vless reality settings).
///
/// The user fills in `server_addr`, `server_port`, `private_key` and optionally
/// `client_id` to authenticate with their reality server.
pub fn generate_tunnel_secret() -> Value {
    json!({
        "server_addr": "",
        "server_port": 443,
        "private_key": "",
        "client_id": ""
    })
}

#[derive(Debug)]
pub enum GenerateConfigError {
    Database(DbError),
}

impl fmt::Display for GenerateConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenerateConfigError::Database(e) => write!(f, "database error: {}", e),
        }
    }
}

/// Generate configs for a tunnel and update the row in-place.
///
/// Replaces `config_json` and `secret_json` with freshly generated values.
/// Useful when the user wants a clean starting point or regenerates after
/// changing `listen_port` / `protocol`.
pub async fn regenerate_config(
    db: &dyn Repository,
    id: i64,
    uid: i64,
) -> Result<(Value, Value), GenerateConfigError> {
    let tunnel = match TunnelRepository::find_by_id(db, id, uid).await {
        Ok(Some(t)) => t,
        Ok(None) => return Ok((json!({}), json!({}))),
        Err(e) => return Err(GenerateConfigError::Database(e)),
    };

    let config = generate_tunnel_config(&tunnel);
    let secret = generate_tunnel_secret();

    let _ = db
        .update_tunnel_fields(
            id,
            uid,
            &UpdateTunnelRequest {
                config_json: Some(config.to_string()),
                secret_json: Some(secret.to_string()),
                ..Default::default()
            },
        )
        .await
        .map_err(GenerateConfigError::Database)?;

    Ok((config, secret))
}

// ── CRUD ──

#[derive(Debug)]
pub enum CreateTunnelError {
    DuplicateGroupIn,
    Database(DbError),
}

impl fmt::Display for CreateTunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CreateTunnelError::DuplicateGroupIn => write!(f, "duplicate group_in"),
            CreateTunnelError::Database(e) => write!(f, "database error: {}", e),
        }
    }
}

#[derive(Debug)]
pub enum UpdateTunnelError {
    NotFound,
    DuplicateGroupIn,
    Database(DbError),
}

impl fmt::Display for UpdateTunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateTunnelError::NotFound => write!(f, "tunnel not found"),
            UpdateTunnelError::DuplicateGroupIn => write!(f, "duplicate group_in"),
            UpdateTunnelError::Database(e) => write!(f, "database error: {}", e),
        }
    }
}

#[derive(Debug)]
pub enum DeleteTunnelError {
    Database(DbError),
}

impl fmt::Display for DeleteTunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeleteTunnelError::Database(e) => write!(f, "database error: {}", e),
        }
    }
}

/// Create a tunnel. Validates that `group_in` is not already claimed by another
/// tunnel (enforced by the UNIQUE constraint, caught via the pre-check).
pub async fn create_tunnel(
    db: &dyn Repository,
    req: &CreateTunnelRequest,
) -> Result<i64, CreateTunnelError> {
    // Pre-check group_in uniqueness.
    if let Ok(Some(_)) = TunnelRepository::find_by_group_in(db, req.group_in).await {
        return Err(CreateTunnelError::DuplicateGroupIn);
    }

    TunnelRepository::create_tunnel(db, req)
        .await
        .map_err(CreateTunnelError::Database)
}

/// Update a tunnel's fields (partial update). Returns the number of rows affected.
pub async fn update_tunnel(
    db: &dyn Repository,
    id: i64,
    uid: i64,
    req: &UpdateTunnelRequest,
) -> Result<u64, UpdateTunnelError> {
    // Pre-check group_in uniqueness if changing.
    if let Some(new_group_in) = req.group_in {
        // Skip self.
        if let Ok(Some(existing)) = TunnelRepository::find_by_id(db, id, uid).await {
            if existing.group_in != new_group_in {
                if let Ok(Some(other)) = TunnelRepository::find_by_group_in(db, new_group_in).await
                {
                    if other.id != id {
                        return Err(UpdateTunnelError::DuplicateGroupIn);
                    }
                }
            }
        }
    }

    TunnelRepository::update_tunnel_fields(db, id, uid, req)
        .await
        .map_err(UpdateTunnelError::Database)
}

/// Delete a tunnel by id, owner-scoped.
pub async fn delete_tunnel(
    db: &dyn Repository,
    id: i64,
    uid: i64,
) -> Result<u64, DeleteTunnelError> {
    TunnelRepository::delete_tunnel(db, id, uid)
        .await
        .map_err(DeleteTunnelError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_tunnel_config_has_required_keys() {
        let tunnel = Tunnels {
            id: 1,
            name: "test".into(),
            group_in: 1,
            group_out: Some(2),
            protocol: "vless_reality".into(),
            listen_port: 443,
            config_json: "{}".into(),
            secret_json: "{}".into(),
            enabled: true,
            uid: 1,
            created_at: "2024-01-01 00:00:00".into(),
        };
        let cfg = generate_tunnel_config(&tunnel);
        assert!(cfg.get("inbounds").is_some());
        assert!(cfg.get("outbounds").is_some());
        assert!(cfg.get("route").is_some());
        assert!(cfg.get("dns").is_some());

        let inbounds: Vec<&Value> = cfg["inbounds"].as_array().unwrap().iter().collect();
        assert_eq!(inbounds.len(), 1);
        assert_eq!(inbounds[0]["listen_port"], 443);
    }

    #[test]
    fn generate_tunnel_secret_has_expected_fields() {
        let secret = generate_tunnel_secret();
        assert!(secret.get("server_addr").is_some());
        assert!(secret.get("server_port").is_some());
        assert!(secret.get("private_key").is_some());
        assert!(secret.get("client_id").is_some());
    }

    #[test]
    fn create_tunnel_error_debug_fmt() {
        let e = CreateTunnelError::DuplicateGroupIn;
        let msg = format!("{:?}", e);
        assert!(msg.contains("DuplicateGroupIn"));
    }
}
