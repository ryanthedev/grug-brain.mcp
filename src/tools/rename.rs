//! Rename-with-link-rewrite (Plan 2 Phase 2).
//!
//! Extends the basic rename in `write.rs` to atomically rewrite every
//! `[[wikilink]]` pointing at the renamed memory across the brain. The
//! transaction wraps three concerns:
//!   1. on-disk rewrite of referrer files (RAII staging + rename-into-place),
//!   2. SQLite reindex of the renamed file + every referrer (rusqlite tx),
//!   3. main file rename (`fs::rename`).
//!
//! Failure at any step rolls back: staged originals are restored, the DB
//! transaction is dropped (no commit), and the brain is left bit-identical to
//! its pre-rename state.

use super::GrugDb;
use crate::helpers::validate_memory_path;
use crate::parsing::{split_frontmatter_and_body, LINK_RE};
use crate::tools::indexing::{index_file, remove_file};
use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::NamedTempFile;

/// Test-only failure injection. When set to `Some(n)`, the rewrite loop will
/// return an `Err` on the n-th file write (1-indexed). Reset to `None` after
/// each test.
#[cfg(test)]
static FAIL_AFTER_REWRITES: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub fn set_fail_after_rewrites(n: usize) {
    FAIL_AFTER_REWRITES.store(n, Ordering::SeqCst);
}

#[cfg(test)]
fn check_fail_injection(n_done: usize) -> Result<(), String> {
    let trigger = FAIL_AFTER_REWRITES.load(Ordering::SeqCst);
    if trigger != 0 && n_done >= trigger {
        return Err("test-injected rewrite failure".to_string());
    }
    Ok(())
}

#[cfg(not(test))]
fn check_fail_injection(_n_done: usize) -> Result<(), String> {
    Ok(())
}

