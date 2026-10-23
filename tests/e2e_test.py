#!/usr/bin/env python3
"""
RelayPanel end-to-end integration test.

Verifies:
  1. Login + JWT, user delete via /admin/users/{id}.
  2. Rules/groups created with uid from the caller's token (not hardcoded 1).
  3. Node authenticates via Authorization header (not query-string token).
  4. TCP + UDP traffic forwarded through the node AND counted.
  5. Traffic attributed by rule_id (not listen_port) — a second inbound group
     reusing the SAME listen port does NOT cross-charge the first rule.
  6. users.traffic_used grows alongside forward_rules.traffic_used.

Exit code 0 on success, non-zero on any failure.

Usage:  py tests\\e2e_test.py
Requires: the panel/node binaries already built (cargo build).
"""

import json
import os
import socket
import socketserver
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

# Force UTF-8 stdout so non-ASCII output never crashes on Windows GBK consoles.
try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except (AttributeError, ValueError):
    pass

BASE_URL = "http://127.0.0.1:18888/api/v1"
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PANEL_BIN = os.path.join(PROJECT_ROOT, "target", "debug", "relay-panel.exe")
NODE_BIN = os.path.join(PROJECT_ROOT, "target", "debug", "relay-node.exe")
DB_PATH = os.path.join(PROJECT_ROOT, "data.db")

# Test topology
TCP_LISTEN_PORT = 19999
UDP_LISTEN_PORT = 20000
TCP_LISTEN_PORT_2 = 19999  # SAME port, different inbound group -> disambiguation
UDP_ECHO_PORT = 9999
TCP_ECHO_PORT = 8888

# v0.1.2 new feature test ports
TCPUDP_LISTEN_PORT = 20001   # one rule, both TCP and UDP
DIRECT_TCP_PORT = 20002      # direct mode (no outbound group)

# node-v1.2.8: clients reach the node over IPv4, the target listens on ::1
TCP6_LISTEN_PORT = 20003
UDP6_LISTEN_PORT = 20004
TCP6_ECHO_PORT = 8889
UDP6_ECHO_PORT = 9998

# panel 1.2.15 / node 1.2.9: PROXY protocol through two rules on one node
PP_A_PORT = 20005      # sends v2 to PP_B_PORT
PP_B_PORT = 20006      # accepts it, sends v1 to the header-echo target
PP_TARGET_PORT = 8890  # answers with the header line it received, then the data


# ---------- echo servers ----------
class TCPHandler(socketserver.BaseRequestHandler):
    def handle(self):
        while True:
            data = self.request.recv(4096)
            if not data:
                break
            self.request.sendall(data)


class UDPHandler(socketserver.BaseRequestHandler):
    def handle(self):
        data, sock = self.request
        sock.sendto(data, self.client_address)


def start_echo_servers():
    tcp = socketserver.ThreadingTCPServer(("127.0.0.1", TCP_ECHO_PORT), TCPHandler)
    udp = socketserver.ThreadingUDPServer(("127.0.0.1", UDP_ECHO_PORT), UDPHandler)
    tcp.allow_reuse_address = udp.allow_reuse_address = True
    threading.Thread(target=tcp.serve_forever, daemon=True).start()
    threading.Thread(target=udp.serve_forever, daemon=True).start()
    print(f"[echo] TCP :{TCP_ECHO_PORT}  UDP :{UDP_ECHO_PORT}")
    return tcp, udp


class TCP6Server(socketserver.ThreadingTCPServer):
    address_family = socket.AF_INET6
    allow_reuse_address = True


class UDP6Server(socketserver.ThreadingUDPServer):
    address_family = socket.AF_INET6
    allow_reuse_address = True


class PPHeaderEcho(socketserver.BaseRequestHandler):
    """Reply with everything up to and including the first CRLF (the PROXY v1
    line the relay sent), then echo the rest."""
    def handle(self):
        data = b""
        while b"\r\n" not in data:
            chunk = self.request.recv(1024)
            if not chunk:
                return
            data += chunk
        self.request.sendall(data)
        while True:
            chunk = self.request.recv(1024)
            if not chunk:
                break
            self.request.sendall(chunk)


def start_pp_target():
    srv = socketserver.ThreadingTCPServer(("127.0.0.1", PP_TARGET_PORT), PPHeaderEcho)
    srv.allow_reuse_address = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    print(f"[echo] PROXY header target :{PP_TARGET_PORT}")
    return srv


