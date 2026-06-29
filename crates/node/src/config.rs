use serde::Deserialize;

const INSECURE_NODE_TOKEN: &str = "default-token";

#[derive(Debug, Deserialize, Clone)]
pub struct NodeConfig {
    pub panel_url: String,
    pub token: String,
    pub poll_interval: u64,
    pub tls_cert_path: Option<String>,
    pub tls_key_path: Option<String>,
    /// v0.4.6: NIC for traffic stats. "auto" = auto-detect default route.
    pub network_interface: String,
    /// v1.0.5: IPv4 listen address. Empty = disabled. Default "0.0.0.0".
    pub listen_ipv4: String,
    /// v1.0.5: IPv6 listen address. Empty = disabled. Default "::".
    pub listen_ipv6: String,
    /// v1.0.5: NIC for outbound IPv4 egress. "auto" = system routing.
    pub outbound_interface: String,
    /// v1.0.5: Exact IPv4 source for outbound connections.
    pub outbound_bind_ipv4: Option<String>,
}

impl NodeConfig {
    pub fn load() -> Self {
        let panel_url =
            std::env::var("PANEL_URL").unwrap_or_else(|_| "http://127.0.0.1:18888".into());
        let token = std::env::var("NODE_TOKEN").unwrap_or_else(|_| String::new());
        let poll_interval = std::env::var("POLL_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        let cfg = Self {
            panel_url,
            token,
            poll_interval,
            tls_cert_path: std::env::var("TLS_CERT_PATH")
                .ok()
                .filter(|s| !s.is_empty()),
            tls_key_path: std::env::var("TLS_KEY_PATH").ok().filter(|s| !s.is_empty()),
            network_interface: std::env::var("NETWORK_INTERFACE")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "auto".to_string()),
            // v1.0.5: distinguish UNSET (use default, backward compatible) from
            // EXPLICITLY EMPTY (LISTEN_IPV6= → disable that family). std::env::var
            // returns Err only when unset; Ok("") when set to empty.
            listen_ipv4: match std::env::var("LISTEN_IPV4") {
                Ok(v) => v.trim().to_string(),
                Err(_) => "0.0.0.0".to_string(),
            },
            listen_ipv6: match std::env::var("LISTEN_IPV6") {
                Ok(v) => v.trim().to_string(),
                Err(_) => "::".to_string(),
            },
            outbound_interface: std::env::var("OUTBOUND_INTERFACE")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "auto".to_string()),
            outbound_bind_ipv4: std::env::var("OUTBOUND_BIND_IPV4")
                .ok()
                .filter(|s| !s.trim().is_empty()),
        };
        cfg.validate();
        cfg
    }

    fn validate(&self) {
        if self.token.trim().is_empty() {
            eprintln!(
                "FATAL: NODE_TOKEN is not set.\n  \
                 Set it to a real inbound-group token from the panel UI, e.g.:\n  \
                 NODE_TOKEN=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
            );
            std::process::exit(1);
        }
        if self.token == INSECURE_NODE_TOKEN {
            eprintln!(
                "FATAL: NODE_TOKEN is still set to the insecure default \"{}\".\n  \
                 Set it to a real inbound-group token from the panel UI.",
                INSECURE_NODE_TOKEN
            );
            std::process::exit(1);
        }
    }
}