/// Rename a memory and rewrite all incoming wikilinks across the brain.
///
/// When `rewrite_links` is `false`, this delegates to the basic rename path
/// in `write.rs` and only updates the `links.target_path` index column (the
/// existing Phase 1 behavior). When `true`, every referrer file on disk is
/// rewritten as well, all inside one rusqlite transaction.
///
/// Returns `(new_rel_path, affected_paths)` where `affected_paths` includes
/// every rewritten referrer plus the renamed file itself (under its new path).
pub fn grug_rename_with_links(
    db: &mut GrugDb,
    old_category: &str,
    old_path_name: &str,
    new_category: &str,
    new_path_name: &str,
    brain_name: Option<&str>,
    rewrite_links: bool,
) -> Result<(String, Vec<String>), String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(brain_name)?.clone();
    if !brain.writable {
        return Err(format!("brain \"{}\" is read-only", brain.name));
    }

    validate_memory_path(old_category)?;
    validate_memory_path(old_path_name)?;
    validate_memory_path(new_category)?;
    validate_memory_path(new_path_name)?;

    let old_cat = crate::helpers::slugify(old_category);
    let old_slug = crate::helpers::slugify(old_path_name);
    let new_cat = crate::helpers::slugify(new_category);
    let new_slug = crate::helpers::slugify(new_path_name);
    if old_cat.is_empty() || old_slug.is_empty() || new_cat.is_empty() || new_slug.is_empty() {
        return Err("category or path slugifies to empty".to_string());
    }

    let old_rel = format!("{old_cat}/{old_slug}.md");
    let new_rel = format!("{new_cat}/{new_slug}.md");
    if old_rel == new_rel {
        return Ok((old_rel, Vec::new()));
    }

    let old_full = brain.dir.join(&old_cat).join(format!("{old_slug}.md"));
    let new_full = brain.dir.join(&new_cat).join(format!("{new_slug}.md"));
    if !old_full.exists() {
        return Err(format!("source not found: {old_rel}"));
    }
    if new_full.exists() {
        return Err(format!("destination already exists: {new_rel}"));
    }

    if !rewrite_links {
        // Delegate to the simpler path. Wrap its String result into our
        // (new_rel, affected) shape.
        crate::tools::write::grug_rename(
            db,
            old_category,
            old_path_name,
            new_category,
            new_path_name,
            brain_name,
        )?;
        return Ok((new_rel.clone(), vec![old_rel, new_rel]));
    }

    // ----- rewrite path -----

    // The memory's frontmatter `name:` may differ from its slug. Discovery
    // must consider both so aliased/bare links via the `name` column are
    // captured.
    let old_name: Option<String> = db
        .conn()
        .query_row(
            "SELECT name FROM brain_fts WHERE brain = ?1 AND path = ?2",
            rusqlite::params![&brain.name, &old_rel],
            |row| row.get(0),
        )
        .ok();
    let referrer_paths =
        find_referrer_paths(db.conn(), &brain.name, &old_rel, &old_slug, &old_cat, old_name.as_deref())?;

    // Precompute the set of "old forms" we'll match against link targets.
    // `extra_name` covers the case where the file's frontmatter `name:`
    // differs from its slug -- bare `[[old-name]]` references resolve via
    // that field and must rewrite the same way.
    let extra_name = old_name
        .as_deref()
        .filter(|n| !n.is_empty() && *n != old_slug)
        .map(|n| n.to_string());
    let old_forms = OldForms {
        bare: old_slug.clone(),
        cat_slug: format!("{old_cat}/{old_slug}"),
        cat_slug_md: old_rel.clone(),
        extra_name,
    };
    let new_forms = NewForms {
        bare: new_slug.clone(),
        cat_slug: format!("{new_cat}/{new_slug}"),
        cat_slug_md: new_rel.clone(),
    };

    // Lock both endpoints to serialize against parallel renames/writes.
    let key_a = (brain.name.clone(), old_rel.clone());
    let key_b = (brain.name.clone(), new_rel.clone());
    let (first, second) = if key_a < key_b {
        (key_a.clone(), key_b.clone())
    } else {
        (key_b.clone(), key_a.clone())
    };
    let lock_first = db.path_locks().for_path(&first.0, &first.1);
    let lock_second = db.path_locks().for_path(&second.0, &second.1);
    let _g1 = lock_first.lock().expect("path lock poisoned");
    let _g2 = lock_second.lock().expect("path lock poisoned");

    // Set up staging RAII guard. Drop will restore originals on Err.
    let mut guard = StagingGuard::new(&brain.dir)
        .map_err(|e| format!("staging dir: {e}"))?;

    // Phase A: stage + rewrite each referrer file in-place.
    for ref_rel in &referrer_paths {
        let ref_full = brain.dir.join(ref_rel);
        if !ref_full.exists() {
            // Stale index row -- skip, the reindex below will clean it up.
            continue;
        }
        let original = fs::read_to_string(&ref_full)
            .map_err(|e| format!("read referrer {ref_rel}: {e}"))?;

        let rewritten = rewrite_link_text(&original, &old_forms, &new_forms);

        if rewritten == original {
            // Nothing to do for this file (e.g. link only inside code spans).
            continue;
        }

        guard.stage(&ref_full)?;
        atomic_write(&ref_full, rewritten.as_bytes())
            .map_err(|e| format!("rewrite {ref_rel}: {e}"))?;
        guard.note_rewritten(&ref_full);

        // Optional injected failure for the rollback test.
        check_fail_injection(guard.staged.len())?;
    }

    // Phase B: rename main file. We also rewrite its frontmatter `name:` to
    // the new slug so bare-name `[[new-slug]]` references in referrers
    // resolve back to this file after reindex. Stage the original first so
    // rollback restores it.
    if let Some(parent) = new_full.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create parent: {e}"))?;
    }
    let old_main_content = fs::read_to_string(&old_full)
        .map_err(|e| format!("read old main: {e}"))?;
    let new_main_content = rewrite_frontmatter_name(&old_main_content, &new_slug);
    fs::rename(&old_full, &new_full)
        .map_err(|e| format!("failed to rename: {e}"))?;
    guard.set_main_rename(&new_full, &old_full);
    if new_main_content != old_main_content {
        atomic_write(&new_full, new_main_content.as_bytes())
            .map_err(|e| format!("rewrite frontmatter: {e}"))?;
        // After rewrite, the file at new_full has new content. On rollback,
        // we'll need to also restore the OLD content. Stage it now so the
        // RAII guard knows about it.
        guard.stage_pre_rename_main(&old_full, &old_main_content)?;
    }

    // Phase C: index updates inside a single transaction.
    let tx = db
        .conn()
        .unchecked_transaction()
        .map_err(|e| format!("begin tx: {e}"))?;

    remove_file(&tx, &brain.name, &old_rel)?;
    index_file(&tx, &brain.name, &new_rel, &new_full, &new_cat)?;

    // Reindex every rewritten referrer so FTS body / links / tags / TF-IDF
    // reflect the rewritten content.
    let mut affected: Vec<String> = Vec::with_capacity(referrer_paths.len() + 2);
    affected.push(old_rel.clone());
    affected.push(new_rel.clone());
    for ref_rel in &referrer_paths {
        let ref_full = brain.dir.join(ref_rel);
        if !ref_full.exists() {
            // Already deleted on disk -- ensure index has no stale row.
            let _ = remove_file(&tx, &brain.name, ref_rel);
            continue;
        }
        let cat = ref_rel.split('/').next().unwrap_or("").to_string();
        index_file(&tx, &brain.name, ref_rel, &ref_full, &cat)?;
        affected.push(ref_rel.clone());
    }

    // Defense-in-depth: any link rows that still point at the old path get
    // their `target_path` rewritten to the new one. After the reindex above
    // these are normally already correct, but unresolved-by-name rows we may
    // not have rewritten on-disk get patched here.
    tx.execute(
        "UPDATE links SET target_path = ?1 WHERE target_brain = ?2 AND target_path = ?3",
        rusqlite::params![&new_rel, &brain.name, &old_rel],
    )
    .map_err(|e| format!("update incoming links: {e}"))?;

    tx.commit().map_err(|e| format!("commit tx: {e}"))?;

    db.enqueue_git_commit(&brain.name, &old_rel, "delete");
    db.enqueue_git_commit(&brain.name, &new_rel, "write");
    for ref_rel in &referrer_paths {
        db.enqueue_git_commit(&brain.name, ref_rel, "write");
    }

    guard.commit();
    affected.sort();
    affected.dedup();
    Ok((new_rel, affected))
}

