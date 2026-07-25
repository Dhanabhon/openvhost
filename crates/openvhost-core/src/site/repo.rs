// SPDX-License-Identifier: GPL-3.0-or-later
//! Site persistence. Queries are compile-time-checked (`query_as!`/`query!`);
//! rows re-validate through the domain newtypes on read (`TryFrom<SiteRow>`),
//! so a hand-edited `state.db` can never feed an unvalidated value downstream.

use sqlx::SqlitePool;

use crate::db::{Db, now_ms};
use crate::error::CoreError;
use crate::site::{Docroot, Domain, NewSite, PhpVersion, Site, SiteId, SiteName, WebServer};

/// The persistence seam. Consumers depend on this, not the concrete type.
///
/// Methods return `impl Future + Send` (RPITIT) rather than using
/// `async_trait`: sqlx's futures are already `Send`, so the extra box+dyn
/// indirection would be pure overhead for our one production implementation.
pub trait SiteRepository: Send + Sync {
    /// Persist a new site, stamping `created_at`/`updated_at` with `now_ms`.
    ///
    /// A UNIQUE violation on `name` or `domain` maps to
    /// `CoreError::Validation`, never a raw `sqlx::Error`.
    fn create(
        &self,
        new: NewSite,
    ) -> impl std::future::Future<Output = Result<Site, CoreError>> + Send;
    /// Look up a site by id; `Ok(None)` if it does not exist.
    fn get(
        &self,
        id: &SiteId,
    ) -> impl std::future::Future<Output = Result<Option<Site>, CoreError>> + Send;
    /// All sites, ordered by name.
    fn list(&self) -> impl std::future::Future<Output = Result<Vec<Site>, CoreError>> + Send;
    /// Overwrite an existing site's mutable fields, bumping `updated_at`.
    ///
    /// A UNIQUE violation on `name` or `domain` maps to
    /// `CoreError::Validation`, never a raw `sqlx::Error`.
    fn update(
        &self,
        site: &Site,
    ) -> impl std::future::Future<Output = Result<Site, CoreError>> + Send;
    /// Delete a site by id; returns whether a row was actually removed.
    fn delete(
        &self,
        id: &SiteId,
    ) -> impl std::future::Future<Output = Result<bool, CoreError>> + Send;
}

