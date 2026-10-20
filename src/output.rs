//! File output with never-overwrite-by-default semantics.
//!
//! Default behavior: refuse to overwrite an existing file. `--force` opts in.
//! Creates parent directories as needed so `-o new/dir/notes.md` works.

use std::io::Write;
use std::path::Path;

pub fn write_markdown(path: &Path, content: &str, force: bool) -> std::io::Result<()> {
    if path.exists() && !force {
        // Never overwrite without explicit permission. Surface the collision
        // so the caller can decide (rename, --force, or pick another path).
        return Err(std::io::Error::other(format!(
            "output file already exists: {} (pass --force to overwrite)",
            path.display()
        )));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfile(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tingbudont_test_{name}.md"));
        p
    }

    #[test]
    fn refuses_to_overwrite_without_force() {
        let p = tmpfile("no_overwrite");
        let _ = std::fs::remove_file(&p);
        std::fs::write(&p, "existing").unwrap();
        let err = write_markdown(&p, "new", false).err().unwrap();
        assert!(err.to_string().contains("already exists"));
        // original preserved
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "existing");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn overwrites_with_force() {
        let p = tmpfile("with_force");
        let _ = std::fs::remove_file(&p);
        std::fs::write(&p, "existing").unwrap();
        write_markdown(&p, "new", true).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn creates_parent_dirs() {
        let mut p = std::env::temp_dir();
        p.push("tingbudont_test_subdir");
        p.push("nested.md");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
        write_markdown(&p, "data", false).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "data");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