// ---------------------------------------------------------------------------
// Referrer discovery
// ---------------------------------------------------------------------------

fn find_referrer_paths(
    conn: &rusqlite::Connection,
    brain_name: &str,
    old_rel: &str,
    old_slug: &str,
    old_cat: &str,
    old_name: Option<&str>,
) -> Result<Vec<String>, String> {
    // Collect referrers via three queries unioned in-memory:
    //   1. resolved links pointing at the old path,
    //   2. unresolved links by bare name (`old_slug`),
    //   3. unresolved links by category/slug form.
    let mut set: HashSet<String> = HashSet::new();
    let cat_slug = format!("{old_cat}/{old_slug}");
    let cat_slug_md = old_rel.to_string();

    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT src_path FROM links
             WHERE brain = ?1 AND target_brain = ?2 AND target_path = ?3",
        )
        .map_err(|e| format!("prepare resolved: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![brain_name, brain_name, old_rel], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| format!("query resolved: {e}"))?;
    for r in rows {
        if let Ok(s) = r {
            set.insert(s);
        }
    }

    // Build the list of "name forms" we'll match unresolved targets against:
    // the slug, the category/slug variants, and the frontmatter `name:` (if
    // it differs from the slug).
    let mut exact_forms: Vec<String> = vec![
        old_slug.to_string(),
        cat_slug.clone(),
        cat_slug_md.clone(),
    ];
    if let Some(n) = old_name {
        if !n.is_empty() && n != old_slug {
            exact_forms.push(n.to_string());
        }
    }

    // Unresolved by bare/path/name forms, exact match.
    let placeholders: Vec<String> = (0..exact_forms.len())
        .map(|i| format!("?{}", i + 2))
        .collect();
    let sql = format!(
        "SELECT DISTINCT src_path FROM links
         WHERE brain = ?1 AND target_path IS NULL AND target_name_unresolved IN ({})",
        placeholders.join(", ")
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare unresolved: {e}"))?;
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&brain_name];
    for f in &exact_forms {
        params.push(f);
    }
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| format!("query unresolved: {e}"))?;
    for r in rows {
        if let Ok(s) = r {
            set.insert(s);
        }
    }

    // Unresolved aliased forms: `target|alias` is stored verbatim by the
    // indexer (see `resolve_link`). Match the prefix `<form>|`.
    let bare_pipe = format!("{old_slug}|%");
    let cs_pipe = format!("{cat_slug}|%");
    let csmd_pipe = format!("{cat_slug_md}|%");
    let name_pipe = old_name.map(|n| format!("{n}|%"));
    let mut alias_patterns: Vec<&String> = vec![&bare_pipe, &cs_pipe, &csmd_pipe];
    if let Some(p) = &name_pipe {
        alias_patterns.push(p);
    }
    let alias_placeholders: Vec<String> = (0..alias_patterns.len())
        .map(|i| format!("target_name_unresolved LIKE ?{} ESCAPE '\\'", i + 2))
        .collect();
    let alias_sql = format!(
        "SELECT DISTINCT src_path FROM links
         WHERE brain = ?1 AND target_path IS NULL AND ({})",
        alias_placeholders.join(" OR ")
    );
    let mut stmt = conn
        .prepare(&alias_sql)
        .map_err(|e| format!("prepare unresolved-alias: {e}"))?;
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&brain_name];
    for p in &alias_patterns {
        params.push(*p);
    }
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| format!("query unresolved-alias: {e}"))?;
    for r in rows {
        if let Ok(s) = r {
            set.insert(s);
        }
    }

    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    Ok(out)
}

// ---------------------------------------------------------------------------
// Link text rewrite
// ---------------------------------------------------------------------------

struct OldForms {
    bare: String,
    cat_slug: String,
    cat_slug_md: String,
    /// Frontmatter `name:` if it differs from slug; matched as another bare
    /// form (replacement is `new.bare`).
    extra_name: Option<String>,
}

