//! Files linearctl keeps: where they live, and writing them so that two
//! runs, a crash or a symlink can never leave one half-written or written
//! somewhere else.

use crate::fail::{Fail, R, failed};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Temp files this run made, removed if it is interrupted.
static OURS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub fn own(p: &Path) {
    if let Ok(mut v) = OURS.lock() {
        v.push(p.to_path_buf());
    }
}

pub fn disown(p: &Path) {
    if let Ok(mut v) = OURS.lock() {
        v.retain(|x| x != p);
    }
}

/// Removes every temp file this run made (on Ctrl-C).
pub fn remove_ours() {
    if let Ok(v) = OURS.lock() {
        for p in v.iter() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// An OS error's own words, as Go prints them: "permission denied".
pub fn cause(e: &std::io::Error) -> String {
    let s = e.to_string();
    let s = match s.find(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s,
    };
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_lowercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// Go's filepath.Clean, for /-separated paths.
pub fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = vec![];
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

fn is_abs(p: &str) -> bool {
    Path::new(p).is_absolute()
}

/// Go's filepath.Abs.
pub fn abs(p: &str) -> String {
    if is_abs(p) {
        return clean(p);
    }
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    clean(&format!("{cwd}/{p}"))
}

fn dir_of(p: &str) -> String {
    match p.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => clean(&p[..i]),
        None => ".".into(),
    }
}

fn base_of(p: &str) -> &str {
    let p = p.trim_end_matches('/');
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// Where config, cache or state live. LINEARCTL_HOME moves all three under
/// one directory; tests set it. Empty when there is no home to put them in
/// (HOME unset, as under cron): never the current directory.
pub fn home(kind: &str) -> String {
    home_with(kind, &|name| std::env::var(name).unwrap_or_default())
}

/// home, reading the environment through env.
fn home_with(kind: &str, env: &dyn Fn(&str) -> String) -> String {
    let dir = env("LINEARCTL_HOME");
    if !dir.is_empty() {
        return format!("{}/{kind}", abs(&dir));
    }
    let (var, sub) = match kind {
        "config" => ("XDG_CONFIG_HOME", ".config"),
        "cache" => ("XDG_CACHE_HOME", ".cache"),
        _ => ("XDG_STATE_HOME", ".local/state"),
    };
    let base = env(var);
    // XDG says to ignore a relative path.
    let base = if is_abs(&base) {
        base
    } else {
        let h = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
        if h.is_empty() || !is_abs(&h) {
            return String::new();
        }
        format!("{}/{sub}", h.trim_end_matches('/'))
    };
    clean(&format!("{base}/linearctl"))
}

/// The file an output path names, refused unless it is plainly inside
/// root: no `..` part, no trailing slash, not the root itself, a parent
/// that resolves inside root, and a last part that is not a symlink, so a
/// write can never follow one.
pub fn inside(root: &str, path: &str) -> R<String> {
    let refuse = |msg: String| failed(format!("{path}: {msg}"));
    // Windows takes both separators; the checks below use /.
    let slashed = |p: &str| if cfg!(windows) { p.replace('\\', "/") } else { p.to_string() };
    let (root_s, path_s) = (slashed(root), slashed(path));
    if path_s.split('/').any(|p| p == "..") {
        return Err(refuse("'..' is not allowed in an output path".into()));
    }
    if path_s.ends_with('/') {
        return Err(refuse("names a directory, not a file".into()));
    }
    let real_root = std::fs::canonicalize(root).map_err(|e| failed(format!("lstat {root}: {}", cause(&e))))?;
    let mut full = clean(&path_s);
    if !is_abs(&full) {
        full = clean(&format!("{root_s}/{full}"));
    }
    let dir = dir_of(&full);
    let parent = std::fs::canonicalize(&dir).map_err(|_| refuse(format!("no such directory: {dir}")))?;
    let target = parent.join(base_of(&full));
    let shown = real_root.to_string_lossy();
    if target == real_root {
        return Err(refuse(format!("is {shown} itself")));
    }
    if !target.starts_with(&real_root) {
        return Err(refuse(format!("outside {shown}")));
    }
    if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(refuse("is a symlink; refusing to write through it".into()));
    }
    Ok(target.to_string_lossy().into_owned())
}

/// Runs f holding the OS's lock on dir/name.lock: flock, or LockFileEx on
/// Windows. The kernel releases it when the process ends, so a crash leaves
/// nothing stale. After 2 s of another run holding it: busy, exit 4.
pub fn with_lock<T>(dir: &str, name: &str, f: impl FnOnce() -> R<T>) -> R<T> {
    let path = inside(dir, &format!("{name}.lock"))?;
    let file = open_lock(&path).map_err(|e| failed(format!("cannot lock {path}: {}", cause(&e))))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) => return Err(failed(format!("cannot lock {path}: {}", cause(&e)))),
        }
        if Instant::now() > deadline {
            return Err(Fail::Busy(format!("{dir}/{name} is locked by another run; retry")));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = f();
    let _ = file.unlock();
    out
}

fn open_lock(path: &str) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path)
}

