use super::GrugDb;
use crate::git::get_hostname;
use crate::helpers::{slugify, today, validate_memory_path};
use crate::tools::indexing::index_file;
use crate::tools::write::{atomic_write, ensure_dir};
use std::fs;

const CATEGORY: &str = "conversations";

pub fn grug_conversation(
    db: &mut GrugDb,
    action: &str,
    title: Option<&str>,
    message: Option<&str>,
    identity: Option<&str>,
    status: Option<&str>,
    brain_name: Option<&str>,
) -> Result<String, String> {
    db.maybe_reload_config();
    match action {
        "open" | "start" | "new" | "begin" | "create" => open(db, title, message, identity, brain_name),
        "reply" | "post" | "message" | "add" | "respond" => reply(db, title, message, identity, brain_name),
        "list" | "ls" => list(db, brain_name),
        "close" | "resolve" | "done" => close(db, title, brain_name),
        "status" => set_status(db, title, status, brain_name),
        _ => Ok(format!(
            "unknown action: {action}\n\n\
             ## grug-conversation usage\n\n\
             | action | required params | description |\n\
             |--------|----------------|-------------|\n\
             | open | title, message | start a new thread |\n\
             | reply | title, message | post to existing thread |\n\
             | list | (none) | show all threads |\n\
             | close | title | mark thread resolved |\n\
             | status | title, status | set custom status |\n\n\
             `identity` is optional (defaults to hostname).\n\
             `brain` is optional (defaults to first writable brain with a git remote)."
        )),
    }
}

/// Resolve the brain for conversations: prefer a writable brain with a git
/// remote so conversations sync across machines. Falls back to primary.
fn resolve_conversation_brain<'a>(db: &'a GrugDb, name: Option<&str>) -> Result<&'a crate::types::Brain, String> {
    if let Some(n) = name {
        return db.resolve_brain(Some(n));
    }
    let config = db.config();
    config.brains.iter()
        .find(|b| b.writable && b.git.is_some())
        .ok_or_else(|| "no writable brain with a git remote configured".to_string())
        .or_else(|_| db.resolve_brain(None))
}

fn resolve_identity(identity: Option<&str>) -> String {
    identity.map(String::from).unwrap_or_else(get_hostname)
}

fn open(
    db: &mut GrugDb,
    title: Option<&str>,
    message: Option<&str>,
    identity: Option<&str>,
    brain_name: Option<&str>,
) -> Result<String, String> {
    let title = title.ok_or("missing field: title")?;
    let message = message.ok_or("missing field: message")?;
    validate_memory_path(title)?;

    let brain = resolve_conversation_brain(db, brain_name)?.clone();
    if !brain.writable {
        return Ok(format!("brain \"{}\" is read-only", brain.name));
    }

    let slug = slugify(title);
    if slug.is_empty() {
        return Err("title slugifies to empty".to_string());
    }

    let cat_dir = brain.dir.join(CATEGORY);
    ensure_dir(&cat_dir);

    let file_path = cat_dir.join(format!("{slug}.md"));
    let rel_path = format!("{CATEGORY}/{slug}.md");

    if file_path.exists() {
        return Err(format!("conversation already exists: {slug}"));
    }

    let who = resolve_identity(identity);
    let date = today();
    let content = format!(
        "---\ntitle: \"{title}\"\ndate: {date}\nstatus: open\nparticipants: [{who}]\n---\n\n\
         ### Message 1 — {who} ({date})\n\n{message}\n"
    );

    atomic_write(&file_path, content.as_bytes())
        .map_err(|e| format!("failed to write {}: {e}", file_path.display()))?;

    index_file(db.conn(), &brain.name, &rel_path, &file_path, CATEGORY)?;
    db.enqueue_git_commit(&brain.name, &rel_path, "write");

    Ok(format!("opened conversation: {slug}"))
}