struct NewForms {
    bare: String,
    cat_slug: String,
    cat_slug_md: String,
}

/// Rewrite every wikilink target in `content` that resolves to one of the old
/// forms, replacing it with the matching new form. Aliases (`[[name|alias]]`)
/// preserve the alias text. Frontmatter and fenced code blocks are left
/// byte-identical; inline backtick spans are skipped.
fn rewrite_link_text(content: &str, old: &OldForms, new: &NewForms) -> String {
    let (fm, body) = split_frontmatter_and_body(content);
    let rewritten_body = rewrite_body_links(body, old, new);
    let mut out = String::with_capacity(content.len() + 16);
    out.push_str(fm);
    out.push_str(&rewritten_body);
    out
}

fn rewrite_body_links(body: &str, old: &OldForms, new: &NewForms) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_fence = false;
    let mut first = true;
    for line in body.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;

        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&rewrite_line_links(line, old, new));
    }
    out
}

/// Rewrite wikilinks in a single line, skipping any that fall inside an
/// inline backtick span. Returns owned String only when changed; for
/// simplicity we always return owned.
fn rewrite_line_links(line: &str, old: &OldForms, new: &NewForms) -> String {
    // Build a mask of byte indices that are inside inline-code spans so we
    // skip rewriting links there.
    let mut in_code = vec![false; line.len()];
    let mut active = false;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            active = !active;
            in_code[i] = active; // the backtick itself: mark as code if entering
            i += 1;
            continue;
        }
        in_code[i] = active;
        i += 1;
    }

    let mut out = String::with_capacity(line.len());
    let mut last = 0;
    for m in LINK_RE.captures_iter(line) {
        let whole = m.get(0).expect("group 0 always present");
        let start = whole.start();
        let end = whole.end();
        // Skip if the link starts inside a code span.
        if in_code.get(start).copied().unwrap_or(false) {
            continue;
        }
        let inner = m.get(1).expect("group 1 always present").as_str();
        let (target, alias_with_pipe) = split_target_alias(inner);
        let target_trim = target.trim();
        let new_target = if target_trim == old.bare {
            Some(new.bare.as_str())
        } else if target_trim == old.cat_slug {
            Some(new.cat_slug.as_str())
        } else if target_trim == old.cat_slug_md {
            Some(new.cat_slug_md.as_str())
        } else if old
            .extra_name
            .as_deref()
            .map(|n| n == target_trim)
            .unwrap_or(false)
        {
            Some(new.bare.as_str())
        } else {
            None
        };
        let Some(new_target) = new_target else {
            continue;
        };
        // Emit the unchanged text before this match, then the replacement.
        out.push_str(&line[last..start]);
        out.push_str("[[");
        out.push_str(new_target);
        out.push_str(alias_with_pipe);
        out.push_str("]]");
        last = end;
    }
    if last == 0 {
        return line.to_string();
    }
    out.push_str(&line[last..]);
    out
}

/// Rewrite the frontmatter `name:` field if present. The rest of the file
/// (frontmatter ordering, body bytes) is preserved.
fn rewrite_frontmatter_name(content: &str, new_name: &str) -> String {
    let (fm, body) = split_frontmatter_and_body(content);
    if fm.is_empty() {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len() + 16);
    let mut replaced = false;
    let mut first = true;
    for line in fm.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        if !replaced && line.trim_start().starts_with("name:") {
            // Preserve leading whitespace.
            let leading_len = line.len() - line.trim_start().len();
            out.push_str(&line[..leading_len]);
            out.push_str("name: ");
            out.push_str(new_name);
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    out.push_str(body);
    out
}

/// Split `target|alias` into `(target, "|alias")`. If no pipe, alias is "".
fn split_target_alias(inner: &str) -> (&str, &str) {
    match inner.find('|') {
        Some(i) => (&inner[..i], &inner[i..]),
        None => (inner, ""),
    }
}

// ---------------------------------------------------------------------------
// Atomic write (mirrors write.rs::atomic_write -- duplicated to avoid making
// it pub from write.rs)
// ---------------------------------------------------------------------------

fn atomic_write(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no parent")
    })?;
    let mut tmp = NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    tmp.persist(target).map_err(|e| e.error)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// StagingGuard: RAII rollback for file rewrites + main rename
// ---------------------------------------------------------------------------

struct StagingGuard {
    staging_root: PathBuf,
    /// (original_abs_path, staged_copy_abs_path)
    staged: Vec<(PathBuf, PathBuf)>,
    /// Set once the main file has been renamed; on rollback we move it back.
    main_rename: Option<(PathBuf, PathBuf)>,
    /// Original bytes of the main file pre-frontmatter-rewrite. If set, after
    /// rolling the rename back we write these bytes back to the (now-restored)
    /// old path so the file is bit-identical to its pre-call state.
    main_pre_bytes: Option<(PathBuf, Vec<u8>)>,
    committed: bool,
    counter: usize,
}

