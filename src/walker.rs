use std::fs;
use std::path::{Path, PathBuf};

/// Recursively walk a directory for .md and .mdx files.
/// Skips entries whose names start with '.' or '_'.
/// Returns sorted absolute paths.
/// Matches JS walkFiles behavior.
pub fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }
    walk_files_inner(dir, &mut files);
    files.sort();
    files
}

fn walk_files_inner(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };

        if name_str.starts_with('.') || name_str.starts_with('_') {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            walk_files_inner(&path, files);
        } else if name_str.ends_with(".md") || name_str.ends_with(".mdx") {
            files.push(path);
        }
    }
}

/// List category subdirectories in a brain directory.
/// Skips dot-prefixed entries. Returns sorted names.
pub fn get_categories(dir: &Path) -> Vec<String> {
    if !dir.exists() {
        fs::create_dir_all(dir).ok();
    }
    let mut categories = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return categories,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        if name_str.starts_with('.') || name_str.starts_with('_') {
            continue;
        }
        if entry.path().is_dir() {
            categories.push(name_str);
        }
    }
    categories.sort();
    categories
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_file(base: &Path, rel_path: &str, content: &str) {
        let full = base.join(rel_path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
    }

    #[test]
    fn test_walk_files_basic() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        create_file(dir, "category1/file1.md", "# test");
        create_file(dir, "category1/file2.mdx", "# test");
        create_file(dir, "category2/file3.md", "# test");

        let files = walk_files(dir);
        assert_eq!(files.len(), 3);
        // Verify sorted
        for i in 1..files.len() {
            assert!(files[i - 1] <= files[i]);
        }
    }

    #[test]
    fn test_walk_files_skips_dotfiles() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        create_file(dir, ".hidden/file.md", "# hidden");
        create_file(dir, "normal/file.md", "# normal");

        let files = walk_files(dir);
        assert_eq!(files.len(), 1);
        assert!(files[0].to_str().unwrap().contains("normal"));
    }

    #[test]
    fn test_walk_files_skips_underscored() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        create_file(dir, "_drafts/file.md", "# draft");
        create_file(dir, "notes/file.md", "# notes");

        let files = walk_files(dir);
        assert_eq!(files.len(), 1);
        assert!(files[0].to_str().unwrap().contains("notes"));
    }

    #[test]
    fn test_walk_files_skips_non_md() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        create_file(dir, "file.txt", "text");
        create_file(dir, "file.md", "# markdown");

        let files = walk_files(dir);
        assert_eq!(files.len(), 1);
        assert!(files[0].to_str().unwrap().ends_with(".md"));
    }

    #[test]
    fn test_walk_files_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let files = walk_files(tmp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn test_walk_files_nonexistent() {
        let files = walk_files(Path::new("/nonexistent/path/xyzzy"));
        assert!(files.is_empty());
    }

    #[test]
    fn test_walk_files_nested() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        create_file(dir, "a/b/deep.md", "# deep");
        create_file(dir, "a/shallow.md", "# shallow");

        let files = walk_files(dir);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_get_categories() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        fs::create_dir_all(dir.join("alpha")).unwrap();
        fs::create_dir_all(dir.join("beta")).unwrap();
        fs::create_dir_all(dir.join(".hidden")).unwrap();
        // Create a file (not a dir)
        fs::write(dir.join("file.txt"), "not a dir").unwrap();

        let cats = get_categories(dir);
        assert_eq!(cats, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_get_categories_empty() {
        let tmp = TempDir::new().unwrap();
        let cats = get_categories(tmp.path());
        assert!(cats.is_empty());
    }

    #[test]
    fn test_get_categories_skips_underscored() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        fs::create_dir_all(dir.join("visible")).unwrap();
        fs::create_dir_all(dir.join("_hidden")).unwrap();
        fs::create_dir_all(dir.join("_drafts")).unwrap();

        let cats = get_categories(dir);
        assert_eq!(cats, vec!["visible"]);
    }
}