fn reply(
    db: &mut GrugDb,
    title: Option<&str>,
    message: Option<&str>,
    identity: Option<&str>,
    brain_name: Option<&str>,
) -> Result<String, String> {
    let title = title.ok_or("missing field: title")?;
    let message = message.ok_or("missing field: message")?;

    let brain = resolve_conversation_brain(db, brain_name)?.clone();
    if !brain.writable {
        return Ok(format!("brain \"{}\" is read-only", brain.name));
    }

    let slug = slugify(title);
    let file_path = brain.dir.join(CATEGORY).join(format!("{slug}.md"));
    let rel_path = format!("{CATEGORY}/{slug}.md");

    if !file_path.exists() {
        return Err(format!("conversation not found: {slug}"));
    }

    let existing = fs::read_to_string(&file_path)
        .map_err(|e| format!("failed to read {}: {e}", file_path.display()))?;

    let msg_num = existing.matches("\n### Message ").count() + 1;
    let who = resolve_identity(identity);
    let date = today();

    // Add participant if not already listed
    let updated = add_participant(&existing, &who);

    let new_content = format!(
        "{updated}\n### Message {msg_num} — {who} ({date})\n\n{message}\n"
    );

    atomic_write(&file_path, new_content.as_bytes())
        .map_err(|e| format!("failed to write {}: {e}", file_path.display()))?;

    index_file(db.conn(), &brain.name, &rel_path, &file_path, CATEGORY)?;
    db.enqueue_git_commit(&brain.name, &rel_path, "write");

    Ok(format!("replied to {slug} (message {msg_num})"))
}

fn add_participant(content: &str, who: &str) -> String {
    if let Some(line_start) = content.find("participants: [") {
        let bracket_start = line_start + "participants: [".len();
        if let Some(bracket_end) = content[bracket_start..].find(']') {
            let participants_str = &content[bracket_start..bracket_start + bracket_end];
            let already = participants_str
                .split(',')
                .any(|p| p.trim() == who);
            if !already {
                let new_participants = if participants_str.is_empty() {
                    who.to_string()
                } else {
                    format!("{participants_str}, {who}")
                };
                return format!(
                    "{}{}{}",
                    &content[..bracket_start],
                    new_participants,
                    &content[bracket_start + bracket_end..]
                );
            }
        }
    }
    content.to_string()
}

fn list(db: &mut GrugDb, brain_name: Option<&str>) -> Result<String, String> {
    let brain = resolve_conversation_brain(db, brain_name)?.clone();
    let conv_dir = brain.dir.join(CATEGORY);

    if !conv_dir.exists() {
        return Ok("no conversations".to_string());
    }

    let mut entries: Vec<(String, String, String)> = Vec::new();

    for entry in fs::read_dir(&conv_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "md") {
            let content = fs::read_to_string(&path).unwrap_or_default();
            let title = extract_frontmatter_value(&content, "title")
                .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().to_string());
            let status = extract_frontmatter_value(&content, "status")
                .unwrap_or_else(|| "unknown".to_string());
            let slug = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            entries.push((slug, title, status));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    if entries.is_empty() {
        return Ok("no conversations".to_string());
    }

    let mut out = String::from("# conversations\n\n");
    for (slug, title, status) in &entries {
        out.push_str(&format!("- **{title}** `{slug}` [{status}]\n"));
    }
    Ok(out)
}

fn close(db: &mut GrugDb, title: Option<&str>, brain_name: Option<&str>) -> Result<String, String> {
    set_status(db, title, Some("resolved"), brain_name)
}

fn set_status(
    db: &mut GrugDb,
    title: Option<&str>,
    status: Option<&str>,
    brain_name: Option<&str>,
) -> Result<String, String> {
    let title = title.ok_or("missing field: title")?;
    let status = status.ok_or("missing field: status")?;

    let brain = resolve_conversation_brain(db, brain_name)?.clone();
    if !brain.writable {
        return Ok(format!("brain \"{}\" is read-only", brain.name));
    }

    let slug = slugify(title);
    let file_path = brain.dir.join(CATEGORY).join(format!("{slug}.md"));
    let rel_path = format!("{CATEGORY}/{slug}.md");

    if !file_path.exists() {
        return Err(format!("conversation not found: {slug}"));
    }

    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("failed to read {}: {e}", file_path.display()))?;

    let updated = if let Some(start) = content.find("status: ") {
        let line_end = content[start..].find('\n').unwrap_or(content[start..].len());
        format!("{}status: {}{}", &content[..start], status, &content[start + line_end..])
    } else {
        content
    };

    atomic_write(&file_path, updated.as_bytes())
        .map_err(|e| format!("failed to write {}: {e}", file_path.display()))?;

    index_file(db.conn(), &brain.name, &rel_path, &file_path, CATEGORY)?;
    db.enqueue_git_commit(&brain.name, &rel_path, "write");

    Ok(format!("conversation {slug} status set to: {status}"))
}

