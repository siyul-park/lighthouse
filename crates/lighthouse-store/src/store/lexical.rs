//! Lexical ranking with SQLite FTS5: the documents are loaded into a temporary
//! virtual table of the store's own connection, ranked by its `bm25()`, and
//! dropped. Nothing is kept.
//!
//! The `trigram` tokenizer is used because it matches English and Hangul alike:
//! `unicode61` splits on spaces only, so a Korean word with a particle
//! (`에러는`) never matches the same word with another (`에러를`), and `porter`
//! stems English only. A trigram index matches any three characters in common,
//! so the query is written as the three-character windows of its words, each
//! quoted, joined by `OR`; words shorter than three characters match nothing,
//! which is the price of the tokenizer.

use rusqlite::params;

use super::Store;
use crate::Error;

/// The id of the row that holds the query itself, which scores the best a
/// document could.
const QUERY_ROW: &str = "\u{0}query";

/// A document ranked against a query.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub id: String,
    /// `bm25()` of the document divided by that of the query text itself, as
    /// a row of the same table: 0 for no shared trigram, about 1 for a
    /// document that says what the query says. Comparable across queries.
    pub similarity: f64,
}

impl Store {
    /// The `documents` (`(id, text)`) that share text with `query`, best first
    /// and at most `limit`; ties go to the smaller id.
    pub fn rank(
        &self,
        documents: &[(String, String)],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Ranked>, Error> {
        let Some(terms) = terms(query) else {
            return Ok(Vec::new());
        };
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS temp.ranking; \
             CREATE VIRTUAL TABLE temp.ranking USING fts5(id UNINDEXED, body, tokenize = 'trigram')",
        )?;
        let ranked = self.ranked(documents, query, &terms);
        self.conn
            .execute_batch("DROP TABLE IF EXISTS temp.ranking")?;
        let mut ranked = ranked?;
        ranked.sort_by(|a, b| {
            b.similarity
                .total_cmp(&a.similarity)
                .then_with(|| a.id.cmp(&b.id))
        });
        ranked.truncate(limit);
        Ok(ranked)
    }

    fn ranked(
        &self,
        documents: &[(String, String)],
        query: &str,
        terms: &str,
    ) -> Result<Vec<Ranked>, Error> {
        let mut insert = self
            .conn
            .prepare("INSERT INTO temp.ranking (id, body) VALUES (?1, ?2)")?;
        for (id, text) in documents {
            insert.execute(params![id, text])?;
        }
        insert.execute(params![QUERY_ROW, query])?;
        let mut stmt = self.conn.prepare(
            "SELECT id, -bm25(ranking) FROM temp.ranking WHERE ranking MATCH ?1 ORDER BY bm25(ranking), id",
        )?;
        let rows: Vec<(String, f64)> = stmt
            .query_map(params![terms], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let best = rows
            .iter()
            .find(|(id, _)| id == QUERY_ROW)
            .map_or(0.0, |(_, score)| *score);
        if best <= 0.0 {
            return Ok(Vec::new());
        }
        Ok(rows
            .into_iter()
            .filter(|(id, _)| id != QUERY_ROW)
            .map(|(id, score)| Ranked {
                id,
                similarity: (score / best).clamp(0.0, 1.0),
            })
            .collect())
    }
}

/// The FTS5 query for `text`: every three-character window of every word,
/// lower-cased and quoted, joined by `OR`; `None` when no word has three
/// characters.
fn terms(text: &str) -> Option<String> {
    let mut windows: Vec<String> = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let chars: Vec<char> = word.to_lowercase().chars().collect();
        for window in chars.windows(3) {
            let window: String = window.iter().collect();
            if !windows.contains(&window) {
                windows.push(window);
            }
        }
    }
    (!windows.is_empty()).then(|| {
        windows
            .iter()
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" OR ")
    })
}