/// Raw DB row (primitive columns) — decoded by sqlx, then re-validated.
struct SiteRow {
    id: String,
    name: String,
    domain: String,
    docroot: String,
    web_server: String,
    php_version: String,
    enabled: i64,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<SiteRow> for Site {
    type Error = CoreError;
    fn try_from(r: SiteRow) -> Result<Site, CoreError> {
        Ok(Site {
            id: SiteId::parse(&r.id)?,
            name: SiteName::parse(&r.name)?,
            domain: Domain::parse(&r.domain)?,
            docroot: Docroot::parse(&r.docroot)?,
            web_server: WebServer::parse(&r.web_server)?,
            php_version: PhpVersion::parse(&r.php_version)?,
            enabled: r.enabled != 0,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

/// Maps a sqlx UNIQUE-constraint error to a `Validation` on the right field.
///
/// Gated on `is_unique_violation()` (the typed `ErrorKind`, not just a
/// string match) before inspecting the message text, so an unrelated error
/// that happens to mention "sites.name"/"sites.domain" can never be
/// misclassified as a validation failure.
fn map_insert_err(e: sqlx::Error, name: &str, domain: &str) -> CoreError {
    if let sqlx::Error::Database(dbe) = &e
        && dbe.is_unique_violation()
    {
        let msg = dbe.message();
        if msg.contains("sites.name") {
            return CoreError::Validation {
                field: "name",
                reason: format!("{name:?} is already taken"),
            };
        }
        if msg.contains("sites.domain") {
            return CoreError::Validation {
                field: "domain",
                reason: format!("{domain:?} is already taken"),
            };
        }
    }
    CoreError::Db(e)
}

/// SQLite-backed [`SiteRepository`].
pub struct SqliteSiteRepository {
    pool: SqlitePool,
}

impl SqliteSiteRepository {
    /// Build a repository over the given database's connection pool.
    pub fn new(db: &Db) -> Self {
        Self {
            pool: db.pool().clone(),
        }
    }
}

impl SiteRepository for SqliteSiteRepository {
    async fn create(&self, new: NewSite) -> Result<Site, CoreError> {
        let id = SiteId::new();
        let ts = now_ms();
        let docroot = new.docroot.as_str();
        let ws = new.web_server.as_str();
        let enabled = i64::from(new.enabled);
        sqlx::query!(
            "INSERT INTO sites (id, name, domain, docroot, web_server, php_version, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id.as_str(),
            new.name.as_str(),
            new.domain.as_str(),
            docroot,
            ws,
            new.php_version.as_str(),
            enabled,
            ts,
            ts,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| map_insert_err(e, new.name.as_str(), new.domain.as_str()))?;

        Ok(Site {
            id,
            name: new.name,
            domain: new.domain,
            docroot: new.docroot,
            web_server: new.web_server,
            php_version: new.php_version,
            enabled: new.enabled,
            created_at: ts,
            updated_at: ts,
        })
    }

    async fn get(&self, id: &SiteId) -> Result<Option<Site>, CoreError> {
        let row = sqlx::query_as!(
            SiteRow,
            "SELECT id, name, domain, docroot, web_server, php_version, enabled, created_at, updated_at \
             FROM sites WHERE id = ?",
            id.as_str()
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Site::try_from).transpose()
    }

    async fn list(&self) -> Result<Vec<Site>, CoreError> {
        let rows = sqlx::query_as!(
            SiteRow,
            "SELECT id, name, domain, docroot, web_server, php_version, enabled, created_at, updated_at \
             FROM sites ORDER BY name"
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Site::try_from).collect()
    }

    async fn update(&self, site: &Site) -> Result<Site, CoreError> {
        let ts = now_ms();
        let docroot = site.docroot.as_str();
        let enabled = i64::from(site.enabled);
        sqlx::query!(
            "UPDATE sites SET name = ?, domain = ?, docroot = ?, web_server = ?, php_version = ?, enabled = ?, updated_at = ? \
             WHERE id = ?",
            site.name.as_str(),
            site.domain.as_str(),
            docroot,
            site.web_server.as_str(),
            site.php_version.as_str(),
            enabled,
            ts,
            site.id.as_str(),
        )
        .execute(&self.pool)
        .await
        .map_err(|e| map_insert_err(e, site.name.as_str(), site.domain.as_str()))?;

        let mut updated = site.clone();
        updated.updated_at = ts;
        Ok(updated)
    }

    async fn delete(&self, id: &SiteId) -> Result<bool, CoreError> {
        let res = sqlx::query!("DELETE FROM sites WHERE id = ?", id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::site::{Docroot, Domain, NewSite, PhpVersion, SiteName, WebServer};

    fn sample(name: &str, domain: &str) -> NewSite {
        NewSite {
            name: SiteName::parse(name).unwrap(),
            domain: Domain::parse(domain).unwrap(),
            docroot: Docroot::parse("/srv/www/shop").unwrap(),
            web_server: WebServer::Nginx,
            php_version: PhpVersion::parse("8.3").unwrap(),
            enabled: true,
        }
    }

    async fn repo() -> (Db, SqliteSiteRepository) {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteSiteRepository::new(&db);
        (db, repo)
    }

    #[tokio::test]
    async fn create_get_list_update_delete_roundtrip() {
        let (_db, repo) = repo().await;
        let created = repo.create(sample("shop", "shop.localhost")).await.unwrap();
        assert_eq!(created.name.as_str(), "shop");
        assert!(created.created_at > 0 && created.updated_at == created.created_at);

        let got = repo.get(&created.id).await.unwrap().unwrap();
        assert_eq!(got, created);

        repo.create(sample("blog", "blog.localhost")).await.unwrap();
        let all = repo.list().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name.as_str(), "blog"); // ordered by name

        let mut edit = created.clone();
        edit.php_version = PhpVersion::parse("8.2").unwrap();
        let updated = repo.update(&edit).await.unwrap();
        assert_eq!(updated.php_version.as_str(), "8.2");
        assert!(updated.updated_at >= created.updated_at);

        assert!(repo.delete(&created.id).await.unwrap());
        assert!(repo.get(&created.id).await.unwrap().is_none());
        assert!(!repo.delete(&created.id).await.unwrap()); // already gone
    }

    #[tokio::test]
    async fn duplicate_name_and_domain_map_to_validation() {
        let (_db, repo) = repo().await;
        repo.create(sample("shop", "shop.localhost")).await.unwrap();
        let dup_name = repo.create(sample("shop", "other.localhost")).await;
        assert!(matches!(
            dup_name,
            Err(CoreError::Validation { field: "name", .. })
        ));
        let dup_domain = repo.create(sample("other", "shop.localhost")).await;
        assert!(matches!(
            dup_domain,
            Err(CoreError::Validation {
                field: "domain",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn tampered_row_is_rejected_on_read() {
        let (db, repo) = repo().await;
        // Hand-insert a row with a hostile domain (simulating a tampered db).
        sqlx::query("INSERT INTO sites VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(crate::site::SiteId::new().as_str())
            .bind("x")
            .bind("evil\";inject")
            .bind("/srv/www")
            .bind("nginx")
            .bind("8.3")
            .bind(1)
            .bind(1)
            .bind(1)
            .execute(db.pool())
            .await
            .unwrap();
        // list() re-validates via TryFrom<SiteRow> and must reject it.
        assert!(matches!(
            repo.list().await,
            Err(CoreError::Validation {
                field: "domain",
                ..
            })
        ));
    }
}
