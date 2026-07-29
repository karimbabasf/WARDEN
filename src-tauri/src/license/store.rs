//! Where the accepted key lives: `~/.warden/license`, one line, plain text.
//!
//! Deliberately not a secret and deliberately not obfuscated. The file holds
//! signed data whose only power is to satisfy a signature check the user already
//! passed, so hiding it would buy nothing and would cost the user the ability to
//! back it up, copy it to a second Mac they bought seats for, or read it back
//! when support asks. It is chmod 0600 anyway, because it carries the buyer's
//! email and that is theirs.
//!
//! The path has NO environment override, on purpose. Every other path in
//! `util.rs` takes one for testing; this one must not, because a redirectable
//! gate input is exactly the shape of a backdoor even when it cannot forge a
//! signature. Tests reach the pure functions instead, which take the path as an
//! argument. The canary in `gate.rs` enforces this.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `~/.warden/license`. Matches `util::default_db_path`'s home for the same
/// reason: everything WARDEN owns lives in one directory the user can delete.
pub fn license_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".warden/license"))
}

/// Read the stored key. Absent, unreadable, and empty are all the same answer:
/// `None`, which the gate turns into the activation screen. Nothing here decides
/// whether the contents are VALID, only whether there are any.
pub fn read_at(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Write the key, atomically: a full write to a sibling temp file, then a rename.
/// A crash or a full disk mid-write leaves the previous key intact rather than a
/// truncated one that would read as corrupt and re-prompt a paying customer.
pub fn write_at(path: &Path, key: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, format!("{}\n", key.trim()))?;
    restrict(&tmp)?;
    // Rename is atomic within a filesystem, and the temp file is a sibling, so
    // there is no cross-device case to handle.
    fs::rename(&tmp, path)
}

/// 0600. The key is not a secret, but the buyer's email is in it.
#[cfg(unix)]
fn restrict(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Read the stored key from the real location.
pub fn read() -> Option<String> {
    read_at(&license_path()?)
}

/// Persist an accepted key to the real location.
pub fn write(key: &str) -> io::Result<()> {
    let path = license_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
    write_at(&path, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_file_reads_as_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_at(&dir.path().join("license")), None);
    }

    #[test]
    fn empty_and_whitespace_only_files_read_as_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        for body in ["", "\n", "   \n\t "] {
            let path = dir.path().join("license");
            fs::write(&path, body).expect("write");
            assert_eq!(read_at(&path), None, "body {body:?} should read as absent");
        }
    }

    #[test]
    fn a_directory_in_place_of_the_file_reads_as_none_rather_than_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("license");
        fs::create_dir(&path).expect("mkdir");
        assert_eq!(read_at(&path), None);
    }

    #[test]
    fn round_trips_and_strips_the_trailing_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/license");
        write_at(&path, "WRDN-abc.def").expect("write");
        assert_eq!(read_at(&path).as_deref(), Some("WRDN-abc.def"));
        // The parent directory is created on demand, so a first launch on a
        // machine with no ~/.warden yet still activates.
        assert!(path.parent().expect("parent").is_dir());
    }

    #[test]
    fn a_rewrite_replaces_rather_than_appends() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("license");
        write_at(&path, "WRDN-first.sig").expect("write");
        write_at(&path, "WRDN-second.sig").expect("rewrite");
        assert_eq!(read_at(&path).as_deref(), Some("WRDN-second.sig"));
    }

    #[test]
    fn the_temp_file_is_not_left_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("license");
        write_at(&path, "WRDN-abc.def").expect("write");
        assert!(!path.with_extension("tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_stored_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("license");
        write_at(&path, "WRDN-abc.def").expect("write");
        let mode = fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "license file should be 0600, was {:o}",
            mode & 0o777
        );
    }

    #[test]
    fn the_real_path_is_under_the_warden_home_directory() {
        let path = license_path().expect("home resolves in the test environment");
        assert!(
            path.ends_with(".warden/license"),
            "unexpected path {}",
            path.display()
        );
    }
}
