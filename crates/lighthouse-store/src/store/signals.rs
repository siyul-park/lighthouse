//! Session signals and the proposals agents recorded from them: local
//! Evidence of the cache, never committed and never part of the decision log.

use lighthouse_model::{Attribution, Fingerprint, SignalStatus};
use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};

use super::{
    Store,
    rows::{conversion, parse, role_of},
};
use crate::{Error, NewSignal, Proposal, Signal, SignalFilter};

const COLUMNS: &str = "signal_id, session, generated_at, event, class, excerpt, features, rule, \
     agent_type, agent_id, agent_role, status, detail, settled_by, asked";

impl Store {
    /// Records a signal unless one with its id is there already, and purges
    /// the signals older than `retention_secs`. Returns whether it was new.
    pub fn add_signal(&mut self, new: &NewSignal, retention_secs: u64) -> Result<bool, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM signals WHERE generated_at < \
             strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)",
            params![format!("-{retention_secs} seconds")],
        )?;
        let added = tx.execute(
            "INSERT OR IGNORE INTO signals (signal_id, session, generated_at, event, class, excerpt, \
                 features, rule, agent_type, agent_id, agent_role) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                new.id,
                new.session,
                lighthouse_model::now(),
                new.event,
                new.class.as_str(),
                new.excerpt,
                new.features.to_string(),
                new.rule,
                new.by.kind.as_str(),
                new.by.id,
                new.by.role.map(|r| r.as_str()),
            ],
        )?;
        tx.commit()?;
        Ok(added > 0)
    }

    /// The signals matching `filter`, oldest first.
    pub fn signals(&self, filter: &SignalFilter) -> Result<Vec<Signal>, Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM signals WHERE (?1 IS NULL OR session = ?1) \
             AND (?2 IS NULL OR status = ?2) AND (?3 = 0 OR asked = 0) \
             AND (?4 = 0 OR class IN ('directive', 'correction', 'preference')) \
             ORDER BY generated_at, signal_id"
        ))?;
        let rows = stmt.query_map(
            params![filter.session, filter.status, filter.unasked, filter.said],
            signal,
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The signal whose id is, or uniquely starts with, `id`.
    pub fn signal(&self, id: &str) -> Result<Signal, Error> {
        if id.len() < Fingerprint::MIN_PREFIX {
            return Err(Error::ShortSignal(id.to_owned()));
        }
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM signals WHERE signal_id LIKE ?1 || '%' LIMIT 6"
        ))?;
        let escaped = id.replace(['%', '_', '\\'], "");
        let mut found: Vec<Signal> = stmt
            .query_map(params![escaped], signal)?
            .collect::<Result<_, _>>()?;
        match found.len() {
            0 => Err(Error::UnknownSignal(id.to_owned())),
            1 => Ok(found.remove(0)),
            _ => Err(Error::AmbiguousSignal {
                prefix: id.to_owned(),
                candidates: found.into_iter().map(|s| s.id).collect(),
            }),
        }
    }

    /// Settles the signal `id` (a full id) as `status`, by `by`.
    pub fn settle_signal(
        &mut self,
        id: &str,
        status: &SignalStatus,
        by: &str,
    ) -> Result<(), Error> {
        self.conn.execute(
            "UPDATE signals SET status = ?2, detail = ?3, settled_by = ?4 WHERE signal_id = ?1",
            params![id, status.name(), status.detail(), by],
        )?;
        Ok(())
    }

    /// Marks signals as asked about, so that a stop does not ask twice.
    pub fn mark_asked(&mut self, ids: &[String]) -> Result<(), Error> {
        for id in ids {
            self.conn.execute(
                "UPDATE signals SET asked = 1 WHERE signal_id = ?1",
                params![id],
            )?;
        }
        Ok(())
    }

    /// Records that an agent or a person created the proposal `uid`, with the
    /// ids of the decisions the dedupe check named.
    pub fn add_proposal(
        &mut self,
        uid: &str,
        decision: &str,
        duplicates: &[String],
    ) -> Result<(), Error> {
        self.conn.execute(
            "INSERT OR REPLACE INTO proposals (decision_uid, decision, generated_at, duplicates) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                uid,
                decision,
                lighthouse_model::now(),
                serde_json::to_string(duplicates)?
            ],
        )?;
        Ok(())
    }

    /// Every proposal recorded, oldest first.
    pub fn proposals(&self) -> Result<Vec<Proposal>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT decision_uid, decision, generated_at, duplicates FROM proposals \
             ORDER BY generated_at, decision_uid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (decision_uid, decision, time, duplicates) = row?;
            Ok(Proposal {
                decision_uid,
                decision,
                time,
                duplicates: serde_json::from_str(&duplicates)?,
            })
        })
        .collect()
    }

    /// Whether the store holds a signal with this id.
    pub fn has_signal(&self, id: &str) -> Result<bool, Error> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM signals WHERE signal_id = ?1",
                params![id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

fn signal(row: &Row) -> rusqlite::Result<Signal> {
    let features: String = row.get(6)?;
    let status = row.get::<_, String>(11)?;
    Ok(Signal {
        id: row.get(0)?,
        session: row.get(1)?,
        time: row.get(2)?,
        event: row.get(3)?,
        class: parse(&row.get::<_, String>(4)?)?,
        excerpt: row.get(5)?,
        features: serde_json::from_str(&features).map_err(conversion)?,
        rule: row.get(7)?,
        by: Attribution {
            kind: parse(&row.get::<_, String>(8)?)?,
            id: row.get(9)?,
            role: role_of(row.get(10)?)?,
        },
        status: SignalStatus::from_parts(&status, row.get(12)?).ok_or_else(|| {
            conversion(Error::Io {
                path: "signals".to_owned(),
                source: std::io::Error::other(format!("unknown status `{status}`")),
            })
        })?,
        settled_by: row.get(13)?,
        asked: row.get(14)?,
    })
}
