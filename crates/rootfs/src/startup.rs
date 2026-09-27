//! One boot domain; init and applications are supplied by the distribution.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Startup {
    pub command: Vec<String>,
    pub script: Option<Vec<String>>,
    pub shutdown: Vec<String>,
    pub session_config: String,
    pub environment: BTreeMap<String, String>,
    pub terminals: Vec<Terminal>,
    pub default_terminal: String,
    pub tray: bool,
    pub web: String,
    pub shutdown_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default = "home")]
    pub cwd: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default = "yes")]
    pub autostart: bool,
}
fn home() -> String {
    "/".into()
}
fn yes() -> bool {
    true
}
impl Default for Startup {
    fn default() -> Self {
        Self {
            command: vec![],
            script: None,
            shutdown: vec![],
            session_config: "/etc/kinakaze/session.json".into(),
            environment: BTreeMap::new(),
            terminals: vec![],
            default_terminal: String::new(),
            tray: true,
            web: "127.0.0.1:0".into(),
            shutdown_timeout_seconds: 15,
        }
    }
}
impl Terminal {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || self.name.len() > 48
            || !self
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err(
                "terminal name must contain 1..48 ASCII letters, digits, '_' or '-'".into(),
            );
        }
        if self.command.is_empty()
            || self.command.len() > 128
            || !self.command[0].starts_with('/')
            || !self.cwd.starts_with('/')
            || self
                .command
                .iter()
                .any(|s| s.contains('\0') || s.len() > 32768)
            || self.cwd.contains('\0')
            || self.environment.len() > 128
            || self
                .environment
                .iter()
                .any(|(k, v)| k.is_empty() || k.contains(['=', '\0']) || v.contains('\0'))
        {
            return Err(
                "terminal requires an absolute guest executable/cwd and valid argv/environment"
                    .into(),
            );
        }
        Ok(())
    }
}
impl Startup {
    pub fn validate(&self) -> Result<(), String> {
        // An absent startup block is valid for legacy/offline rootfs manifests.
        if self.command.is_empty() && self.terminals.is_empty() {
            return Ok(());
        }
        if self.environment.len() > 128
            || self.environment.iter().any(|(key, value)| {
                key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')
            })
        {
            return Err("invalid startup environment".into());
        }
        if self.command.is_empty()
            || !self.command[0].starts_with('/')
            || self.command.iter().any(|s| s.contains('\0'))
        {
            return Err("startup.command requires an absolute PID 1 executable".into());
        }
        if !self.session_config.starts_with('/')
            || self
                .session_config
                .trim_start_matches('/')
                .split('/')
                .any(|p| p.is_empty() || matches!(p, "." | "..") || p.contains(['\\', ':', '\0']))
        {
            return Err("invalid startup.session_config path".into());
        }
        if self.terminals.is_empty()
            || self.terminals.len() > 32
            || !(1..=120).contains(&self.shutdown_timeout_seconds)
        {
            return Err(
                "startup needs 1..32 terminals and a 1..120 second shutdown timeout".into(),
            );
        }
        let mut names = std::collections::BTreeSet::new();
        for terminal in &self.terminals {
            terminal.validate()?;
            if !names.insert(&terminal.name) {
                return Err("duplicate startup terminal name".into());
            }
        }
        if !names.contains(&self.default_terminal) {
            return Err("default_terminal is not configured".into());
        }
        for command in self
            .script
            .iter()
            .chain((!self.shutdown.is_empty()).then_some(&self.shutdown))
        {
            if command.is_empty()
                || !command[0].starts_with('/')
                || command.iter().any(|s| s.contains('\0'))
            {
                return Err("startup script/shutdown requires an absolute guest executable".into());
            }
        }
        let address: std::net::SocketAddr = self.web.parse().map_err(|_| "invalid startup.web")?;
        if address.ip() != std::net::Ipv4Addr::LOCALHOST {
            return Err("startup.web must bind 127.0.0.1".into());
        }
        Ok(())
    }
}
pub fn load(dist: &Path, explicit: Option<&Path>) -> Result<Startup, Box<dyn std::error::Error>> {
    let path = explicit
        .map(Path::to_owned)
        .unwrap_or_else(|| dist.join("rootfs.manifest.json"));
    let mut bytes = Vec::new();
    File::open(path)?
        .take(super::MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > super::MAX_MANIFEST {
        return Err("manifest too large".into());
    }
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        startup: Startup,
    }
    let config: Config = serde_json::from_slice(&bytes)?;
    config.startup.validate()?;
    Ok(config.startup)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configured() -> Startup {
        serde_json::from_value(serde_json::json!({
            "command":["/custom/init","--boot"],
            "script":["/custom/interpreter","/boot/start"],
            "shutdown":["/custom/poweroff"],
            "session_config":"/var/run/custom-session.json",
            "terminals":[{"name":"app","command":["/opt/my-elf"],"cwd":"/"}],
            "default_terminal":"app"
        }))
        .unwrap()
    }
    #[test]
    fn init_shell_and_paths_are_distribution_configuration() {
        let startup = configured();
        startup.validate().unwrap();
        assert_eq!(startup.command[0], "/custom/init");
        assert_eq!(startup.terminals[0].command[0], "/opt/my-elf");
        assert_eq!(startup.script.unwrap()[0], "/custom/interpreter");
    }
    #[test]
    fn rejects_ambiguous_terminal_names_and_escaping_control_path() {
        let mut startup = configured();
        startup.terminals.push(startup.terminals[0].clone());
        assert!(startup.validate().is_err());
        let mut startup = configured();
        startup.session_config = "/../outside".into();
        assert!(startup.validate().is_err());
        let mut startup = configured();
        startup.default_terminal = "absent".into();
        assert!(startup.validate().is_err());
    }
}
