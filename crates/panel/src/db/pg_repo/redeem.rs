use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::RedeemCode;
use relay_shared::money;

// ── RedeemRepository (v1.2.0) ──
//
// Same contract as the SQLite impl, but the concurrency guarantee comes from a
// different place. SQLite serializes writers process-wide, so its transaction
// is enough on its own; PG allows concurrent writers, so the code row is locked
// explicitly with SELECT ... FOR UPDATE (the same thing buy_plan does to the
// user row). Both still make the final claim conditional on status.

#[async_trait]
impl RedeemRepository for PgRepository {
    async fn create_redeem_codes(&self, codes: &[NewRedeemCode]) -> Result<u64, DbError> {
        if codes.is_empty() {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        let mut inserted = 0u64;
        for c in codes {
            // ON CONFLICT DO NOTHING is PG's INSERT OR IGNORE: a (vanishingly
            // unlikely) code collision skips that row instead of failing the
            // whole batch.
            let res = sqlx::query(
                "INSERT INTO redeem_codes (code, amount, expires_at, batch_id, remark) \
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (code) DO NOTHING",
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
        let mut tx = self.pool.begin().await?;

        // FOR UPDATE holds the code row for this transaction, so a concurrent
        // redemption of the same code blocks here instead of reading a stale
        // 'unused' and racing us to the claim.
        let row: Option<(i64, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, amount, status, expires_at FROM redeem_codes WHERE code = $1 FOR UPDATE",
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
        // TEXT comparison is chronological: all timestamps are
        // 'YYYY-MM-DD HH:MM:SS' UTC.
        if let Some(exp) = expires_at.as_deref() {
            if now > exp {
                let _ = tx.rollback().await;
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

        let balance_row: Option<(String,)> =
            sqlx::query_as("SELECT balance FROM users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((balance_str,)) = balance_row else {
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

        // Top-up adds, so it can overflow the ceiling (deduction never could).
        let new_cents = balance_cents
            .checked_add(amount_cents)
            .filter(|c| *c <= money::MAX_BALANCE_CENTS)
            .ok_or(RedeemCodeError::BalanceOverflow)?;
        let new_balance = money::cents_to_balance(new_cents);

        // Still conditional on status even under FOR UPDATE — defence in depth,
        // and it keeps the two backends' invariants identical.
        let claimed = sqlx::query(
            "UPDATE redeem_codes SET status = 'used', used_by = $1, used_at = $2 \
             WHERE id = $3 AND status = 'unused'",
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

        sqlx::query("UPDATE users SET balance = $1 WHERE id = $2")
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
        Ok(sqlx::query_as(
            "SELECT * FROM redeem_codes \
             WHERE status = COALESCE($1, status) AND batch_id = COALESCE($2, batch_id) \
             ORDER BY id DESC LIMIT $3 OFFSET $4",
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
             WHERE status = COALESCE($1, status) AND batch_id = COALESCE($2, batch_id)",
        )
        .bind(&filter.status)
        .bind(&filter.batch_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    async fn void_redeem_code(&self, id: i64) -> Result<u64, DbError> {
        Ok(sqlx::query(
            "UPDATE redeem_codes SET status = 'void' WHERE id = $1 AND status = 'unused'",
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
        // PG can express this as one statement with = ANY($1); a used row is
        // never eligible.
        Ok(
            sqlx::query("DELETE FROM redeem_codes WHERE id = ANY($1) AND status != 'used'")
                .bind(ids)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }
}
