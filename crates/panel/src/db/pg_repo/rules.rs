use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::ForwardRule;

// ── RuleRepository ──

#[async_trait]
impl RuleRepository for PgRepository {
    async fn list_rules(&self, scope: &ResourceScope) -> Result<Vec<ForwardRule>, DbError> {
        let mut rules: Vec<ForwardRule> = match scope.owner_id() {
            None => sqlx::query_as("SELECT * FROM forward_rules ORDER BY id"),
            Some(uid) => {
                sqlx::query_as("SELECT * FROM forward_rules WHERE uid = $1 ORDER BY id").bind(uid)
            }
        }
        .fetch_all(&self.pool)
        .await?;
        for rule in &mut rules {
            rule.targets = self.list_rule_targets(rule.id, scope).await?;
        }
        Ok(rules)
    }

    async fn find_rule_by_id(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<ForwardRule>, DbError> {
        let mut rule: Option<ForwardRule> = match scope.owner_id() {
            None => sqlx::query_as("SELECT * FROM forward_rules WHERE id = $1").bind(rule_id),
            Some(uid) => sqlx::query_as("SELECT * FROM forward_rules WHERE id = $1 AND uid = $2")
                .bind(rule_id)
                .bind(uid),
        }
        .fetch_optional(&self.pool)
        .await?;
        if let Some(r) = &mut rule {
            r.targets = self.list_rule_targets(r.id, scope).await?;
        }
        Ok(rule)
    }

    async fn list_rule_targets(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
    ) -> Result<Vec<relay_shared::models::ForwardRuleTarget>, DbError> {
        let targets = match scope.owner_id() {
            None => sqlx::query_as(
                "SELECT * FROM forward_rule_targets WHERE rule_id = $1 ORDER BY position, id",
            )
            .bind(rule_id),
            Some(uid) => sqlx::query_as(
                "SELECT * FROM forward_rule_targets WHERE rule_id = $1 AND EXISTS \
                 (SELECT 1 FROM forward_rules WHERE id = forward_rule_targets.rule_id AND uid = $2) \
                 ORDER BY position, id",
            )
            .bind(rule_id)
            .bind(uid),
        }
        .fetch_all(&self.pool)
        .await?;
        Ok(targets)
    }

    async fn list_enabled_rule_targets(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
    ) -> Result<Vec<relay_shared::models::ForwardRuleTarget>, DbError> {
        let targets = match scope.owner_id() {
            None => sqlx::query_as(
                "SELECT * FROM forward_rule_targets WHERE rule_id = $1 AND enabled = TRUE ORDER BY position, id",
            )
            .bind(rule_id),
            Some(uid) => sqlx::query_as(
                "SELECT * FROM forward_rule_targets WHERE rule_id = $1 AND enabled = TRUE AND EXISTS \
                 (SELECT 1 FROM forward_rules WHERE id = forward_rule_targets.rule_id AND uid = $2) \
                 ORDER BY position, id",
            )
            .bind(rule_id)
            .bind(uid),
        }
        .fetch_all(&self.pool)
        .await?;
        Ok(targets)
    }

    async fn replace_rule_targets(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
        targets: &[relay_shared::protocol::RuleTargetRequest],
    ) -> Result<(), DbError> {
        // Scope guard: under Owner scope, no-op unless the rule is owned by uid.
        if let Some(uid) = scope.owner_id() {
            let owned: Option<(i32,)> =
                sqlx::query_as("SELECT 1 FROM forward_rules WHERE id = $1 AND uid = $2")
                    .bind(rule_id)
                    .bind(uid)
                    .fetch_optional(&self.pool)
                    .await?;
            if owned.is_none() {
                return Ok(());
            }
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM forward_rule_targets WHERE rule_id = $1")
            .bind(rule_id)
            .execute(&mut *tx)
            .await?;
        for (idx, target) in targets.iter().enumerate() {
            sqlx::query(
                "INSERT INTO forward_rule_targets (rule_id, host, port, position, enabled) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(rule_id)
            .bind(target.host.trim())
            .bind(target.port as i32)
            .bind(idx as i32 + 1)
            .bind(target.enabled)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn set_rule_load_balance_strategy(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
        strategy: &str,
    ) -> Result<u64, DbError> {
        let result = match scope.owner_id() {
            None => {
                sqlx::query("UPDATE forward_rules SET load_balance_strategy = $1 WHERE id = $2")
                    .bind(strategy)
                    .bind(rule_id)
            }
            Some(uid) => sqlx::query(
                "UPDATE forward_rules SET load_balance_strategy = $1 WHERE id = $2 AND uid = $3",
            )
            .bind(strategy)
            .bind(rule_id)
            .bind(uid),
        }
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn set_rule_rate_limits(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
        upload_limit_mbps: i32,
        download_limit_mbps: i32,
    ) -> Result<u64, DbError> {
        let result = match scope.owner_id() {
            None => sqlx::query(
                "UPDATE forward_rules SET upload_limit_mbps = $1, download_limit_mbps = $2 WHERE id = $3",
            )
            .bind(upload_limit_mbps)
            .bind(download_limit_mbps)
            .bind(rule_id),
            Some(uid) => sqlx::query(
                "UPDATE forward_rules SET upload_limit_mbps = $1, download_limit_mbps = $2 \
                 WHERE id = $3 AND uid = $4",
            )
            .bind(upload_limit_mbps)
            .bind(download_limit_mbps)
            .bind(rule_id)
            .bind(uid),
        }
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn set_rule_tunnel_profile(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
        profile_id: Option<i64>,
    ) -> Result<u64, DbError> {
        let result = match scope.owner_id() {
            None => sqlx::query("UPDATE forward_rules SET tunnel_profile_id = $1 WHERE id = $2")
                .bind(profile_id)
                .bind(rule_id),
            Some(uid) => sqlx::query(
                "UPDATE forward_rules SET tunnel_profile_id = $1 WHERE id = $2 AND uid = $3",
            )
            .bind(profile_id)
            .bind(rule_id)
            .bind(uid),
        }
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn list_group_port_protocols(
        &self,
        device_group_in: i64,
    ) -> Result<Vec<(i32, String)>, DbError> {
        let rows: Vec<(i32, String)> = sqlx::query_as(
            "SELECT listen_port, protocol FROM forward_rules WHERE device_group_in = $1",
        )
        .bind(device_group_in)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn count_by_uid(&self, uid: i64) -> Result<i64, DbError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM forward_rules WHERE uid = $1")
            .bind(uid)
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    async fn max_rules_for_uid(&self, uid: i64) -> Result<i32, DbError> {
        let max_rules: i32 =
            sqlx::query_scalar("SELECT COALESCE(max_rules, 0) FROM users WHERE id = $1")
                .bind(uid)
                .fetch_one(&self.pool)
                .await?;
        Ok(max_rules)
    }

    async fn insert_quota_guarded(
        &self,
        name: &str,
        uid: i64,
        listen_port: i32,
        protocol: &str,
        public_transport: &str,
        node_transport: &str,
        route_mode: &str,
        entry_transport: &str,
        ws_path: Option<&str>,
        device_group_in: i64,
        device_group_out: Option<i64>,
        forward_mode: &str,
        target_addr: &str,
        target_port: i32,
    ) -> Result<u64, DbError> {
        // v0.4.4: serialize concurrent inserts for the SAME user with a row lock.
        // PostgreSQL's MVCC means two concurrent transactions can both read the
        // same COUNT(*) and both pass the quota check, overshooting max_rules.
        // SQLite avoids this via single-writer serialization; PG needs an
        // explicit lock. We take `SELECT ... FOR UPDATE` on the user row inside a
        // transaction, so a second creator for the same uid blocks until the
        // first commits and then re-evaluates the (now-updated) count.
        //
        // v0.4.11 PR4: ALSO serialize concurrent inserts for the SAME inbound
        // group with a per-group advisory xact lock, so the port-conflict
        // pre-check below is race-safe (two users sharing one group can't both
        // pass the check for the same port). Lock order is ALWAYS advisory(group)
        // then row(user) across every caller, so no deadlock cycle can form.
        //
        // The INSERT keeps its own WHERE guard as the authoritative quota check
        // and the partial unique indexes are the authoritative port backstop;
        // the locks only make the reads stable for the duration.
        let needs_tcp = matches!(protocol, "tcp" | "tcp_udp");
        let needs_udp = matches!(protocol, "udp" | "tcp_udp");

        let mut tx = self.pool.begin().await?;

        // Per-group advisory lock (released automatically at tx end).
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(device_group_in)
            .execute(&mut *tx)
            .await?;

        // Lock the user row. If the user doesn't exist, there's nothing to lock
        // and the INSERT's FK on uid would fail anyway — let it surface naturally.
        sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
            .bind(uid)
            .fetch_optional(&mut *tx)
            .await?;

        // Port-conflict pre-check: same inbound group + same port + an
        // overlapping socket type. A pure-TCP and a pure-UDP rule do NOT conflict.
        let conflict: Result<Option<(i32,)>, sqlx::Error> = sqlx::query_as(
            "SELECT 1 FROM forward_rules \
             WHERE device_group_in = $1 AND listen_port = $2 \
               AND ( ($3 AND protocol IN ('tcp', 'tcp_udp')) \
                  OR ($4 AND protocol IN ('udp', 'tcp_udp')) ) \
             LIMIT 1",
        )
        .bind(device_group_in)
        .bind(listen_port)
        .bind(needs_tcp)
        .bind(needs_udp)
        .fetch_optional(&mut *tx)
        .await;
        match conflict {
            Ok(Some(_)) => {
                let _ = tx.rollback().await;
                return Err(DbError::PortConflict);
            }
            Ok(None) => {}
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(e.into());
            }
        }

        // 14 row-value binds ($1..$14) + 3 uid binds for the WHERE subqueries
        // ($15..$17). Same atomic quota-guarded INSERT shape as SQLite.
        let result = sqlx::query(
            "INSERT INTO forward_rules \
               (name, uid, listen_port, protocol, public_transport, node_transport, \
                route_mode, entry_transport, ws_path, \
                device_group_in, device_group_out, forward_mode, target_addr, target_port) \
             SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14 \
             WHERE (SELECT max_rules FROM users WHERE id = $15) = 0 \
                OR (SELECT COUNT(*) FROM forward_rules WHERE uid = $16) \
                   < (SELECT max_rules FROM users WHERE id = $17)",
        )
        .bind(name)
        .bind(uid)
        .bind(listen_port)
        .bind(protocol)
        .bind(public_transport)
        .bind(node_transport)
        .bind(route_mode)
        .bind(entry_transport)
        .bind(ws_path)
        .bind(device_group_in)
        .bind(device_group_out)
        .bind(forward_mode)
        .bind(target_addr)
        .bind(target_port)
        .bind(uid)
        .bind(uid)
        .bind(uid)
        .execute(&mut *tx)
        .await;

        match result {
            Ok(r) => {
                tx.commit().await?;
                Ok(r.rows_affected())
            }
            Err(e) => {
                // Roll back so the user-row lock is released; map the error
                // (e.g. UNIQUE on listen_port) the same way as elsewhere.
                let _ = tx.rollback().await;
                Err(e.into())
            }
        }
    }

    async fn create_rule_full(
        &self,
        name: &str,
        uid: i64,
        listen_port: i32,
        protocol: &str,
        public_transport: &str,
        node_transport: &str,
        route_mode: &str,
        entry_transport: &str,
        ws_path: Option<&str>,
        device_group_in: i64,
        device_group_out: Option<i64>,
        forward_mode: &str,
        target_addr: &str,
        target_port: i32,
        targets: &[relay_shared::protocol::RuleTargetRequest],
        load_balance_strategy: &str,
        upload_limit_mbps: i32,
        download_limit_mbps: i32,
        tunnel_profile_id: Option<i64>,
    ) -> Result<Option<i64>, DbError> {
        // v1.2: atomic create. Same advisory-xact-lock + user-row FOR UPDATE +
        // conflict pre-check + quota-guarded INSERT shape as
        // insert_quota_guarded, but the INSERT is a RETURNING-id, followed by
        // the targets / LB / rate-limit / tunnel writes INSIDE the same tx.
        // The new id comes from RETURNING (not a post-commit listen_port
        // re-lookup), so two inbound groups reusing a port can't cross-write.

        let needs_tcp = matches!(protocol, "tcp" | "tcp_udp");
        let needs_udp = matches!(protocol, "udp" | "tcp_udp");

        let mut tx = self.pool.begin().await?;

        // Convert any sqlx error into a rollback + DbError early return so the
        // body stays linear. Evaluates to `!`, so `try_!(expr)` is well-typed
        // for any sqlx::Error-producing statement.
        macro_rules! try_ {
            ($tx:expr, $expr:expr) => {
                match $expr {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = $tx.rollback().await;
                        return Err(DbError::from(e));
                    }
                }
            };
        }

        // Lock order: advisory(group) then row(user) — identical to
        // insert_quota_guarded so no deadlock cycle can form.
        try_!(
            tx,
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(device_group_in)
                .execute(&mut *tx)
                .await
        );

        try_!(
            tx,
            sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
                .bind(uid)
                .fetch_optional(&mut *tx)
                .await
        );

        let conflict: Option<(i32,)> = try_!(
            tx,
            sqlx::query_as(
                "SELECT 1 FROM forward_rules \
                 WHERE device_group_in = $1 AND listen_port = $2 \
                   AND ( ($3 AND protocol IN ('tcp', 'tcp_udp')) \
                      OR ($4 AND protocol IN ('udp', 'tcp_udp')) ) \
                 LIMIT 1",
            )
            .bind(device_group_in)
            .bind(listen_port)
            .bind(needs_tcp)
            .bind(needs_udp)
            .fetch_optional(&mut *tx)
            .await
        );
        if conflict.is_some() {
            let _ = tx.rollback().await;
            return Err(DbError::PortConflict);
        }

        // RETURNING id so we get the new row's id directly. The WHERE quota
        // guard is the same as SQLite's; when it matches 0 rows the SELECT
        // yields no id → Option<i64> None → quota exhausted.
        let rule_id: Option<i64> = try_!(
            tx,
            sqlx::query_scalar(
                "INSERT INTO forward_rules \
                   (name, uid, listen_port, protocol, public_transport, node_transport, \
                    route_mode, entry_transport, ws_path, \
                    device_group_in, device_group_out, forward_mode, target_addr, target_port) \
                 SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14 \
                 WHERE (SELECT max_rules FROM users WHERE id = $15) = 0 \
                    OR (SELECT COUNT(*) FROM forward_rules WHERE uid = $16) \
                       < (SELECT max_rules FROM users WHERE id = $17) \
                 RETURNING id",
            )
            .bind(name)
            .bind(uid)
            .bind(listen_port)
            .bind(protocol)
            .bind(public_transport)
            .bind(node_transport)
            .bind(route_mode)
            .bind(entry_transport)
            .bind(ws_path)
            .bind(device_group_in)
            .bind(device_group_out)
            .bind(forward_mode)
            .bind(target_addr)
            .bind(target_port)
            .bind(uid)
            .bind(uid)
            .bind(uid)
            .fetch_optional(&mut *tx)
            .await
        );

        let Some(rule_id) = rule_id else {
            // Quota exhausted — nothing inserted. Commit the empty tx.
            tx.commit().await?;
            return Ok(None);
        };

        try_!(
            tx,
            sqlx::query("DELETE FROM forward_rule_targets WHERE rule_id = $1")
                .bind(rule_id)
                .execute(&mut *tx)
                .await
        );
        for (idx, target) in targets.iter().enumerate() {
            try_!(
                tx,
                sqlx::query(
                    "INSERT INTO forward_rule_targets (rule_id, host, port, position, enabled) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(rule_id)
                .bind(target.host.trim())
                .bind(target.port as i32)
                .bind(idx as i32 + 1)
                .bind(target.enabled)
                .execute(&mut *tx)
                .await
            );
        }

        if load_balance_strategy != "first" {
            try_!(
                tx,
                sqlx::query("UPDATE forward_rules SET load_balance_strategy = $1 WHERE id = $2")
                    .bind(load_balance_strategy)
                    .bind(rule_id)
                    .execute(&mut *tx)
                    .await
            );
        }

        if upload_limit_mbps != 0 || download_limit_mbps != 0 {
            try_!(
                tx,
                sqlx::query(
                    "UPDATE forward_rules SET upload_limit_mbps = $1, download_limit_mbps = $2 \
                     WHERE id = $3",
                )
                .bind(upload_limit_mbps)
                .bind(download_limit_mbps)
                .bind(rule_id)
                .execute(&mut *tx)
                .await
            );
        }

        if let Some(pid) = tunnel_profile_id {
            try_!(
                tx,
                sqlx::query("UPDATE forward_rules SET tunnel_profile_id = $1 WHERE id = $2")
                    .bind(pid)
                    .bind(rule_id)
                    .execute(&mut *tx)
                    .await
            );
        }

        tx.commit().await?;
        Ok(Some(rule_id))
    }

    async fn find_transport_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<(String, String)>, DbError> {
        let row: Option<(String, String)> = match scope.owner_id() {
            None => {
                sqlx::query_as("SELECT protocol, public_transport FROM forward_rules WHERE id = $1")
                    .bind(id)
            }
            Some(uid) => sqlx::query_as(
                "SELECT protocol, public_transport FROM forward_rules WHERE id = $1 AND uid = $2",
            )
            .bind(id)
            .bind(uid),
        }
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_device_group_out_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<Option<i64>>, DbError> {
        let row: Option<(Option<i64>,)> = match scope.owner_id() {
            None => {
                sqlx::query_as("SELECT device_group_out FROM forward_rules WHERE id = $1").bind(id)
            }
            Some(uid) => sqlx::query_as(
                "SELECT device_group_out FROM forward_rules WHERE id = $1 AND uid = $2",
            )
            .bind(id)
            .bind(uid),
        }
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(v,)| v))
    }

    async fn update_rule_fields(
        &self,
        id: i64,
        scope: &ResourceScope,
        name: Option<&str>,
        listen_port: Option<i32>,
        protocol: Option<&str>,
        public_transport: Option<&str>,
        node_transport: Option<&str>,
        entry_transport: Option<&str>,
        route_mode: Option<&str>,
        ws_path: Option<Option<&str>>,
        device_group_in: Option<i64>,
        device_group_out: Option<Option<i64>>,
        forward_mode: Option<&str>,
        target_addr: Option<&str>,
        target_port: Option<i32>,
        paused: Option<bool>,
    ) -> Result<u64, DbError> {
        // Same field-order logic as SQLite. PG needs numbered placeholders;
        // we walk the SET list twice — once to know which fields are present,
        // once to bind them in order.
        let mut sets: Vec<&str> = Vec::new();
        if name.is_some() {
            sets.push("name = ");
        }
        if listen_port.is_some() {
            sets.push("listen_port = ");
        }
        if protocol.is_some() {
            sets.push("protocol = ");
        }
        if public_transport.is_some() {
            sets.push("public_transport = ");
            sets.push("node_transport = ");
            sets.push("entry_transport = ");
        }
        if route_mode.is_some() {
            sets.push("route_mode = ");
        }
        if ws_path.is_some() {
            sets.push("ws_path = ");
        }
        if device_group_in.is_some() {
            sets.push("device_group_in = ");
        }
        if device_group_out.is_some() {
            sets.push("device_group_out = ");
        }
        if forward_mode.is_some() {
            sets.push("forward_mode = ");
        }
        if target_addr.is_some() {
            sets.push("target_addr = ");
        }
        if target_port.is_some() {
            sets.push("target_port = ");
        }
        if paused.is_some() {
            sets.push("paused = ");
            // v1.0.8: an explicit paused write is always a human action (the
            // on/off switch, batch pause/resume) — clear auto_paused so a later
            // buy_plan re-authorization doesn't treat this rule as something IT
            // needs to reconcile.
            sets.push("auto_paused = ");
        }

        if sets.is_empty() {
            return Ok(0);
        }

        // Number placeholders. id is the next bind; uid (if scoped) is after it.
        let mut ph = 1;
        let sets_with_ph: Vec<String> = sets
            .iter()
            .map(|s| {
                let p = format!("{s}${ph}");
                ph += 1;
                p
            })
            .collect();
        let id_ph = ph;
        let uid_ph = ph + 1;
        let sql = match scope.owner_id() {
            None => format!(
                "UPDATE forward_rules SET {} WHERE id = ${}",
                sets_with_ph.join(", "),
                id_ph
            ),
            Some(_) => format!(
                "UPDATE forward_rules SET {} WHERE id = ${} AND uid = ${}",
                sets_with_ph.join(", "),
                id_ph,
                uid_ph
            ),
        };

        let mut q = sqlx::query(&sql);
        if let Some(v) = name {
            q = q.bind(v);
        }
        if let Some(v) = listen_port {
            q = q.bind(v);
        }
        if let Some(v) = protocol {
            q = q.bind(v);
        }
        if let Some(v) = public_transport {
            q = q.bind(v);
            q = q.bind(node_transport.unwrap_or(v));
            q = q.bind(entry_transport.unwrap_or(v));
        }
        if let Some(v) = route_mode {
            q = q.bind(v);
        }
        if let Some(v) = ws_path {
            q = q.bind(v);
        }
        if let Some(v) = device_group_in {
            q = q.bind(v);
        }
        if let Some(v) = device_group_out {
            q = q.bind(v);
        }
        if let Some(v) = forward_mode {
            q = q.bind(v);
        }
        if let Some(v) = target_addr {
            q = q.bind(v);
        }
        if let Some(v) = target_port {
            q = q.bind(v);
        }
        if let Some(v) = paused {
            q = q.bind(v);
            q = q.bind(false); // auto_paused reset
        }
        q = q.bind(id);
        if let Some(uid) = scope.owner_id() {
            q = q.bind(uid);
        }

        let result = q.execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    async fn increment_rule_traffic(
        &self,
        id: i64,
        upload: u64,
        download: u64,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE forward_rules SET traffic_used = traffic_used + $1 + $2 WHERE id = $3")
            .bind(upload as i64)
            .bind(download as i64)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn find_rule_owner(
        &self,
        rule_id: i64,
        device_group_in: i64,
    ) -> Result<Option<(i64, i64)>, DbError> {
        let row: Option<(i64, i64)> = sqlx::query_as(
            "SELECT id, uid FROM forward_rules WHERE id = $1 AND device_group_in = $2",
        )
        .bind(rule_id)
        .bind(device_group_in)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn delete_rule(&self, id: i64, scope: &ResourceScope) -> Result<u64, DbError> {
        let result = match scope.owner_id() {
            None => sqlx::query("DELETE FROM forward_rules WHERE id = $1").bind(id),
            Some(uid) => sqlx::query("DELETE FROM forward_rules WHERE id = $1 AND uid = $2")
                .bind(id)
                .bind(uid),
        }
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn delete_rules_by_uid(&self, uid: i64) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM forward_rules WHERE uid = $1")
            .bind(uid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn list_active_for_config(&self, group_id: i64) -> Result<Vec<ForwardRule>, DbError> {
        // v0.4.11 PR3: REMOVED cross-owner defense filter. See SQLite impl comment.
        // v1.0.8: FOUR gating conditions (banned, suspended, over-quota,
        // expired). Mirrors the SQLite WHERE clause. PG uses now() for the
        // expiry comparison; plan_expire_at is TEXT UTC 'YYYY-MM-DD HH:MM:SS'.
        let rules: Vec<ForwardRule> = sqlx::query_as(
            "SELECT fr.* FROM forward_rules fr \
             JOIN users u ON fr.uid = u.id \
             WHERE fr.device_group_in = $1 AND fr.paused = FALSE \
             AND u.banned = FALSE \
             AND u.suspended = FALSE \
             AND (u.traffic_limit = 0 OR u.traffic_used < u.traffic_limit) \
             AND (u.plan_expire_at IS NULL OR u.plan_expire_at > to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rules)
    }
}
