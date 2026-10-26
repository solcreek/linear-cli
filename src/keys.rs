//! Where the API key comes from: LINEAR_API_KEY, else the key `auth login`
//! stored, in the macOS Keychain or in credentials.json. The key never goes
//! in argv: `security -i` reads its command from stdin.

use crate::fail::{R, failed};
use crate::state::{cause, home, inside, replace_file, with_lock};
use crate::util::is_key;
use serde_json::Value;
use std::cell::RefCell;
use std::io::Write;
use std::process::{Command, Stdio};

thread_local! {
    /// The key serve read once at start, so a request does not read the
    /// Keychain again (~10 ms each).
    pub static SERVED: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

/// LINEAR_API_KEY, else the stored key, with where it came from. An empty
/// key with no error means none is set anywhere.
pub fn api_key() -> R<(String, String)> {
    if let Some(k) = SERVED.with(|s| s.borrow().clone()) {
        return Ok(k);
    }
    if let Ok(k) = std::env::var("LINEAR_API_KEY")
        && !k.is_empty()
    {
        return Ok((k.trim().to_string(), "env".into()));
    }
    store().read()
}

const SERVICE: &str = "linearctl";
const ACCOUNT: &str = "api-key";

/// Where a key is stored.
pub enum Store {
    /// The macOS Keychain, through this security(1).
    Keychain(String),
    /// credentials.json in this directory; "" when there is no home.
    File(String),
}

/// The macOS Keychain, unless LINEARCTL_HOME isolates this run (tests,
/// experiments) or there is no `security`.
pub fn store() -> Store {
    let bin = "/usr/bin/security";
    let isolated = std::env::var("LINEARCTL_HOME").is_ok_and(|h| !h.is_empty());
    if cfg!(target_os = "macos") && !isolated && std::path::Path::new(bin).exists() {
        return Store::Keychain(bin.into());
    }
    Store::File(home("config"))
}

fn credentials_file(dir: &str) -> R<(String, String)> {
    if dir.is_empty() {
        return Err(failed("no home directory; set LINEARCTL_HOME or XDG_CONFIG_HOME"));
    }
    Ok((dir.to_string(), "credentials.json".into()))
}

impl Store {
    pub fn name(&self) -> &'static str {
        match self {
            Store::Keychain(_) => "keychain",
            Store::File(_) => "file",
        }
    }

    /// The stored key, "" when there is none, and the store's name.
    pub fn read(&self) -> R<(String, String)> {
        let source = self.name().to_string();
        match self {
            Store::Keychain(bin) => {
                let out = Command::new(bin)
                    .args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"])
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .map_err(|e| failed(format!("cannot read the Keychain: {}", cause(&e))))?;
                match out.status.code() {
                    Some(0) => Ok((String::from_utf8_lossy(&out.stdout).trim().to_string(), source)),
                    Some(44) => Ok((String::new(), source)), // errSecItemNotFound
                    code => Err(failed(format!("cannot read the Keychain: {}", exit_status(code)))),
                }
            }
            Store::File(dir) => {
                let (dir, name) = credentials_file(dir)?;
                let path = format!("{dir}/{name}");
                let data = match std::fs::read(&path) {
                    Ok(d) => d,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((String::new(), source)),
                    Err(e) => return Err(failed(format!("cannot read {path}: {}", cause(&e)))),
                };
                let invalid = || failed(format!("{path} is not valid JSON; remove it and log in again"));
                let v = crate::json::parse(&data).map_err(|_| invalid())?;
                let key = match &v {
                    Value::Object(m) => {
                        match m.iter().find(|(k, _)| k.eq_ignore_ascii_case("apiKey")).map(|(_, v)| v) {
                            None | Some(Value::Null) => String::new(),
                            Some(Value::String(s)) => s.clone(),
                            Some(_) => return Err(invalid()),
                        }
                    }
                    Value::Null => String::new(),
                    _ => return Err(invalid()),
                };
                Ok((key, source))
            }
        }
    }

    /// Saves key where read finds it.
    pub fn write(&self, key: &str) -> R {
        if !is_key(key) {
            return Err(failed("refusing to store a key with unexpected characters"));
        }
        match self {
            Store::Keychain(bin) => {
                let mut child = Command::new(bin)
                    .arg("-i")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| failed(format!("cannot write the Keychain: {}", cause(&e))))?;
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = writeln!(stdin, "add-generic-password -U -s {SERVICE} -a {ACCOUNT} -w {key}");
                }
                let out = child
                    .wait_with_output()
                    .map_err(|e| failed(format!("cannot write the Keychain: {}", cause(&e))))?;
                if !out.status.success() {
                    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                    text.push_str(&String::from_utf8_lossy(&out.stderr));
                    return Err(failed(format!("cannot write the Keychain: {}", text.replace(key, "…").trim())));
                }
                // `security -i` exits 0 whatever happened; read it back to know.
                match self.read() {
                    Ok((got, _)) if got == key => Ok(()),
                    _ => Err(failed("cannot write the Keychain: the key did not read back")),
                }
            }
            Store::File(dir) => {
                let (dir, name) = credentials_file(dir)?;
                make_dir(&dir).map_err(|e| failed(format!("cannot create {dir}: {}", cause(&e))))?;
                let data = format!("{{\n  \"apiKey\": {}\n}}\n", crate::json::marshal(&Value::String(key.into())));
                with_lock(&dir, &name, || replace_file(&dir, &name, data.as_bytes()))
            }
        }
    }

    /// Removes the stored key; false when there was none.
    pub fn forget(&self) -> R<bool> {
        match self {
            Store::Keychain(bin) => {
                let out = Command::new(bin)
                    .args(["delete-generic-password", "-s", SERVICE, "-a", ACCOUNT])
                    .stdin(Stdio::null())
                    .output()
                    .map_err(|e| failed(format!("cannot write the Keychain: {}", cause(&e))))?;
                match out.status.code() {
                    Some(0) => Ok(true),
                    Some(44) => Ok(false),
                    _ => {
                        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                        text.push_str(&String::from_utf8_lossy(&out.stderr));
                        Err(failed(format!("cannot write the Keychain: {}", text.trim())))
                    }
                }
            }
            Store::File(dir) => {
                let (dir, name) = credentials_file(dir)?;
                let path = match inside(&dir, &name) {
                    Ok(p) => p,
                    Err(_) if !std::path::Path::new(&dir).exists() => return Ok(false),
                    Err(e) => return Err(e),
                };
                let mut removed = false;
                with_lock(&dir, &name, || match std::fs::remove_file(&path) {
                    Ok(()) => {
                        removed = true;
                        Ok(())
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(failed(format!("cannot remove {path}: {}", cause(&e)))),
                })?;
                Ok(removed)
            }
        }
    }
}