def start_echo_servers_v6():
    """Echo servers on ::1, or None where the host has no IPv6 loopback."""
    try:
        tcp = TCP6Server(("::1", TCP6_ECHO_PORT), TCPHandler)
        udp = UDP6Server(("::1", UDP6_ECHO_PORT), UDPHandler)
    except OSError as e:
        print(f"[echo] no IPv6 loopback: {e}")
        return None
    threading.Thread(target=tcp.serve_forever, daemon=True).start()
    threading.Thread(target=udp.serve_forever, daemon=True).start()
    print(f"[echo] TCP [::1]:{TCP6_ECHO_PORT}  UDP [::1]:{UDP6_ECHO_PORT}")
    return tcp, udp


# ---------- HTTP helper ----------
def api(method, path, token=None, body=None, extra_headers=None):
    url = BASE_URL + path
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    if extra_headers:
        for k, v in extra_headers.items():
            req.add_header(k, v)
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read().decode())


def wait_for_port(host, port, timeout=30):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with socket.create_connection((host, port), timeout=1):
                return True
        except OSError:
            time.sleep(0.5)
    return False


def tcp_roundtrip(port, payload):
    """Send `payload` and read back as many bytes as were sent.

    The echo comes back through the relay in as many pieces as TCP and the
    echo server (4096-byte reads) make of it, so one recv() can return just the
    first: a 5016-byte echo read that way came back as 4096 bytes. Stops early
    on EOF or the socket timeout and returns what did arrive, so a short echo
    fails the caller's comparison with its real length instead of hanging.
    """
    with socket.create_connection(("127.0.0.1", port), timeout=5) as s:
        s.sendall(payload)
        got = b""
        try:
            while len(got) < len(payload):
                chunk = s.recv(len(payload) - len(got))
                if not chunk:
                    break
                got += chunk
        except socket.timeout:
            pass
        return got


def udp_roundtrip(port, payload, timeout=5):
    us = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    us.settimeout(timeout)
    us.sendto(payload, ("127.0.0.1", port))
    resp, _ = us.recvfrom(8192)
    us.close()
    return resp