impl StagingGuard {
    fn new(brain_dir: &Path) -> std::io::Result<Self> {
        // One staging dir per call so concurrent renames don't collide.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let staging_root = brain_dir
            .join(".grug-rename-tmp")
            .join(format!("{nanos:x}-{}", std::process::id()));
        fs::create_dir_all(&staging_root)?;
        Ok(Self {
            staging_root,
            staged: Vec::new(),
            main_rename: None,
            main_pre_bytes: None,
            committed: false,
            counter: 0,
        })
    }

    /// Copy the current contents of `original` into the staging dir before
    /// the caller overwrites it. We use a copy (not a move) so the original
    /// remains in place during the `atomic_write` rename-into-place.
    fn stage(&mut self, original: &Path) -> Result<(), String> {
        self.counter += 1;
        let staged = self.staging_root.join(format!("{}.bak", self.counter));
        fs::copy(original, &staged)
            .map_err(|e| format!("stage {}: {e}", original.display()))?;
        self.staged.push((original.to_path_buf(), staged));
        Ok(())
    }

    /// Mark that `original` was successfully overwritten; on rollback we'll
    /// restore from its staged copy.
    fn note_rewritten(&mut self, _original: &Path) {
        // Stage list already records the pair; this hook exists to make the
        // call site read clearly and to keep open the option of differentiating
        // staged-but-not-yet-written vs staged-and-overwritten in the future.
    }

    fn set_main_rename(&mut self, new_full: &Path, old_full: &Path) {
        self.main_rename = Some((new_full.to_path_buf(), old_full.to_path_buf()));
    }

    /// Record the pre-frontmatter-rewrite bytes of the main file so rollback
    /// can restore them after renaming-back.
    fn stage_pre_rename_main(&mut self, old_full: &Path, original: &str) -> Result<(), String> {
        self.main_pre_bytes = Some((old_full.to_path_buf(), original.as_bytes().to_vec()));
        Ok(())
    }

    fn commit(mut self) {
        self.committed = true;
        // Best-effort cleanup of staging dir.
        let _ = fs::remove_dir_all(&self.staging_root);
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // 1) Undo the main rename if it happened.
        if let Some((new_full, old_full)) = &self.main_rename {
            let _ = fs::rename(new_full, old_full);
            // Restore pre-frontmatter-rewrite bytes if we mutated them.
            if let Some((path, bytes)) = &self.main_pre_bytes {
                if path == old_full {
                    let _ = fs::write(old_full, bytes);
                }
            }
        }
        // 2) Restore each staged original (overwrites the in-place rewrite).
        for (orig, staged) in self.staged.iter().rev() {
            // Use a copy + remove so we don't hit cross-device issues; the
            // staging dir lives under the same brain root, but defensive
            // either way.
            if let Ok(()) = fs::copy(staged, orig).map(|_| ()) {
                let _ = fs::remove_file(staged);
            }
        }
        // 3) Drop staging tree.
        let _ = fs::remove_dir_all(&self.staging_root);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_helpers::test_db;
    use crate::tools::write::grug_write;
    use std::sync::Mutex;

    /// Serialize failure-injection tests so they can't observe each other's
    /// global atomic state. Held for the duration of the test that uses it.
    static INJECT_LOCK: Mutex<()> = Mutex::new(());

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    // ----- DW-2.1: link rewrite forms -----

    fn old_new(old_slug: &str, old_cat: &str, new_slug: &str, new_cat: &str) -> (OldForms, NewForms) {
        (
            OldForms {
                bare: old_slug.to_string(),
                cat_slug: format!("{old_cat}/{old_slug}"),
                cat_slug_md: format!("{old_cat}/{old_slug}.md"),
                extra_name: None,
            },
            NewForms {
                bare: new_slug.to_string(),
                cat_slug: format!("{new_cat}/{new_slug}"),
                cat_slug_md: format!("{new_cat}/{new_slug}.md"),
            },
        )
    }

    #[test]
    fn test_dw_2_1_rewrites_bare_link() {
        let (o, n) = old_new("old-slug", "notes", "new-slug", "notes");
        let body = "See [[old-slug]] there.";
        let out = rewrite_link_text(body, &o, &n);
        assert_eq!(out, "See [[new-slug]] there.");
    }

    #[test]
    fn test_dw_2_1_rewrites_aliased_link_preserves_alias() {
        let (o, n) = old_new("old-slug", "notes", "new-slug", "notes");
        let body = "Check [[old-slug|the original]].";
        let out = rewrite_link_text(body, &o, &n);
        assert_eq!(out, "Check [[new-slug|the original]].");
    }

    #[test]
    fn test_dw_2_1_rewrites_categoryslug_form() {
        let (o, n) = old_new("old-slug", "notes", "new-slug", "refs");
        let body = "[[notes/old-slug]] and [[notes/old-slug.md]]";
        let out = rewrite_link_text(body, &o, &n);
        assert_eq!(out, "[[refs/new-slug]] and [[refs/new-slug.md]]");
    }

    #[test]
    fn test_dw_2_1_does_not_rewrite_inside_fenced_code() {
        let (o, n) = old_new("old-slug", "notes", "new-slug", "notes");
        let body = "before\n```\n[[old-slug]]\n```\nafter [[old-slug]]";
        let out = rewrite_link_text(body, &o, &n);
        // Code-fenced occurrence stays; outside-fence one is rewritten.
        assert!(out.contains("```\n[[old-slug]]\n```"));
        assert!(out.contains("after [[new-slug]]"));
    }

    #[test]
    fn test_dw_2_1_does_not_rewrite_inside_inline_code() {
        let (o, n) = old_new("old-slug", "notes", "new-slug", "notes");
        let body = "use `[[old-slug]]` literally and [[old-slug]] for real";
        let out = rewrite_link_text(body, &o, &n);
        assert_eq!(
            out,
            "use `[[old-slug]]` literally and [[new-slug]] for real"
        );
    }

    #[test]
    fn test_dw_2_1_preserves_frontmatter_byte_identical() {
        let (o, n) = old_new("old-slug", "notes", "new-slug", "notes");
        let content = "---\nname: x\ntype: memory\n---\n\n[[old-slug]]\n";
        let out = rewrite_link_text(content, &o, &n);
        assert!(out.starts_with("---\nname: x\ntype: memory\n---\n"));
        assert!(out.ends_with("[[new-slug]]\n"));
    }

    #[test]
    fn test_dw_2_1_does_not_rewrite_unrelated_links() {
        let (o, n) = old_new("old-slug", "notes", "new-slug", "notes");
        let body = "[[other-thing]] and [[notes/something-else]]";
        let out = rewrite_link_text(body, &o, &n);
        assert_eq!(out, body);
    }

    // ----- DW-2.5: integration on the rename path -----

    #[test]
    fn test_dw_2_5_rename_with_link_rewrite_updates_referrers() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, tmp) = test_db();

