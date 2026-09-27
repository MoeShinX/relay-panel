# Reverse Proxy Guide

RelayPanel serves a web UI + REST API + WebSocket endpoint from a single HTTP
port (`18888` by default). This guide covers putting the panel behind a reverse
proxy for TLS termination, custom domains, and production hardening.

---

## Quick decision: which setup?

| Your situation | Recommended approach |
|---------------|---------------------|
| Single server, no existing proxy, want auto-TLS | Use `RELAYPANEL_WEB_MODE=caddy` — Compose-managed Caddy handles everything. |
| Already have Nginx/Caddy on the host | Use `RELAYPANEL_WEB_MODE=reverse-proxy` — panel binds localhost only, your proxy forwards. |
| Firewall-restricted, no domain | Use `RELAYPANEL_WEB_MODE=direct` (default) — panel publishes `0.0.0.0:18888`. |

---

## `PUBLIC_PANEL_URL`

This is the URL that **forwarding nodes** use to reach the panel. The "Copy
Install Command" button in **Device Groups** reads it from
`/api/v1/system/version` and embeds it into the one-line install command.

| `PUBLIC_PANEL_URL` | Effect |
|-------------------|--------|
| Empty / unset | Frontend falls back to `window.location.origin`. Nodes use whatever URL the admin's browser is on. |
| `https://panel.example.com` | Nodes connect via `wss://panel.example.com/api/v1/node/ws`. Install command includes this URL. |
| `http://203.0.113.10:18888` | Explicit IP:port. Nodes use `ws://...` (no TLS). |

**Rule of thumb:** set `PUBLIC_PANEL_URL` whenever the panel is behind a
reverse proxy or domain. Otherwise nodes may get the wrong URL (e.g. the
admin's localhost address).

---

## Nginx

```nginx
# WebSocket upgrade for node control channel
location /api/v1/node/ws {
    proxy_pass http://127.0.0.1:18888;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_read_timeout 3600s;
}

# API + SPA
location / {
    proxy_pass http://127.0.0.1:18888;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
}
```

> **Path prefix not supported.** The panel serves the SPA from `/` and the API
> from `/api/v1/...`. Do not add a path prefix (e.g. `/panel/`) — it will break
> the SPA router and API routes.

---

## Caddy (external / host-level)

If you run Caddy on the host (not via the Compose profile):

```caddyfile
panel.example.com {
    encode gzip zstd
    reverse_proxy 127.0.0.1:18888
}
```

Caddy handles WebSocket upgrade automatically — no extra configuration needed.

---

## Caddy (Compose profile)

Set `RELAYPANEL_WEB_MODE=caddy` and `RELAYPANEL_DOMAIN=panel.example.com` in
`.env`. The Compose file includes a `caddy` service (profile `caddy`) that:

- Binds host ports `80` and `443`.
- Reads `RELAYPANEL_DOMAIN` from the environment.
- Auto-issues a Let's Encrypt certificate on first start.
- Proxies to `panel:18888` on the Compose network.

The `Caddyfile` is at the repo root and uses `{$RELAYPANEL_DOMAIN}` as the
site address. You can customize it (e.g. add rate limiting, IP filtering)
before running `deploy.sh`.

---

## Cloudflare / CDN

If the panel sits behind Cloudflare or another CDN:

1. Go to **Network** settings → enable **WebSockets**.
2. Ensure the CDN does not strip `Upgrade` or `Connection` headers.
3. If using Cloudflare's proxy (orange cloud), the CDN terminates TLS — set
   `PUBLIC_PANEL_URL` to the Cloudflare-facing domain (e.g.
   `https://panel.example.com`).

---

## WebSocket troubleshooting

If node logs show `websocket error` or keeps reconnecting:

1. **Panel unreachable:** verify `PANEL_URL` from the node's perspective.
2. **Invalid token:** verify `NODE_TOKEN` matches the panel UI.
3. **Reverse proxy missing Upgrade header:** check Nginx/Caddy config.
4. **CDN blocking WS:** enable WebSocket in CDN dashboard.
5. **Firewall:** ensure the port is open for HTTP and WS.

> Even if WebSocket fails, the node continues forwarding with cached config
> and falls back to HTTP polling every 10s. The node never exits due to WS
> failure.

---

## Security checklist

- [ ] Set a strong `admin` password — the first login forces a change from the
  default `admin123`.
- [ ] Set `PUBLIC_PANEL_URL` to the correct external URL.
- [ ] Enable HTTPS (TLS) for the admin UI — never expose `:18888` directly to
  the internet without TLS.
- [ ] Restrict the panel port with a firewall (only allow your IP or the
  reverse proxy's IP).
- [ ] Use strong, randomly-generated `JWT_SECRET` and `PANEL_KEY` (the
  `deploy.sh` script does this automatically).
- [ ] Rotate `NODE_TOKEN` periodically via the panel UI.

## Same-host vs separate-host reverse proxy

`RELAYPANEL_WEB_MODE=reverse-proxy` defaults to the safe same-host layout:

```env
RELAYPANEL_WEB_MODE=reverse-proxy
PUBLIC_PANEL_URL=https://panel.example.com
# panel binds 127.0.0.1:18888
```

If Nginx/Caddy runs on another server, it cannot reach `127.0.0.1` on the
panel host. In that case explicitly opt into the separate-host layout:

```env
RELAYPANEL_WEB_MODE=reverse-proxy
REVERSE_PROXY_EXTERNAL=1
PUBLIC_PANEL_URL=https://panel.example.com
# panel binds 0.0.0.0:18888
```

When using `REVERSE_PROXY_EXTERNAL=1`, restrict TCP/18888 with a firewall so
only the proxy host can reach the panel.

## Compose Caddy requirements

For `RELAYPANEL_WEB_MODE=caddy`:

- `RELAYPANEL_DOMAIN` must be a plain public domain such as
  `panel.example.com` (no `https://`, no path, no IP address, no `localhost`).
- `ACME_EMAIL` is optional. When set, `deploy.sh` injects Caddy's global
  `email` option so Let's Encrypt can use it for ACME account/contact purposes.
- Host ports `80` and `443/tcp` must be free; `deploy.sh` treats conflicts as
  fatal. Port `443/udp` is also published so Caddy can serve HTTP/3.
- `PUBLIC_PANEL_URL` defaults to `https://${RELAYPANEL_DOMAIN}` and existing
  values are preserved on upgrade.
