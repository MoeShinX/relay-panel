use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::Order;

// ── OrderRepository ──

#[async_trait]
impl OrderRepository for PgRepository {
    async fn list_orders_by_user(&self, user_id: i64) -> Result<Vec<Order>, DbError> {
        let orders: Vec<Order> = sqlx::query_as(
            "SELECT id, user_id, plan_id, plan_name, price, created_at \
             FROM orders WHERE user_id = $1 ORDER BY id DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(orders)
    }

    async fn insert_order(
        &self,
        user_id: i64,
        plan_id: Option<i64>,
        plan_name: &str,
        price: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO orders (user_id, plan_id, plan_name, price) VALUES ($1, $2, $3, $4)",
        )
        .bind(user_id)
        .bind(plan_id)
        .bind(plan_name)
        .bind(price)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    async fn list_all_orders(&self, limit: i64, offset: i64) -> Result<Vec<Order>, DbError> {
        // id DESC breaks ties: several purchases can land in the same second,
        // and only the id gives a stable total order for pagination.
        Ok(sqlx::query_as(
            "SELECT * FROM orders ORDER BY created_at DESC, id DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn count_all_orders(&self) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM orders")
            .fetch_one(&self.pool)
            .await?)
    }
}
