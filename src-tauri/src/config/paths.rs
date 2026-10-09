//! ConfigPaths::resolve honors XDG on both platforms; no tool-global config is accessed.
use std::{env, ffi::OsString, path::PathBuf};
#[derive(Clone, Debug)]
pub struct ConfigPaths {
    pub dir: PathBuf,
    pub ultra: PathBuf,
    pub terminal: PathBuf,
    pub keys: PathBuf,
    pub machine_file: PathBuf,
    pub machine: String,
    pub state_file: PathBuf,
}
impl ConfigPaths {
    pub fn resolve() -> Result<Self, String> {
        Self::resolve_with(|name| env::var_os(name), hostname)
    }
    /// `var` reads one environment variable; `hostname` names the machine when
    /// `ULTRA_9000_MACHINE` is unset, empty or not Unicode.
    fn resolve_with(
        var: impl Fn(&str) -> Option<OsString>,
        hostname: impl FnOnce() -> String,
    ) -> Result<Self, String> {
        let home = var("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .ok_or("HOME is unavailable")?;
        let config = var("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let state = var("XDG_STATE_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"));
        if !config.is_absolute() || !state.is_absolute() {
            return Err("XDG paths must be absolute".into());
        }
        let machine = var("ULTRA_9000_MACHINE")
            .and_then(|v| v.into_string().ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(hostname);
        if machine == "." || machine == ".." || machine.contains(['/', '\\', '\0']) {
            return Err("machine name must be one filename component".into());
        }
        let dir = config.join("ultra-9000");
        Ok(Self {
            ultra: dir.join("ultra.toml"),
            terminal: dir.join("terminal.toml"),
            keys: dir.join("keys.toml"),
            machine_file: dir.join("machines").join(format!("{machine}.toml")),
            machine,
            dir,
            state_file: state.join("ultra-9000/state.json"),
        })
    }
}
/// The host name up to its first dot, or "machine" when it cannot be read.
fn hostname() -> String {
    let mut bytes = [0u8; 256];
    // gethostname writes at most this buffer, which is initialized and bounded.
    let ok = unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } == 0;
    if ok {
        String::from_utf8_lossy(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
            .split('.')
            .next()
            .unwrap_or("machine")
            .to_owned()
    } else {
        "machine".into()
    }
}
#[cfg(test)]
mod tests;