/// Replaces dir/name with data: a uniquely named temp file beside it, given
/// the target's mode (a new file 0600), renamed over it. The rename never
/// follows a symlink; inside refused one.
pub fn replace_file(dir: &str, name: &str, data: &[u8]) -> R {
    let target = inside(dir, name)?;
    let cannot = |e: &std::io::Error| failed(format!("cannot write {target}: {}", cause(e)));
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&target).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600)
    };
    let (temp, mut f) = loop {
        let mut b = [0u8; 4];
        let _ = getrandom::fill(&mut b);
        let temp = PathBuf::from(format!("{dir}/.{name}.{}.tmp", u32::from_le_bytes(b)));
        let mut o = OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        match o.open(&temp) {
            Ok(f) => break (temp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(cannot(&e)),
        }
    };
    own(&temp);
    let mut result = f.write_all(data).and_then(|()| f.sync_all());
    drop(f);
    #[cfg(unix)]
    if result.is_ok() {
        use std::os::unix::fs::PermissionsExt;
        result = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(mode));
    }
    if result.is_ok() {
        result = std::fs::rename(&temp, &target);
    }
    disown(&temp);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(cannot(&e));
    }
    Ok(())
}

/// Writes a new file, never through a link or over one that appeared since
/// the check; a failed write removes it.
pub fn create_file(target: &str, data: &[u8]) -> R {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o644);
    let mut f = o.open(target).map_err(|e| failed(format!("cannot create {target}: {}", cause(&e))))?;
    let p = PathBuf::from(target);
    own(&p);
    let result = f.write_all(data);
    drop(f);
    disown(&p);
    if let Err(e) = result {
        let _ = std::fs::remove_file(target);
        return Err(failed(format!("cannot write {target}: {}", cause(&e))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_follows_xdg() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| vars.iter().find(|(k, _)| *k == name).map_or(String::new(), |(_, v)| v.to_string())
        };
        // XDG says to ignore a relative path.
        let e = env(&[("HOME", "/home/ada"), ("XDG_CONFIG_HOME", "/xdg/config"), ("XDG_CACHE_HOME", "relative/cache")]);
        assert_eq!(home_with("config", &e), "/xdg/config/linearctl");
        assert_eq!(home_with("cache", &e), "/home/ada/.cache/linearctl");
        assert_eq!(home_with("state", &e), "/home/ada/.local/state/linearctl");
        assert_eq!(home_with("config", &env(&[("LINEARCTL_HOME", "/tmp/lc"), ("HOME", "/h")])), "/tmp/lc/config");
        // No home: nowhere, never the current directory.
        assert_eq!(home_with("state", &env(&[])), "");
        assert_eq!(home_with("state", &env(&[("HOME", "relative")])), "");
    }

    #[test]
    fn os_errors_in_go_words() {
        let e = std::fs::read("/nonexistent/x").unwrap_err();
        assert_eq!(cause(&e), "no such file or directory");
    }

    #[test]
    fn write_failures_are_named() {
        let dir = std::env::temp_dir().join(format!("lc-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().into_owned();
        create_file(&format!("{d}/new"), b"x").unwrap();
        let e = create_file(&format!("{d}/new"), b"x").unwrap_err().to_string();
        assert!(e.starts_with("cannot create ") && e.ends_with("file exists"), "{e}");
        let e = replace_file(&format!("{d}/missing"), "x", b"x").unwrap_err().to_string();
        assert!(e.starts_with("lstat ") && e.ends_with("missing: no such file or directory"), "{e}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
            let e = replace_file(&d, "x.json", b"x").unwrap_err().to_string();
            assert!(e.starts_with("cannot write ") && e.ends_with("permission denied"), "{e}");
            let e = with_lock(&d, "y.json", || Ok(())).unwrap_err().to_string();
            assert!(e.starts_with("cannot lock "), "{e}");
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(abs("/a/./b"), "/a/b");
        assert!(abs("x").ends_with("/x"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cleans_as_go() {
        for (p, want) in
            [("a/../b", "b"), ("/a/./b/", "/a/b"), ("", "."), ("../a", "../a"), ("/..", "/"), ("a//b", "a/b")]
        {
            assert_eq!(clean(p), want, "{p}");
        }
    }

    #[test]
    fn inside_refuses_escapes() {
        let dir = std::env::temp_dir().join(format!("lc-inside-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let root = dir.to_string_lossy().into_owned();
        let real = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
        assert_eq!(inside(&root, "f.txt").unwrap(), format!("{real}/f.txt"));
        assert_eq!(inside(&root, "sub/f.txt").unwrap(), format!("{real}/sub/f.txt"));
        for (p, msg) in [
            ("../x", "'..' is not allowed"),
            ("sub/", "names a directory"),
            ("nope/f", "no such directory"),
            ("/etc/passwd", "outside"),
            (".", "itself"),
        ] {
            let e = inside(&root, p).unwrap_err().to_string();
            assert!(e.contains(msg), "{p}: {e}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.join("link")).unwrap();
            assert!(inside(&root, "link").unwrap_err().to_string().contains("symlink"));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replaces_and_locks() {
        let dir = std::env::temp_dir().join(format!("lc-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().into_owned();
        with_lock(&d, "x.json", || replace_file(&d, "x.json", b"one")).unwrap();
        replace_file(&d, "x.json", b"two").unwrap();
        assert_eq!(std::fs::read(dir.join("x.json")).unwrap(), b"two");
        // A lock held elsewhere makes the second taker busy.
        let held = open_lock(&format!("{d}/y.json.lock")).unwrap();
        held.lock().unwrap();
        let e = std::thread::spawn(move || with_lock(&d, "y.json", || Ok(()))).join().unwrap();
        assert!(matches!(e, Err(Fail::Busy(_))), "{e:?}");
        drop(held);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
