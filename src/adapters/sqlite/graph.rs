//! `impl GraphPort for GrugDb` — cross-link similarity graph queries.

use crate::domain::ports::GraphPort;
use crate::tools::GrugDb;
use serde_json::{json, Value};

impl GraphPort for GrugDb {
    fn graph_json(
        &mut self,
        brain: Option<&str>,
        _mode: Option<&str>,
        _node: Option<&str>,
        _depth: Option<usize>,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        const SCORE_THRESHOLD: f64 = 0.1;
        const EDGE_CAP: usize = 1000;

        let brain_owned: Option<String> = brain.map(|s| s.to_string());
        let (node_sql, params): (&str, Vec<&dyn rusqlite::types::ToSql>) = match &brain_owned {
            Some(name) => (
                "SELECT brain, path, category, name FROM brain_fts WHERE brain = ?1",
                vec![name as &dyn rusqlite::types::ToSql],
            ),
            None => ("SELECT brain, path, category, name FROM brain_fts", vec![]),
        };
        let mut stmt = self.conn().prepare(node_sql).map_err(|e| e.to_string())?;
        let nodes: Vec<Value> = stmt
            .query_map(params.as_slice(), |row| {
                Ok(json!({
                    "brain": row.get::<_, String>(0)?,
                    "path": row.get::<_, String>(1)?,
                    "category": row.get::<_, String>(2)?,
                    "name": row.get::<_, String>(3)?,
                }))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        // Similarity edges from cross_links.
        let sim_edges: Vec<Value> = match &brain_owned {
            Some(name) => {
                let mut sim_stmt = self
                    .conn()
                    .prepare(
                        "SELECT brain_a, path_a, brain_b, path_b, score FROM cross_links \
                         WHERE brain_a = ?1 AND brain_b = ?1 AND score >= ?2 \
                         ORDER BY score DESC LIMIT ?3",
                    )
                    .map_err(|e| e.to_string())?;
                sim_stmt
                    .query_map(
                        rusqlite::params![name, SCORE_THRESHOLD, EDGE_CAP as i64],
                        |row| {
                            Ok(json!({
                                "src": {"brain": row.get::<_, String>(0)?, "path": row.get::<_, String>(1)?},
                                "dst": {"brain": row.get::<_, String>(2)?, "path": row.get::<_, String>(3)?},
                                "kind": "similarity",
                                "score": row.get::<_, f64>(4)?,
                            }))
                        },
                    )
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect()
            }
            None => {
                let mut sim_stmt = self
                    .conn()
                    .prepare(
                        "SELECT brain_a, path_a, brain_b, path_b, score FROM cross_links \
                         WHERE score >= ?1 ORDER BY score DESC LIMIT ?2",
                    )
                    .map_err(|e| e.to_string())?;
                sim_stmt
                    .query_map(
                        rusqlite::params![SCORE_THRESHOLD, EDGE_CAP as i64],
                        |row| {
                            Ok(json!({
                                "src": {"brain": row.get::<_, String>(0)?, "path": row.get::<_, String>(1)?},
                                "dst": {"brain": row.get::<_, String>(2)?, "path": row.get::<_, String>(3)?},
                                "kind": "similarity",
                                "score": row.get::<_, f64>(4)?,
                            }))
                        },
                    )
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect()
            }
        };

        // Explicit wikilink edges from links table (resolved targets only).
        let link_edges: Vec<Value> = match &brain_owned {
            Some(name) => {
                let mut link_stmt = self
                    .conn()
                    .prepare(
                        "SELECT brain, src_path, target_brain, target_path FROM links \
                         WHERE target_brain IS NOT NULL AND target_path IS NOT NULL \
                         AND brain = ?1 AND target_brain = ?1 LIMIT ?2",
                    )
                    .map_err(|e| e.to_string())?;
                link_stmt
                    .query_map(rusqlite::params![name, EDGE_CAP as i64], |row| {
                        Ok(json!({
                            "src": {"brain": row.get::<_, String>(0)?, "path": row.get::<_, String>(1)?},
                            "dst": {"brain": row.get::<_, String>(2)?, "path": row.get::<_, String>(3)?},
                            "kind": "explicit",
                            "score": 1.0,
                        }))
                    })
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect()
            }
            None => {
                let mut link_stmt = self
                    .conn()
                    .prepare(
                        "SELECT brain, src_path, target_brain, target_path FROM links \
                         WHERE target_brain IS NOT NULL AND target_path IS NOT NULL LIMIT ?1",
                    )
                    .map_err(|e| e.to_string())?;
                link_stmt
                    .query_map(rusqlite::params![EDGE_CAP as i64], |row| {
                        Ok(json!({
                            "src": {"brain": row.get::<_, String>(0)?, "path": row.get::<_, String>(1)?},
                            "dst": {"brain": row.get::<_, String>(2)?, "path": row.get::<_, String>(3)?},
                            "kind": "explicit",
                            "score": 1.0,
                        }))
                    })
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect()
            }
        };

        let mut edges = sim_edges;
        edges.extend(link_edges);
        if edges.len() > EDGE_CAP {
            edges.truncate(EDGE_CAP);
        }

        serde_json::to_string(&json!({"nodes": nodes, "edges": edges}))
            .map_err(|e| e.to_string())
    }

    fn graph_local_json(
        &mut self,
        brain: Option<&str>,
        path: &str,
        hops: u64,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        const VISIT_CAP: usize = 200;
        let focus_brain = self.resolve_brain(brain)?.name.clone();
        let focus_key = (focus_brain.clone(), path.to_string());

        use std::collections::{HashMap, HashSet};
        let mut visited: HashSet<(String, String)> = HashSet::new();
        visited.insert(focus_key.clone());
        let mut frontier: Vec<(String, String)> = vec![focus_key.clone()];
        let mut edges: Vec<((String, String), (String, String))> = Vec::new();

        let mut out_stmt = self
            .conn()
            .prepare(
                "SELECT target_brain, target_path FROM links \
                 WHERE brain = ?1 AND src_path = ?2 \
                 AND target_brain IS NOT NULL AND target_path IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let mut in_stmt = self
            .conn()
            .prepare(
                "SELECT brain, src_path FROM links \
                 WHERE target_brain = ?1 AND target_path = ?2",
            )
            .map_err(|e| e.to_string())?;

        for _ in 0..hops {
            let mut next: Vec<(String, String)> = Vec::new();
            for (b, p) in &frontier {
                if visited.len() >= VISIT_CAP {
                    break;
                }
                let out_neighbors: Vec<(String, String)> = out_stmt
                    .query_map(rusqlite::params![b, p], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect();
                for n in out_neighbors {
                    edges.push(((b.clone(), p.clone()), n.clone()));
                    if !visited.contains(&n) && visited.len() < VISIT_CAP {
                        visited.insert(n.clone());
                        next.push(n);
                    }
                }
                let in_neighbors: Vec<(String, String)> = in_stmt
                    .query_map(rusqlite::params![b, p], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect();
                for n in in_neighbors {
                    edges.push((n.clone(), (b.clone(), p.clone())));
                    if !visited.contains(&n) && visited.len() < VISIT_CAP {
                        visited.insert(n.clone());
                        next.push(n);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }

        // Enrich nodes from brain_fts.
        let mut node_meta: HashMap<(String, String), (String, String)> = HashMap::new();
        {
            let mut meta_stmt = self
                .conn()
                .prepare(
                    "SELECT category, name FROM brain_fts WHERE brain = ?1 AND path = ?2",
                )
                .map_err(|e| e.to_string())?;
            for k in &visited {
                let row: Option<(String, String)> = meta_stmt
                    .query_row(rusqlite::params![&k.0, &k.1], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .ok();
                if let Some(r) = row {
                    node_meta.insert(k.clone(), r);
                }
            }
        }

        let nodes: Vec<Value> = visited
            .iter()
            .map(|k| {
                let (cat, name) = node_meta
                    .get(k)
                    .cloned()
                    .unwrap_or_else(|| (String::new(), k.1.clone()));
                json!({
                    "brain": k.0,
                    "path":  k.1,
                    "category": cat,
                    "name": name,
                })
            })
            .collect();

        // Dedup edges (undirected, keep "explicit" kind).
        let mut seen: HashSet<(String, String, String, String)> = HashSet::new();
        let mut edge_values: Vec<Value> = Vec::new();
        for (a, b) in edges {
            let mut key = [a.clone(), b.clone()];
            key.sort();
            let k = (
                key[0].0.clone(),
                key[0].1.clone(),
                key[1].0.clone(),
                key[1].1.clone(),
            );
            if seen.insert(k) {
                edge_values.push(json!({
                    "src": {"brain": a.0, "path": a.1},
                    "dst": {"brain": b.0, "path": b.1},
                    "kind": "explicit",
                    "score": 1.0,
                }));
            }
        }

        serde_json::to_string(&json!({"nodes": nodes, "edges": edge_values}))
            .map_err(|e| e.to_string())
    }
}