fn extract_frontmatter_value(content: &str, key: &str) -> Option<String> {
    let fm = content.strip_prefix("---\n")?;
    let end = fm.find("\n---")?;
    let prefix = format!("{key}: ");
    for line in fm[..end].lines() {
        if let Some(val) = line.strip_prefix(&prefix) {
            return Some(val.trim_matches('"').to_string());
        }
    }
    None
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::tools::test_helpers::test_db_with_git;

    #[test]
    fn test_open_creates_conversation() {
        let (mut db, _tmp, _rx) = test_db_with_git();
        let result = grug_conversation(
            &mut db, "open",
            Some("test thread"), Some("hello world"),
            Some("mac-studio"), None, None,
        );
        assert!(result.is_ok());
        assert!(result.unwrap().contains("opened conversation: test-thread"));
    }

    #[test]
    fn test_open_duplicate_errors() {
        let (mut db, _tmp, _rx) = test_db_with_git();
        grug_conversation(&mut db, "open", Some("dupe"), Some("msg"), Some("a"), None, None).unwrap();
        let result = grug_conversation(&mut db, "open", Some("dupe"), Some("msg2"), Some("b"), None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn test_reply_appends_message() {
        let (mut db, tmp, _rx) = test_db_with_git();
        grug_conversation(&mut db, "open", Some("chat"), Some("first"), Some("alice"), None, None).unwrap();
        let result = grug_conversation(
            &mut db, "reply", Some("chat"), Some("second"), Some("bob"), None, None,
        );
        assert!(result.is_ok());
        assert!(result.unwrap().contains("message 2"));

        let path = tmp.path().join("memories/conversations/chat.md");
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("### Message 1 — alice"));
        assert!(content.contains("### Message 2 — bob"));
        assert!(content.contains("participants: [alice, bob]"));
    }

    #[test]
    fn test_reply_not_found() {
        let (mut db, _tmp, _rx) = test_db_with_git();
        let result = grug_conversation(&mut db, "reply", Some("nope"), Some("msg"), Some("a"), None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_list_conversations() {
        let (mut db, _tmp, _rx) = test_db_with_git();
        grug_conversation(&mut db, "open", Some("alpha"), Some("msg"), Some("a"), None, None).unwrap();
        grug_conversation(&mut db, "open", Some("beta"), Some("msg"), Some("b"), None, None).unwrap();
        let result = grug_conversation(&mut db, "list", None, None, None, None, None).unwrap();
        assert!(result.contains("alpha"));
        assert!(result.contains("beta"));
        assert!(result.contains("[open]"));
    }

    #[test]
    fn test_list_empty() {
        let (mut db, _tmp, _rx) = test_db_with_git();
        let result = grug_conversation(&mut db, "list", None, None, None, None, None).unwrap();
        assert!(result.contains("no conversations"));
    }

    #[test]
    fn test_close_sets_resolved() {
        let (mut db, tmp, _rx) = test_db_with_git();
        grug_conversation(&mut db, "open", Some("issue"), Some("bug"), Some("a"), None, None).unwrap();
        grug_conversation(&mut db, "close", Some("issue"), None, None, None, None).unwrap();
        let path = tmp.path().join("memories/conversations/issue.md");
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("status: resolved"));
    }

    #[test]
    fn test_status_custom() {
        let (mut db, tmp, _rx) = test_db_with_git();
        grug_conversation(&mut db, "open", Some("thread"), Some("msg"), Some("a"), None, None).unwrap();
        grug_conversation(
            &mut db, "status", Some("thread"), None, None, Some("awaiting-verification"), None,
        ).unwrap();
        let path = tmp.path().join("memories/conversations/thread.md");
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("status: awaiting-verification"));
    }

    #[test]
    fn test_open_missing_title() {
        let (mut db, _tmp, _rx) = test_db_with_git();
        let result = grug_conversation(&mut db, "open", None, Some("msg"), None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_open_missing_message() {
        let (mut db, _tmp, _rx) = test_db_with_git();
        let result = grug_conversation(&mut db, "open", Some("t"), None, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_add_participant_new() {
        let content = "---\nparticipants: [alice]\n---\n";
        let result = add_participant(content, "bob");
        assert!(result.contains("participants: [alice, bob]"));
    }

    #[test]
    fn test_add_participant_already_present() {
        let content = "---\nparticipants: [alice, bob]\n---\n";
        let result = add_participant(content, "alice");
        assert_eq!(result, content);
    }

    #[test]
    fn test_emits_git_commit() {
        let (mut db, _tmp, mut rx) = test_db_with_git();
        grug_conversation(&mut db, "open", Some("git-test"), Some("msg"), Some("a"), None, None).unwrap();
        let req = rx.try_recv().unwrap();
        assert_eq!(req.rel_path, "conversations/git-test.md");
        assert_eq!(req.action, "write");
    }
}