        // Target the rename will hit.
        grug_write(
            &mut db,
            "notes",
            "target",
            "---\nname: target-b\ntype: note\n---\n\nTarget body.",
            None,
            None,
        )
        .unwrap();

        // Three referrers in different forms.
        grug_write(
            &mut db,
            "notes",
            "ref-bare",
            "---\nname: ref1\ntype: note\n---\n\nLinks to [[target-b]].",
            None,
            None,
        )
        .unwrap();
        grug_write(
            &mut db,
            "notes",
            "ref-alias",
            "---\nname: ref2\ntype: note\n---\n\nSee [[target-b|the target]].",
            None,
            None,
        )
        .unwrap();
        grug_write(
            &mut db,
            "notes",
            "ref-path",
            "---\nname: ref3\ntype: note\n---\n\nPath form [[notes/target.md]].",
            None,
            None,
        )
        .unwrap();

        let (new_rel, affected) = grug_rename_with_links(
            &mut db,
            "notes",
            "target",
            "refs",
            "renamed-target",
            None,
            true,
        )
        .unwrap();
        assert_eq!(new_rel, "refs/renamed-target.md");
        assert!(affected.contains(&"notes/ref-bare.md".to_string()));
        assert!(affected.contains(&"notes/ref-alias.md".to_string()));
        assert!(affected.contains(&"notes/ref-path.md".to_string()));
        assert!(affected.contains(&"refs/renamed-target.md".to_string()));

