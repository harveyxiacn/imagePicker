//! Smart collections (docs/api-contract-m4.md C.2): saved `/api/photos` queries. The built-ins
//! live in code, user collections in `smart_collection`.

use serde::Serialize;

use crate::catalog::now_ms;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

/// `(key, query)`; the display name is the i18n key `collection.<key>`.
pub const BUILTIN: [(&str, &str); 4] = [
    ("best_per_group", "burst_best_only=1"),
    ("has_closed_eyes", "issues_any=closed_eyes"),
    ("undecided", "flag=unflagged"),
    ("edited", "has_edits=1"),
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CollectionOut {
    /// Built-ins: their key; user collections: `user_<n>`.
    pub id: String,
    pub name: String,
    pub query: String,
    pub builtin: bool,
}

fn user_id(n: i64) -> String {
    format!("user_{n}")
}

fn parse_user_id(id: &str) -> Result<i64> {
    if BUILTIN.iter().any(|(k, _)| *k == id) {
        return Err(CoreError::bad_request(
            "built-in collections cannot be changed or deleted",
        ));
    }
    id.strip_prefix("user_")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CoreError::not_found(format!("collection {id} not found")))
}

fn clean_name(name: &str) -> Result<String> {
    let n = name.trim().to_string();
    if n.is_empty() || n.chars().count() > 100 {
        return Err(CoreError::bad_request("name must be 1..100 characters"));
    }
    Ok(n)
}

fn clean_query(q: &str) -> Result<String> {
    let q = q.trim().trim_start_matches('?').to_string();
    if q.len() > 4000 {
        return Err(CoreError::bad_request("query is too long"));
    }
    Ok(q)
}

impl Core {
    pub async fn collections(&self) -> Result<Vec<CollectionOut>> {
        let mut out: Vec<CollectionOut> = BUILTIN
            .iter()
            .map(|(k, q)| CollectionOut {
                id: (*k).to_string(),
                name: format!("collection.{k}"),
                query: (*q).to_string(),
                builtin: true,
            })
            .collect();
        let rows = self
            .db
            .call(|c| {
                let mut st =
                    c.prepare("SELECT id, name, query FROM smart_collection ORDER BY id")?;
                let v = st
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await?;
        out.extend(rows.into_iter().map(|(id, name, query)| CollectionOut {
            id: user_id(id),
            name,
            query,
            builtin: false,
        }));
        Ok(out)
    }

    /// `query` must already be validated by the caller (the HTTP layer owns the parameter
    /// grammar of `/api/photos`).
    pub async fn create_collection(&self, name: &str, query: &str) -> Result<CollectionOut> {
        let (name, query) = (clean_name(name)?, clean_query(query)?);
        let (n2, q2) = (name.clone(), query.clone());
        let id = self
            .db
            .call(move |c| {
                c.execute(
                    "INSERT INTO smart_collection(name, query, created_at) VALUES(?1,?2,?3)",
                    rusqlite::params![n2, q2, now_ms()],
                )?;
                Ok(c.last_insert_rowid())
            })
            .await?;
        self.events.emit(Event::CollectionsUpdated {});
        Ok(CollectionOut {
            id: user_id(id),
            name,
            query,
            builtin: false,
        })
    }

    pub async fn update_collection(
        &self,
        id: &str,
        name: Option<String>,
        query: Option<String>,
    ) -> Result<CollectionOut> {
        let n = parse_user_id(id)?;
        let name = name.as_deref().map(clean_name).transpose()?;
        let query = query.as_deref().map(clean_query).transpose()?;
        let out = self
            .db
            .call(move |c| {
                let (mut cur_name, mut cur_query): (String, String) = c
                    .query_row(
                        "SELECT name, query FROM smart_collection WHERE id=?1",
                        [n],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => {
                            CoreError::not_found(format!("collection user_{n} not found"))
                        }
                        e => e.into(),
                    })?;
                if let Some(v) = name {
                    cur_name = v;
                }
                if let Some(v) = query {
                    cur_query = v;
                }
                c.execute(
                    "UPDATE smart_collection SET name=?2, query=?3 WHERE id=?1",
                    rusqlite::params![n, cur_name, cur_query],
                )?;
                Ok(CollectionOut {
                    id: user_id(n),
                    name: cur_name,
                    query: cur_query,
                    builtin: false,
                })
            })
            .await?;
        self.events.emit(Event::CollectionsUpdated {});
        Ok(out)
    }

    pub async fn delete_collection(&self, id: &str) -> Result<()> {
        let n = parse_user_id(id)?;
        let gone = self
            .db
            .call(move |c| Ok(c.execute("DELETE FROM smart_collection WHERE id=?1", [n])?))
            .await?;
        if gone == 0 {
            return Err(CoreError::not_found(format!("collection {id} not found")));
        }
        self.events.emit(Event::CollectionsUpdated {});
        Ok(())
    }
}
