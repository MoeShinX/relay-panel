use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::RedeemCode;
use relay_shared::money;

// ── RedeemRepository (v1.2.0) ──

#[async_trait]
impl RedeemRepository for SqliteRepository {
    async fn create_redeem_codes(&self, codes: &[NewRedeemCode]) -> Result<u64, DbError> {
        if codes.is_empty() {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        let mut inserted = 0u64;
        for c in codes {
            // OR IGNORE: `code` is UNIQUE and generation is random, so a
            // collision is astronomically unlikely — but if one happens, an
            // admin who asked for 100 codes would rather receive 99 than an
            // error and no batch at all. The returned count is the truth.
            let res = sqlx::query(
                "INSERT OR IGNORE INTO redeem_codes (code, amount, expires_at, batch_id, remark) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&c.code)
            .bind(&c.amount)
            .bind(&c.expires_at)
            .bind(&c.batch_id)
            .bind(&c.remark)
            .execute(&mut *tx)
            .await?;
            inserted += res.rows_affected();
        }
        tx.commit().await?;
        Ok(inserted)
    }

    async fn redeem_code(
        &self,
        code: &str,
        user_id: i64,
        now: &str,
    ) -> Result<(String, String), RedeemCodeError> {
        // One DEFERRED transaction, same shape as buy_plan: the write lock is
        // taken on the first write (the claim below), and SQLite serializes
        // writers, so a concurrent redeemer either blocks briefly or gets
        // SQLITE_BUSY — never a double credit.
        let mut tx = self.pool.begin().await?;

        // Read the code first so we can tell "expired" apart from "not
        // redeemable" — the user needs to know an expired card isn't a typo.
        let row: Option<(i64, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, amount, status, expires_at FROM redeem_codes WHERE code = ?",
        )
        .bind(code)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((id, amount, status, expires_at)) = row else {
            let _ = tx.rollback().await;
            return Err(RedeemCodeError::NotRedeemable);
        };
        if status != "unused" {
            let _ = tx.rollback().await;
            return Err(RedeemCodeError::NotRedeemable);
        }
        // Expiry is compared as TEXT: every timestamp in this schema is
        // 'YYYY-MM-DD HH:MM:SS' UTC, a format where lexicographic order IS
        // chronological order.
        if let Some(exp) = expires_at.as_deref() {
            if now > exp {
                let _ = tx.rollback().await;
                // Left 'unused' on purpose so an admin can extend the batch.
                return Err(RedeemCodeError::Expired);
            }
        }

        let amount_cents = money::balance_to_cents(&amount).ok_or_else(|| {
            tracing::error!(
                "redeem_code: code {} has non-canonical amount {:?}",
                id,
                amount
            );
            RedeemCodeError::Database(DbError::NotFound)
        })?;

        let balance_str: Option<(String,)> =
            sqlx::query_as("SELECT balance FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((balance_str,)) = balance_str else {
            let _ = tx.rollback().await;
            return Err(RedeemCodeError::Database(DbError::NotFound));
        };
        let balance_cents = money::balance_to_cents(&balance_str).ok_or_else(|| {
            tracing::error!(
                "redeem_code: user {} has non-canonical balance {:?}",
                user_id,
                balance_str
            );
            RedeemCodeError::Database(DbError::NotFound)
        })?;

        // Top-up ADDS, so unlike buy_plan it can overflow the ceiling. Refuse
        // rather than persist a balance the panel could no longer write back.
        let new_cents = balance_cents
            .checked_add(amount_cents)
            .filter(|c| *c <= money::MAX_BALANCE_CENTS)
            .ok_or(RedeemCodeError::BalanceOverflow)?;
        let new_balance = money::cents_to_balance(new_cents);

        // THE claim. Conditional on status so two concurrent redemptions of the
        // same code cannot both proceed: exactly one gets rows_affected == 1.
        // Without the WHERE clause this whole function would be a race.
        let claimed = sqlx::query(
            "UPDATE redeem_codes SET status = 'used', used_by = ?, used_at = ? \
             WHERE id = ? AND status = 'unused'",
        )
        .bind(user_id)
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if claimed != 1 {
            let _ = tx.rollback().await;
            return Err(RedeemCodeError::NotRedeemable);
        }

        sqlx::query("UPDATE users SET balance = ? WHERE id = ?")
            .bind(&new_balance)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok((amount, new_balance))
    }

    async fn list_redeem_codes(
        &self,
        filter: &RedeemCodeFilter,
    ) -> Result<Vec<RedeemCode>, DbError> {
        // COALESCE keeps this one prepared statement for every filter combo
        // (the same trick the statistics query uses).
        Ok(sqlx::query_as(
            "SELECT * FROM redeem_codes \
             WHERE status = COALESCE(?, status) AND batch_id = COALESCE(?, batch_id) \
             ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(&filter.status)
        .bind(&filter.batch_id)
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn count_redeem_codes(&self, filter: &RedeemCodeFilter) -> Result<i64, DbError> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM redeem_codes \
             WHERE status = COALESCE(?, status) AND batch_id = COALESCE(?, batch_id)",
        )
        .bind(&filter.status)
        .bind(&filter.batch_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    async fn void_redeem_code(&self, id: i64) -> Result<u64, DbError> {
        // Only an UNUSED code can be voided. A used one already moved money;
        // rewriting its status would falsify the audit trail.
        Ok(sqlx::query(
            "UPDATE redeem_codes SET status = 'void' WHERE id = ? AND status = 'unused'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    async fn delete_unused_redeem_codes(&self, ids: &[i64]) -> Result<u64, DbError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        let mut deleted = 0u64;
        for id in ids {
            // `status != 'used'` — unused and voided rows are disposable;
            // a redemption record is not.
            deleted += sqlx::query("DELETE FROM redeem_codes WHERE id = ? AND status != 'used'")
                .bind(id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        }
        tx.commit().await?;
        Ok(deleted)
    }
}
