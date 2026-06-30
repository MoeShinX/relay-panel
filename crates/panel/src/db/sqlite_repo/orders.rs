use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::Order;

// ── OrderRepository ──

#[async_trait]
impl OrderRepository for SqliteRepository {
    async fn list_orders_by_user(&self, user_id: i64) -> Result<Vec<Order>, DbError> {
        let orders: Vec<Order> = sqlx::query_as(
            "SELECT id, user_id, plan_id, plan_name, price, created_at \
             FROM orders WHERE user_id = ? ORDER BY id DESC",
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
            "INSERT INTO orders (user_id, plan_id, plan_name, price) VALUES (?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(plan_id)
        .bind(plan_name)
        .bind(price)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