fn exit_status(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("exit status {c}"),
        None => "signal: killed".into(),
    }
}

/// A directory and its parents, 0700 where it makes them.
pub fn make_dir(dir: &str) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for macOS's security(1): it keeps one password in a file
    /// and logs every argv, so a test can check the key never was one.
    const FAKE_SECURITY: &str = r#"#!/bin/sh
echo "$*" >> "$0.argv"
case "$1" in
-i)
	read -r cmd
	set -- $cmd
	while [ $# -gt 0 ]; do [ "$1" = "-w" ] && { printf %s "$2" > "$(dirname "$0")/keychain"; exit 0; }; shift; done
	exit 0 ;;
find-generic-password)
	[ -f "$(dirname "$0")/keychain" ] || exit 44
	cat "$(dirname "$0")/keychain"; echo ;;
delete-generic-password)
	[ -f "$(dirname "$0")/keychain" ] || exit 44
	rm "$(dirname "$0")/keychain" ;;
esac
"#;

    fn fake(body: &str) -> (std::path::PathBuf, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lc-keychain-{}-{}", std::process::id(), crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("security");
        std::fs::write(&tool, body).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, tool.to_string_lossy().into_owned())
    }

    const KEY: &str = "lin_api_test0000000000000000";

    #[test]
    fn keychain_store_read_forget() {
        let (dir, tool) = fake(FAKE_SECURITY);
        let k = Store::Keychain(tool.clone());
        assert_eq!(k.read().unwrap(), (String::new(), "keychain".into()));
        k.write(KEY).unwrap();
        assert_eq!(k.read().unwrap().0, KEY);
        assert!(k.forget().unwrap());
        assert!(!k.forget().unwrap());
        let argv = std::fs::read_to_string(format!("{tool}.argv")).unwrap();
        assert!(!argv.contains(KEY) && argv.contains("find-generic-password -s linearctl -a api-key -w"), "{argv}");
        assert!(k.write("has space in it 00000000").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_store_read_write_forget() {
        let dir = std::env::temp_dir().join(format!("lc-creds-{}-{}", std::process::id(), crate::util::new_id()));
        let d = dir.join("config").to_string_lossy().into_owned();
        let f = Store::File(d.clone());
        assert_eq!(f.read().unwrap(), (String::new(), "file".into()));
        assert!(!f.forget().unwrap());
        f.write(KEY).unwrap();
        assert_eq!(f.read().unwrap().0, KEY);
        assert_eq!(
            std::fs::read_to_string(format!("{d}/credentials.json")).unwrap(),
            format!("{{\n  \"apiKey\": \"{KEY}\"\n}}\n")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &str| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!((mode(&d), mode(&format!("{d}/credentials.json"))), (0o700, 0o600));
        }
        assert!(f.forget().unwrap());
        assert!(!f.forget().unwrap());
        assert!(f.write("not a key").is_err());
        std::fs::write(format!("{d}/credentials.json"), "{").unwrap();
        assert!(
            f.read()
                .unwrap_err()
                .to_string()
                .ends_with("credentials.json is not valid JSON; remove it and log in again")
        );
        std::fs::write(format!("{d}/credentials.json"), "null").unwrap();
        assert_eq!(f.read().unwrap().0, "");
        std::fs::remove_dir_all(dir).unwrap();
        let nowhere = Store::File(String::new());
        assert!(nowhere.read().unwrap_err().to_string().starts_with("no home directory"));
    }

    #[test]
    fn keychain_failures_are_named() {
        let (dir, tool) = fake("#!/bin/sh\necho 'User interaction is not allowed.' >&2\nexit 51\n");
        let k = Store::Keychain(tool);
        assert!(k.read().unwrap_err().to_string().contains("cannot read the Keychain"));
        assert!(k.write(KEY).unwrap_err().to_string().contains("cannot write the Keychain"));
        assert!(k.forget().unwrap_err().to_string().contains("not allowed"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