        // Verify on-disk content rewritten.
        let bare = read(&tmp.path().join("memories/notes/ref-bare.md"));
        assert!(bare.contains("[[renamed-target]]"));
        let alias = read(&tmp.path().join("memories/notes/ref-alias.md"));
        assert!(alias.contains("[[renamed-target|the target]]"));
        let pathf = read(&tmp.path().join("memories/notes/ref-path.md"));
        assert!(pathf.contains("[[refs/renamed-target.md]]"));
    }

    #[test]
    fn test_dw_2_5_rename_with_10_referrers() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, _tmp) = test_db();
        grug_write(
            &mut db,
            "notes",
            "hub",
            "---\nname: hub\n---\n\nThe hub.",
            None,
            None,
        )
        .unwrap();
        for i in 0..10 {
            grug_write(
                &mut db,
                "notes",
                &format!("r{i}"),
                &format!("---\nname: r{i}\n---\n\nRef [[hub]] body."),
                None,
                None,
            )
            .unwrap();
        }
        let (_, affected) = grug_rename_with_links(
            &mut db,
            "notes",
            "hub",
            "notes",
            "central",
            None,
            true,
        )
        .unwrap();
        // 10 referrers + old + new = 12 entries
        assert_eq!(affected.len(), 12);

        // Verify all referrers rewritten in DB (link target points to new path)
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM links WHERE brain = 'memories' AND target_path = 'notes/central.md'",
                [], |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 10, "all 10 referrers should index the new target_path");
    }

    // ----- DW-2.3: index consistency -----

    #[test]
    fn test_dw_2_3_no_stale_fts_or_link_rows_for_old_path() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, _tmp) = test_db();
        grug_write(
            &mut db,
            "notes",
            "old",
            "---\nname: old\n---\n\nbody",
            None,
            None,
        )
        .unwrap();
        grug_write(
            &mut db,
            "notes",
            "src",
            "---\nname: src\n---\n\nLinks [[old]]",
            None,
            None,
        )
        .unwrap();

        grug_rename_with_links(&mut db, "notes", "old", "notes", "renamed", None, true).unwrap();

        let fts: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM brain_fts WHERE path = 'notes/old.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts, 0, "no stale FTS rows for old path");

        let stale_target: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM links WHERE target_path = 'notes/old.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_target, 0, "no link rows still target old path");

        let new_target: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM links WHERE target_path = 'notes/renamed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_target, 1, "src now targets new path");
    }

    // ----- DW-7.7 regression: extra_name form after rename must resolve in DB -----

    #[test]
    fn test_dw_7_7_extra_name_backlink_resolves_after_rename() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, _tmp) = test_db();

        // Target file has frontmatter `name: Hello World` (not the slug).
        grug_write(
            &mut db,
            "notes",
            "hello",
            "---\nname: Hello World\ndate: 2025-01-01\ndescription: A greeting\n---\n\n# Hello World\n",
            None,
            None,
        )
        .unwrap();

        // Source links via the frontmatter name (not slug).
        grug_write(
            &mut db,
            "notes",
            "source",
            "---\nname: source\ndate: 2025-01-01\ntype: memory\n---\n\nsee [[Hello World]] for details",
            None,
            None,
        )
        .unwrap();

        // Verify pre-rename state: source.md → hello.md exists in links.
        let pre_count: i32 = db.conn().query_row(
            "SELECT COUNT(*) FROM links WHERE src_path = 'notes/source.md' AND target_path = 'notes/hello.md'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(pre_count, 1, "source.md should link to hello.md before rename");

        // Rename hello → hello-renamed (with link rewrite).
        grug_rename_with_links(
            &mut db,
            "notes", "hello",
            "notes", "hello-renamed",
            None,
            true,
        ).unwrap();

        // Post-rename: source.md should link to hello-renamed.md.
        let new_count: i32 = db.conn().query_row(
            "SELECT COUNT(*) FROM links WHERE src_path = 'notes/source.md' AND target_path = 'notes/hello-renamed.md'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(new_count, 1, "source.md should link to hello-renamed.md after rename");

        // Old link should be gone.
        let stale: i32 = db.conn().query_row(
            "SELECT COUNT(*) FROM links WHERE target_path = 'notes/hello.md'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(stale, 0, "no stale links pointing at old path");
    }

    // ----- DW-2.7: rewrite_links flag -----

    #[test]
    fn test_dw_2_7_rewrite_links_false_skips_on_disk_rewrite() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, tmp) = test_db();
        grug_write(
            &mut db,
            "notes",
            "old",
            "---\nname: old\n---\n\nbody",
            None,
            None,
        )
        .unwrap();
        grug_write(
            &mut db,
            "notes",
            "src",
            "---\nname: src\n---\n\nLinks [[old]]",
            None,
            None,
        )
        .unwrap();
        grug_rename_with_links(
            &mut db,
            "notes",
            "old",
            "notes",
            "renamed",
            None,
            false,
        )
        .unwrap();
        let src_body = read(&tmp.path().join("memories/notes/src.md"));
        assert!(
            src_body.contains("[[old]]"),
            "rewrite_links=false must NOT touch on-disk text"
        );
    }

    #[test]
    fn test_dw_2_7_rewrite_links_true_rewrites_on_disk() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, tmp) = test_db();
        grug_write(
            &mut db,
            "notes",
            "old",
            "---\nname: old\n---\n\nbody",
            None,
            None,
        )
        .unwrap();
        grug_write(
            &mut db,
            "notes",
            "src",
            "---\nname: src\n---\n\nLinks [[old]]",
            None,
            None,
        )
        .unwrap();
        grug_rename_with_links(
            &mut db,
            "notes",
            "old",
            "notes",
            "renamed",
            None,
            true,
        )
        .unwrap();
        let src_body = read(&tmp.path().join("memories/notes/src.md"));
        assert!(src_body.contains("[[renamed]]"));
        assert!(!src_body.contains("[[old]]"));
    }

    // ----- DW-2.2 + DW-2.6: atomicity / rollback -----

    #[test]
    fn test_dw_2_6_induced_failure_bit_identical_rollback() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, tmp) = test_db();
        // Create target + 3 referrers.
        grug_write(
            &mut db,
            "notes",
            "target",
            "---\nname: target\n---\n\nbody",
            None,
            None,
        )
        .unwrap();
        for i in 0..3 {
            grug_write(
                &mut db,
                "notes",
                &format!("ref{i}"),
                &format!("---\nname: ref{i}\n---\n\nLink [[target]] body{i}"),
                None,
                None,
            )
            .unwrap();
        }

        // Snapshot brain state (file paths + bytes).
        let pre = snapshot_brain(tmp.path());

        // Inject failure on the 2nd file rewrite.
        set_fail_after_rewrites(2);
        let result = grug_rename_with_links(
            &mut db,
            "notes",
            "target",
            "refs",
            "renamed",
            None,
            true,
        );
        set_fail_after_rewrites(0);
        assert!(result.is_err(), "rewrite must fail with injected error");

        // After rollback, brain bytes must equal pre-snapshot.
        let post = snapshot_brain(tmp.path());
        assert_eq!(
            pre, post,
            "brain must be bit-identical after rollback; diff in keys/bytes"
        );

        // And the renamed file must NOT exist at the new path.
        assert!(!tmp.path().join("memories/refs/renamed.md").exists());
        assert!(tmp.path().join("memories/notes/target.md").exists());
    }

    #[test]
    fn test_dw_2_2_db_rolls_back_when_files_fail() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // After an injected failure, the DB rows for the old path must still
        // exist and no FTS rows for the new path may be present.
        let (mut db, _tmp) = test_db();
        grug_write(
            &mut db,
            "notes",
            "target",
            "---\nname: target\n---\n\nbody",
            None,
            None,
        )
        .unwrap();
        grug_write(
            &mut db,
            "notes",
            "src",
            "---\nname: src\n---\n\nLink [[target]]",
            None,
            None,
        )
        .unwrap();

        set_fail_after_rewrites(1);
        let r = grug_rename_with_links(
            &mut db,
            "notes",
            "target",
            "refs",
            "renamed",
            None,
            true,
        );
        set_fail_after_rewrites(0);
        assert!(r.is_err());

        let old_present: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM brain_fts WHERE path = 'notes/target.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_present, 1, "old FTS row must persist after rollback");
        let new_present: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM brain_fts WHERE path = 'refs/renamed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_present, 0, "no FTS row for new path after rollback");
    }

    fn snapshot_brain(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mem = root.join("memories");
        let mut out = Vec::new();
        walk(&mem, &mem, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for ent in entries.flatten() {
            let path = ent.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            // Skip the staging dir we use for rollback.
            if name == ".grug-rename-tmp" {
                continue;
            }
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                let bytes = std::fs::read(&path).unwrap_or_default();
                out.push((rel, bytes));
            }
        }
    }

    // ----- DW-2.8: performance -----

    #[test]
    #[cfg_attr(debug_assertions, ignore)]
    fn test_dw_2_8_rename_100_referrers_under_1s() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, _tmp) = test_db();
        grug_write(
            &mut db,
            "notes",
            "hub",
            "---\nname: hub\n---\n\nbody",
            None,
            None,
        )
        .unwrap();
        for i in 0..100 {
            grug_write(
                &mut db,
                "notes",
                &format!("r{i:03}"),
                &format!("---\nname: r{i}\n---\n\n[[hub]] body{i}"),
                None,
                None,
            )
            .unwrap();
        }
        let t0 = std::time::Instant::now();
        grug_rename_with_links(
            &mut db,
            "notes",
            "hub",
            "notes",
            "central",
            None,
            true,
        )
        .unwrap();
        let elapsed = t0.elapsed();
        assert!(
            elapsed.as_secs_f64() < 1.0,
            "100-referrer rename took {elapsed:?}, budget 1s"
        );
    }

    // ----- read-only brain rejection -----

    #[test]
    fn test_readonly_brain_rejected() {
        let _g = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut db, _tmp) = crate::tools::test_helpers::test_db_multi();
        let r = grug_rename_with_links(
            &mut db,
            "notes",
            "x",
            "notes",
            "y",
            Some("docs"),
            true,
        );
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("read-only"));
    }
}
