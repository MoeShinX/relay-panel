use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;

// ── AnnouncementRepository (v1.2.4) ──
//
// Mirrors the SQLite impl; see there for the ordering and update-semantics
// notes. Placeholders are numbered ($1) rather than positional (?).

/// Ordering shared by the banner pick and the history list: pinned first, then
/// newest. `id DESC` breaks ties because several notices can share a second and
/// only the id gives a stable total order for pagination.
const ORDER: &str = "ORDER BY pinned DESC, published_at DESC, id DESC";

#[async_trait]
impl AnnouncementRepository for PgRepository {
    async fn active_announcement(&self, now: &str) -> Result<Option<Announcement>, DbError> {
        Ok(sqlx::query_as(&format!(
            "SELECT * FROM announcements \
             WHERE expires_at IS NULL OR expires_at > $1 {ORDER} LIMIT 1"
        ))
        .bind(now)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn list_announcements(
        &self,
        include_expired: bool,
        now: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Announcement>, DbError> {
        // Two statements rather than a COALESCE trick: the expired filter
        // changes which index helps, and the branch is one line each.
        let sql = if include_expired {
            format!("SELECT * FROM announcements {ORDER} LIMIT $1 OFFSET $2")
        } else {
            format!(
                "SELECT * FROM announcements \
                 WHERE expires_at IS NULL OR expires_at > $1 {ORDER} LIMIT $2 OFFSET $3"
            )
        };
        let mut q = sqlx::query_as(&sql);
        if !include_expired {
            q = q.bind(now);
        }
        Ok(q.bind(limit).bind(offset).fetch_all(&self.pool).await?)
    }

    async fn count_announcements(&self, include_expired: bool, now: &str) -> Result<i64, DbError> {
        if include_expired {
            Ok(sqlx::query_scalar("SELECT COUNT(*) FROM announcements")
                .fetch_one(&self.pool)
                .await?)
        } else {
            Ok(sqlx::query_scalar(
                "SELECT COUNT(*) FROM announcements WHERE expires_at IS NULL OR expires_at > $1",
            )
            .bind(now)
            .fetch_one(&self.pool)
            .await?)
        }
    }

    async fn find_announcement(&self, id: i64) -> Result<Option<Announcement>, DbError> {
        Ok(sqlx::query_as("SELECT * FROM announcements WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    async fn create_announcement(&self, a: &NewAnnouncement) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "INSERT INTO announcements \
             (title, content, kind, pinned, published_at, expires_at, author_id, author_name) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        )
        .bind(&a.title)
        .bind(&a.content)
        .bind(&a.kind)
        .bind(a.pinned)
        .bind(&a.published_at)
        .bind(&a.expires_at)
        .bind(a.author_id)
        .bind(&a.author_name)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn update_announcement(&self, id: i64, a: &NewAnnouncement) -> Result<u64, DbError> {
        // published_at and the author are NOT rewritten: editing a typo must
        // not re-date a notice or reassign who posted it.
        Ok(sqlx::query(
            "UPDATE announcements SET title = $1, content = $2, kind = $3, pinned = $4, \
             expires_at = $5 WHERE id = $6",
        )
        .bind(&a.title)
        .bind(&a.content)
        .bind(&a.kind)
        .bind(a.pinned)
        .bind(&a.expires_at)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    async fn delete_announcement(&self, id: i64) -> Result<u64, DbError> {
        Ok(sqlx::query("DELETE FROM announcements WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }
    async fn latest_announcement_id(&self) -> Result<i64, DbError> {
        Ok(
            sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM announcements")
                .fetch_one(&self.pool)
                .await?,
        )
    }
}