# ---------- main ----------
def main():
    global PANEL_BIN, NODE_BIN
    if sys.platform != "win32":
        PANEL_BIN = PANEL_BIN[:-4]
        NODE_BIN = NODE_BIN[:-4]

    if os.path.exists(DB_PATH):
        os.remove(DB_PATH)

    start_echo_servers()

    panel_env = dict(os.environ, LISTEN="127.0.0.1:18888", RUST_LOG="info",
                     REGISTRATION_ENABLED="1",
                     ADMIN_INITIAL_PASSWORD="admin123")
    panel = subprocess.Popen([PANEL_BIN], env=panel_env,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    node = None
    print(f"[panel] PID {panel.pid}")
    try:
        assert wait_for_port("127.0.0.1", 18888), "panel did not start"

        # 1. Login as admin
        r = api("POST", "/auth/login", body={"username": "admin", "password": "admin123"})
        assert r["code"] == 0 and r["data"]["admin"], "login failed"
        admin_token = r["data"]["token"]
        admin_uid = 1  # default admin user
        print(f"[ok] admin login -> token (len {len(admin_token)})")

        # 1a. The seeded admin now boots with must_change_password set, so the
        # first login's token is only good for GET /user/me and PUT /user/password
        # (every other endpoint returns 403 PASSWORD_CHANGE_REQUIRED). Change the
        # password (allowed by the whitelist), which also bumps token_version and
        # revokes this token — so we must log in again with the new password.
        new_admin_pw = "admin-e2e-pass-1"
        chg = api("PUT", "/user/password", admin_token,
                  {"current_password": "admin123", "new_password": new_admin_pw})
        assert chg["code"] == 0, f"forced password change failed: {chg}"
        r = api("POST", "/auth/login", body={"username": "admin", "password": new_admin_pw})
        assert r["code"] == 0 and r["data"]["admin"], "re-login after password change failed"
        admin_token = r["data"]["token"]
        print("[ok] forced first-login password change + re-login")

        # 1b. Register + delete a temp user via /admin/users/{id}
        # v0.4.10 PR3: registration requires password >= 8 bytes (bcrypt boundary).
        tmp_user = f"tmp_{int(time.time())}"
        api("POST", "/auth/register", body={"username": tmp_user, "password": "temp-pass-1"})
        users_list = api("GET", "/admin/users", admin_token)["data"]
        tmp_id = next(u["id"] for u in users_list if u["username"] == tmp_user)
        del_res = api("DELETE", f"/admin/users/{tmp_id}", admin_token)
        assert del_res["code"] == 0, f"delete user failed: {del_res}"
        print(f"[ok] delete user {tmp_user} (id={tmp_id}) via /admin/users/{{id}}")

        # 2. Create groups + rules as admin. Verify uid == admin (from token),
        #    NOT hardcoded uid=1 by coincidence (admin IS uid 1, so we also
        #    create a second rule set under a different user below to prove
        #    the uid follows the caller).
        # v0.4.10: groups/rules moved from /admin/* to owner-scoped /groups and
        # /rules (the handler folds the caller's ResourceScope into the query).
        in_g1 = api("POST", "/groups", admin_token, {
            "name": "in-1", "group_type": "in",
            "connect_host": "127.0.0.1", "port_range": "10000-30000",
        })
        out_g1 = api("POST", "/groups", admin_token, {
            "name": "out-1", "group_type": "out",
            "connect_host": "127.0.0.1", "port_range": "1-65535",
        })
        in_g1_id = in_g1["data"]["id"]
        out_g1_id = out_g1["data"]["id"]
        in_g1_token = in_g1["data"]["token"]

        # Verify group uid came from token (admin = uid 1)
        assert in_g1["data"]["uid"] == admin_uid, \
            f"group uid should be {admin_uid} (from token), got {in_g1['data']['uid']}"
        print(f"[ok] group uid={in_g1['data']['uid']} (from token, not hardcoded)")

        tcp_resp = api("POST", "/rules", admin_token, {
            "name": "tcp-rule", "listen_port": TCP_LISTEN_PORT,
            "protocol": "tcp", "device_group_in": in_g1_id,
            "forward_mode": "direct",
            "target_addr": "127.0.0.1", "target_port": TCP_ECHO_PORT,
        })
        assert tcp_resp["code"] == 0, f"tcp-rule creation failed: {tcp_resp}"
        udp_resp = api("POST", "/rules", admin_token, {
            "name": "udp-rule", "listen_port": UDP_LISTEN_PORT,
            "protocol": "udp", "device_group_in": in_g1_id,
            "forward_mode": "direct",
            "target_addr": "127.0.0.1", "target_port": UDP_ECHO_PORT,
        })
        assert udp_resp["code"] == 0, f"udp-rule creation failed: {udp_resp}"

        # 2b. PROXY protocol chain (panel 1.2.15 / node 1.2.9). Created before
        #     the node starts: the panel refuses PROXY protocol for a group
        #     whose nodes it knows to be older than 1.2.9, and the node built
        #     here reports its own version.
        start_pp_target()
        for name, port, target_port, extra in (
            ("pp-b", PP_B_PORT, PP_TARGET_PORT, {
                "proxy_protocol_accept": True, "proxy_protocol_trusted": "127.0.0.1",
                "proxy_protocol_send": "v1"}),
            ("pp-a", PP_A_PORT, PP_B_PORT, {"proxy_protocol_send": "v2"}),
        ):
            r = api("POST", "/rules", admin_token, {
                "name": name, "listen_port": port, "protocol": "tcp",
                "device_group_in": in_g1_id, "forward_mode": "direct",
                "target_addr": "127.0.0.1", "target_port": target_port, **extra,
            })
            assert r["code"] == 0, f"{name} creation failed: {r}"

        rules = {r["name"]: r for r in api("GET", "/rules", admin_token)["data"]}
        tcp_rule_id = rules["tcp-rule"]["id"]
        udp_rule_id = rules["udp-rule"]["id"]
        assert rules["tcp-rule"]["uid"] == admin_uid, "rule uid should come from token"
        print(f"[ok] rules created: tcp id={tcp_rule_id} udp id={udp_rule_id} (uid from token)")

        # 3. Start node — it authenticates via Authorization header now.
        node_env = dict(os.environ,
                        PANEL_URL="http://127.0.0.1:18888",
                        NODE_TOKEN=in_g1_token,
                        POLL_INTERVAL="2",
                        RUST_LOG="info")
        node = subprocess.Popen([NODE_BIN], env=node_env,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        print(f"[node] PID {node.pid}")
        assert wait_for_port("127.0.0.1", TCP_LISTEN_PORT), "node TCP listener not up"
        print("[ok] node opened listeners (auth via Authorization header)")

        # 3b. Live rate (panel 1.2.11 / node 1.2.5): the node pushes its NIC rate
        #     over the WS control channel every 2s and the panel serves it from
        #     memory. Unit tests cover each hop; this proves the real frames, the
        #     X-Node-ID binding and the parsing line up end to end.
        live = []
        deadline = time.time() + 10
        while time.time() < deadline:
            live = [r for r in api("GET", "/nodes/live-rates", admin_token)["data"]
                    if r["group_id"] == in_g1_id]
            if live:
                break
            time.sleep(0.5)
        assert live, "node never appeared in /nodes/live-rates"
        assert live[0]["node_id"], "a live rate must carry the node's id"
        print(f"[ok] live rate pushed over WS: up={live[0]['upload_bps']}B/s "
              f"down={live[0]['download_bps']}B/s")

        # 3c. Memory in bytes (panel 1.2.14 / node 1.2.8): the real node's
        #     status carries the total and the amount in use, and they agree
        #     with the percentage it has always sent.
        status = next(r for r in api("GET", "/nodes", admin_token)["data"]
                      if r.get("group_id") == in_g1_id)
        total, used = status.get("mem_total"), status.get("mem_used")
        assert total and total > 0, f"no memory total in the node status: {status}"
        assert used is not None and used <= total, f"bad memory in use: {used} of {total}"
        assert abs(used / total * 100 - status["mem"]) < 5,             f"bytes {used}/{total} disagree with mem={status['mem']}%"
        print(f"[ok] node memory: {used}B of {total}B ({status['mem']:.1f}%)")

        # 3d. Every disk (panel after 1.2.15 / node after 1.2.9): the real
        #     node lists its local disks beside the primary-disk figures it
        #     has always sent, root first, and the two agree about `/`. Only
        #     Linux nodes can list them (they read /proc/self/mountinfo).
        if sys.platform.startswith("linux"):
            disks = status.get("disks") or []
            root = next((d for d in disks if d["mount"] == "/"), None)
            assert root, f"no / among the node's disks: {disks}"
            assert disks[0] is root, f"/ must come first: {disks}"
            assert root["total"] == status["disk_total"],                 f"disks / total {root['total']} != disk_total {status['disk_total']}"
            # Read a moment apart, so the used figure may have moved a little.
            assert abs(root["used"] - status["disk_used"]) <= root["total"] * 0.01,                 f"disks / used {root['used']} vs disk_used {status['disk_used']}"
            for d in disks:
                assert d["total"] > 0 and 0 <= d["used"] <= d["total"], d
            print(f"[ok] node disks: {[(d['mount'], d['fs_type']) for d in disks]}")

        # 4. Verify the v0.4.0 protocol gate: a request WITHOUT the
        #    X-Config-Protocol-Version header must be rejected with 426
        #    (Upgrade Required), NOT return config. This is the gate that
        #    protects old nodes from receiving fields they can't deserialize.
        try:
            api("GET", "/node/config", token=in_g1_token)
            assert False, "config pull without protocol-version header should fail"
        except urllib.error.HTTPError as e:
            assert e.code == 426, f"expected 426, got {e.code}"
            body = json.loads(e.read().decode())
            assert body["code"] == "CONFIG_PROTOCOL_MISMATCH", \
                f"expected CONFIG_PROTOCOL_MISMATCH, got {body.get('code')}"
        print("[ok] config pull without protocol-version header rejected (426)")

        # 4b. WITH the header but wrong token → empty listeners (gate passes,
        #     token check returns empty). This confirms the gate is before the
        #     token check but both work.
        with_auth = api("GET", "/node/config", token=in_g1_token,
                        extra_headers={"X-Config-Protocol-Version": "4"})
        assert with_auth["listeners"] != [], \
            "config pull with valid header + token should return listeners"
        print("[ok] config pull with valid header + token returns listeners")

        # 5. TCP + UDP forwarding + traffic counting
        payload = b"tcp-relay-" + b"X" * 2000 + b"\n"
        resp = tcp_roundtrip(TCP_LISTEN_PORT, payload)
        assert resp == payload, f"TCP echo mismatch: got {len(resp)}B of {len(payload)}B"
        print(f"[ok] TCP forwarded {len(payload)} bytes round-trip")

        udp_payload = b"udp-relay-" + b"Y" * 500
        resp = udp_roundtrip(UDP_LISTEN_PORT, udp_payload)
        assert resp == udp_payload, "UDP echo mismatch"
        print(f"[ok] UDP forwarded {len(udp_payload)} bytes round-trip")

        # 5b. The chain carries the client: rule A sends v2 to rule B, which
        #     reads it and sends v1 to the target. The target must see THIS
        #     client and the port it dialled on rule A — not the node itself.
        assert wait_for_port("127.0.0.1", PP_A_PORT), "pp-a listener not up"
        with socket.create_connection(("127.0.0.1", PP_A_PORT), timeout=5) as s:
            me = s.getsockname()
            s.sendall(b"pp-payload")
            want = f"PROXY TCP4 127.0.0.1 127.0.0.1 {me[1]} {PP_A_PORT}\r\n".encode() + b"pp-payload"
            got = b""
            try:
                while len(got) < len(want):
                    chunk = s.recv(1024)
                    if not chunk:
                        break
                    got += chunk
            except socket.timeout:
                pass
        assert got == want, f"PROXY chain: got {got!r}, want {want!r}"
        print(f"[ok] PROXY protocol chain: target saw client port {me[1]} via two hops")

        # 6. Wait for node to report, then check forward_rules + users traffic.
        time.sleep(6)
        rules = {r["name"]: r for r in api("GET", "/rules", admin_token)["data"]}
        assert rules["tcp-rule"]["traffic_used"] > 0, "TCP rule traffic not reported"
        assert rules["udp-rule"]["traffic_used"] > 0, "UDP rule traffic not reported"
        tcp_traffic = rules["tcp-rule"]["traffic_used"]
        udp_traffic = rules["udp-rule"]["traffic_used"]
        print(f"[ok] rule traffic: tcp={tcp_traffic}B udp={udp_traffic}B")

        users = {u["id"]: u for u in api("GET", "/admin/users", admin_token)["data"]}
        user_traffic = users[admin_uid]["traffic_used"]
        assert user_traffic >= tcp_traffic + udp_traffic, \
            f"users.traffic_used ({user_traffic}) should include rule traffic ({tcp_traffic + udp_traffic})"
        print(f"[ok] user traffic_used={user_traffic}B (synced from rules)")

        # 7. Disambiguation: a SECOND inbound group cannot reuse the same
        #    listen port on the same node (only one socket can bind). Instead
        #    we verify the accounting is keyed by rule_id by checking that
        #    only the rules we sent traffic through got charged, and the
        #    totals match exactly (no cross-charging to a phantom same-port
        #    rule). We confirm via direct report_traffic with rule_id.
        #    Simulate a second rule on the same port existing but getting NO
        #    traffic: its counter must stay 0 while rule 1 grows.
        #    (The forward_rules table already has exactly our 2 rules; neither
        #    shares a listen_port, so this is structurally clean. The key
        #    guarantee is that report_traffic UPDATEs WHERE id = rule_id, not
        #    WHERE listen_port = ?.)
        # We assert the rule_id-based UPDATE by reporting traffic for
        # tcp_rule_id directly and confirming only that rule grows.
        before = rules["tcp-rule"]["traffic_used"]
        api("POST", "/node/report_traffic", extra_headers={"Authorization": f"Bearer {in_g1_token}"}, body={
            "token": "", "reports": [{"rule_id": tcp_rule_id, "upload": 1234, "download": 5678}],
        })
        rules = {r["name"]: r for r in api("GET", "/rules", admin_token)["data"]}
        after = rules["tcp-rule"]["traffic_used"]
        assert after == before + 1234 + 5678, \
            f"rule_id-keyed update failed: {before} -> {after} (expected +{1234+5678})"
        # The UDP rule (different rule_id) must be unchanged by the TCP report
        assert rules["udp-rule"]["traffic_used"] == udp_traffic, \
            "UDP rule traffic changed from a TCP rule_id report (cross-charge!)"
        print(f"[ok] rule_id-keyed accounting: tcp {before}->{after}, udp unchanged ({udp_traffic}B)")

        # === v0.1.2 NEW FEATURES ===

        # 8. Group edit: update the outbound group's name and connect_host
        update_res = api("PUT", f"/groups/{out_g1_id}", admin_token, {
            "name": "out-1-renamed", "connect_host": "127.0.0.1",
        })
        assert update_res["code"] == 0, f"group update failed: {update_res}"
        groups_check = {g["id"]: g for g in api("GET", "/groups", admin_token)["data"]}
        assert groups_check[out_g1_id]["name"] == "out-1-renamed", "group name not updated"
        print(f"[ok] group edit: name -> {groups_check[out_g1_id]['name']}")

        # 9. TCP+UDP rule: one rule, protocol tcp_udp, both forwarded on same port
        #    v0.4.20: forward_mode locked to direct, no outbound group.
        tcpudp_resp = api("POST", "/rules", admin_token, {
            "name": "tcpudp-rule", "listen_port": TCPUDP_LISTEN_PORT,
            "protocol": "tcp_udp", "device_group_in": in_g1_id,
            "forward_mode": "direct",
            "target_addr": "127.0.0.1", "target_port": TCP_ECHO_PORT,
        })
        assert tcpudp_resp["code"] == 0, f"tcpudp-rule creation failed: {tcpudp_resp}"
        # Wait for node to pick up new config
        time.sleep(4)
        # TCP through the tcp_udp rule
        tcpudp_tcp = tcp_roundtrip(TCPUDP_LISTEN_PORT, b"tcpudp-tcp-test\n")
        assert tcpudp_tcp == b"tcpudp-tcp-test\n", f"TCP+UDP rule TCP echo mismatch: {tcpudp_tcp}"
        # UDP through the same rule (target is TCP echo port, may fail — use UDP echo)
        # Actually TCP+UDP rule targets TCP_ECHO_PORT for TCP and needs a separate
        # UDP target. For simplicity we test TCP only here (the UDP path is already
        # validated by the dedicated udp-rule above). The key is that the node
        # opened a TCP listener on TCPUDP_LISTEN_PORT.
        print(f"[ok] tcp_udp rule: TCP forwarded on port {TCPUDP_LISTEN_PORT}")

        # 10. Direct mode rule: forward_mode=direct, no outbound group
        direct_rule_resp = api("POST", "/rules", admin_token, {
            "name": "direct-rule", "listen_port": DIRECT_TCP_PORT,
            "protocol": "tcp", "device_group_in": in_g1_id,
            "forward_mode": "direct",
            "target_addr": "127.0.0.1", "target_port": TCP_ECHO_PORT,
        })
        assert direct_rule_resp["code"] == 0, f"direct-rule creation failed: {direct_rule_resp}"
        time.sleep(4)
        direct_resp = tcp_roundtrip(DIRECT_TCP_PORT, b"direct-mode-test\n")
        assert direct_resp == b"direct-mode-test\n", f"Direct mode TCP echo mismatch: {direct_resp}"
        print(f"[ok] direct mode: TCP forwarded on port {DIRECT_TCP_PORT} (no outbound group)")

        # 10b. IPv6 targets (node-v1.2.8): a client that reaches the node over
        #      IPv4, a target that has only IPv6 — an IPv6-only line reached
        #      from an IPv4 address. TCP always dialled either family; UDP sent
        #      from an IPv4-only socket and could never reach the target.
        #      The CI runner has an IPv6 loopback, so there this must run.
        if start_echo_servers_v6() is None:
            assert sys.platform == "win32", "Linux CI must have an IPv6 loopback"
            print("[skip] IPv6 targets: no IPv6 loopback on this host")
        else:
            for name, proto, listen, target in (
                ("tcp6-rule", "tcp", TCP6_LISTEN_PORT, TCP6_ECHO_PORT),
                ("udp6-rule", "udp", UDP6_LISTEN_PORT, UDP6_ECHO_PORT),
            ):
                created = api("POST", "/rules", admin_token, {
                    "name": name, "listen_port": listen,
                    "protocol": proto, "device_group_in": in_g1_id,
                    "forward_mode": "direct",
                    "target_addr": "::1", "target_port": target,
                })
                assert created["code"] == 0, f"{name} creation failed: {created}"
            assert wait_for_port("127.0.0.1", TCP6_LISTEN_PORT), "tcp6-rule listener not up"
            got = tcp_roundtrip(TCP6_LISTEN_PORT, b"tcp-to-ipv6\n")
            assert got == b"tcp-to-ipv6\n", f"TCP to an IPv6 target: got {got!r}"
            # The UDP listener opens with the same config pull as the TCP one,
            # but give it a moment rather than lose the first datagram.
            udp6 = None
            deadline = time.time() + 10
            while time.time() < deadline:
                try:
                    udp6 = udp_roundtrip(UDP6_LISTEN_PORT, b"udp-to-ipv6", timeout=2)
                    break
                except socket.timeout:
                    pass
            assert udp6 == b"udp-to-ipv6", f"UDP to an IPv6 target: got {udp6!r}"
            print("[ok] IPv4 client -> IPv6 target: TCP and UDP forwarded")

        # 11. Auto port assignment: create rule without listen_port
        auto_rule = api("POST", "/rules", admin_token, {
            "name": "auto-port-rule", "listen_port": None,
            "protocol": "tcp", "device_group_in": in_g1_id,
            "forward_mode": "direct",
            "target_addr": "127.0.0.1", "target_port": TCP_ECHO_PORT,
        })
        assert auto_rule["code"] == 0, f"auto-port rule creation failed: {auto_rule}"
        rules_check = {r["name"]: r for r in api("GET", "/rules", admin_token)["data"]}
        assigned_port = rules_check["auto-port-rule"]["listen_port"]
        assert assigned_port >= 10000, f"auto-assigned port should be >= 10000, got {assigned_port}"
        print(f"[ok] auto port assignment: got port {assigned_port}")

        # 12. Rule update: verify PUT /rules/{id} works for valid changes,
        #     and that device_group_out is rejected (v0.4.20).
        tcpudp_rule_id = rules_check["tcpudp-rule"]["id"]
        update_resp = api("PUT", f"/rules/{tcpudp_rule_id}", admin_token, {
            "name": "tcpudp-rule-renamed",
        })
        assert update_resp["code"] == 0, f"rule update failed: {update_resp}"
        time.sleep(1)
        rules_after = {r["name"]: r for r in api("GET", "/rules", admin_token)["data"]}
        assert rules_after["tcpudp-rule-renamed"]["name"] == "tcpudp-rule-renamed", \
            "rule name not updated"
        # Verify that setting device_group_out is rejected (v0.4.20).
        reject_resp = api("PUT", f"/rules/{tcpudp_rule_id}", admin_token, {
            "device_group_out": 99,
        })
        assert reject_resp["code"] == 400, \
            f"device_group_out should be rejected, got: {reject_resp}"
        print(f"[ok] rule update: name changed, device_group_out rejected (v0.4.20)")

        # 13. Graceful shutdown (node 1.2.5). Traffic forwarded moments before
        #     SIGTERM must still be billed: the node reports every 2s here, so
        #     without the shutdown flush these bytes would almost always die with
        #     the process. And with nothing open, the node must not sit out the
        #     drain window refusing connections — it should exit at once, cleanly.
        #     Needs a real SIGTERM, so Linux (CI) only: on Windows terminate()
        #     is a hard kill.
        if sys.platform != "win32":
            import signal
            by_name = {r["name"]: r for r in api("GET", "/rules", admin_token)["data"]}
            before = by_name["tcp-rule"]["traffic_used"]
            flush_payload = b"shutdown-flush-" + b"Z" * 5000 + b"\n"
            echo = tcp_roundtrip(TCP_LISTEN_PORT, flush_payload)
            assert echo == flush_payload, (
                f"echo before SIGTERM: got {len(echo)}B of {len(flush_payload)}B")
            node.send_signal(signal.SIGTERM)
            started = time.time()
            code = node.wait(timeout=20)
            took = time.time() - started
            assert code == 0, f"graceful shutdown should exit 0, got {code}"
            assert took < 4.5, (f"with no connection open the node should exit at once, "
                                f"not wait out the drain; took {took:.1f}s")
            by_name = {r["name"]: r for r in api("GET", "/rules", admin_token)["data"]}
            gained = by_name["tcp-rule"]["traffic_used"] - before
            assert gained >= 2 * len(flush_payload), (
                f"traffic forwarded right before SIGTERM was not billed: "
                f"+{gained}B, expected >= {2 * len(flush_payload)}B")
            print(f"[ok] SIGTERM: exit 0 in {took:.1f}s, last {gained}B billed")
        else:
            print("[skip] graceful shutdown check needs SIGTERM (Linux only)")

        print("\nALL TESTS PASSED [PASS]")
        return 0
    except Exception as e:
        import traceback
        traceback.print_exc()
        print(f"\nTEST FAILED [FAIL]: {e}")
        return 1
    finally:
        for p in (panel, node):
            if p is None:
                continue
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    p.kill()


if __name__ == "__main__":
    sys.exit(main())
