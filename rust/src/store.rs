use crate::service::{Definition, timestamp};
use anyhow::{Context, Result, anyhow, bail};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Link {
    pub id: String,
    pub url: String,
    pub port: u16,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AuthRecord {
    api_key: String,
    created_at: String,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn default_root() -> Result<PathBuf> {
        if let Ok(root) = env::var("BURNER_HOME") {
            if !root.is_empty() {
                return Ok(if Path::new(&root).is_absolute() {
                    PathBuf::from(root)
                } else {
                    env::current_dir()?.join(root)
                });
            }
        }

        if cfg!(target_os = "linux") {
            return Ok(PathBuf::from("/var/lib/burner"));
        }

        if let Ok(home) = env::var("HOME") {
            if !home.is_empty() {
                return Ok(PathBuf::from(home).join(".burner"));
            }
        }

        bail!("could not determine Burner state directory: set BURNER_HOME or HOME");
    }

    pub fn init(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.services_dir(),
            self.logs_dir(),
            self.links_dir(),
        ] {
            fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn save(&self, def: &mut Definition) -> Result<()> {
        self.init()?;
        def.updated_at = timestamp();
        if def.created_at.is_empty() {
            def.created_at = def.updated_at.clone();
        }

        let data = serde_json::to_vec_pretty(def).context("marshal service definition")?;
        fs::write(self.service_path(&def.name), [data, vec![b'\n']].concat())
            .context("write service definition")?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Definition> {
        let data = fs::read(self.service_path(name)).context("read service definition")?;
        serde_json::from_slice(&data).context("decode service definition")
    }

    pub fn list(&self) -> Result<Vec<Definition>> {
        let entries = match fs::read_dir(self.services_dir()) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err).context("read services directory"),
        };

        let mut services = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() || path.extension().and_then(|ext| ext.to_str()) != Some("json")
            {
                continue;
            }

            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| anyhow!("invalid service file name"))?;
            services.push(self.get(name)?);
        }

        services.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(services)
    }

    pub fn save_link(&self, link: &Link) -> Result<()> {
        self.init()?;
        let data = serde_json::to_vec_pretty(link).context("marshal link")?;
        fs::write(self.link_path(&link.id), [data, vec![b'\n']].concat()).context("write link")?;
        Ok(())
    }

    pub fn get_link(&self, id: &str) -> Result<Link> {
        let data = fs::read(self.link_path(id)).context("read link")?;
        serde_json::from_slice(&data).context("decode link")
    }

    pub fn log_path(&self, name: &str) -> PathBuf {
        self.logs_dir().join(format!("{name}.log"))
    }

    pub fn delete_service(&self, name: &str) -> Result<()> {
        let path = self.service_path(name);
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("delete service definition {}", path.display()))?;
        }
        Ok(())
    }

    pub fn delete_log(&self, def: &Definition) -> Result<()> {
        let log_path = if def.log_path.is_empty() {
            self.log_path(&def.name)
        } else {
            PathBuf::from(&def.log_path)
        };
        if log_path.exists() {
            fs::remove_file(&log_path).with_context(|| format!("delete service log {}", log_path.display()))?;
        }
        Ok(())
    }

    pub fn backup_service(&self, def: &Definition) -> Result<PathBuf> {
        self.init()?;
        fs::create_dir_all(self.backups_dir()).context("create backups directory")?;

        let backup_dir = self
            .backups_dir()
            .join(format!("{}-{}", def.name, timestamp().replace([':', '+'], "")));
        fs::create_dir_all(&backup_dir)
            .with_context(|| format!("create backup directory {}", backup_dir.display()))?;

        let data = serde_json::to_vec_pretty(def).context("marshal service backup")?;
        fs::write(backup_dir.join("service.json"), [data, vec![b'\n']].concat())
            .context("write service backup")?;

        let stored_definition = self.service_path(&def.name);
        if stored_definition.exists() {
            let _ = fs::copy(&stored_definition, backup_dir.join("service.stored.json"));
        }

        let log_path = if def.log_path.is_empty() {
            self.log_path(&def.name)
        } else {
            PathBuf::from(&def.log_path)
        };
        if log_path.exists() {
            let _ = fs::copy(&log_path, backup_dir.join("service.log"));
        }

        if !def.unit_name.is_empty() {
            let unit_path = PathBuf::from("/etc/systemd/system").join(&def.unit_name);
            if unit_path.exists() {
                let _ = fs::copy(&unit_path, backup_dir.join(&def.unit_name));
            }
        }

        Ok(backup_dir)
    }

    pub fn services_dir(&self) -> PathBuf {
        self.root.join("services")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn links_dir(&self) -> PathBuf {
        self.root.join("links")
    }

    pub fn deployments_dir(&self) -> PathBuf {
        self.root.join("deployments")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    pub fn ensure_api_key(&self) -> Result<String> {
        self.init()?;
        if let Ok(record) = self.read_auth() {
            if !record.api_key.is_empty() {
                return Ok(record.api_key);
            }
        }

        let mut bytes = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut bytes);
        let record = AuthRecord {
            api_key: hex::encode(bytes),
            created_at: timestamp(),
        };

        let data = serde_json::to_vec_pretty(&record).context("marshal burner auth")?;
        fs::write(self.auth_path(), [data, vec![b'\n']].concat()).context("write burner auth")?;
        Ok(record.api_key)
    }

    pub fn api_key(&self) -> Result<String> {
        let record = self.read_auth().context("read burner auth")?;
        if record.api_key.is_empty() {
            bail!("no API key configured: run burner online -p <port> first");
        }
        Ok(record.api_key)
    }

    fn service_path(&self, name: &str) -> PathBuf {
        self.services_dir().join(format!("{name}.json"))
    }

    fn link_path(&self, id: &str) -> PathBuf {
        self.links_dir().join(format!("{id}.json"))
    }

    fn auth_path(&self) -> PathBuf {
        self.root.join("auth.json")
    }

    fn read_auth(&self) -> Result<AuthRecord> {
        let data = fs::read(self.auth_path()).context("read burner auth")?;
        serde_json::from_slice(&data).context("decode burner auth")
    }
}

#[cfg(test)]
mod tests {
    use super::Store;
    use crate::service::Definition;
    use std::path::PathBuf;

    #[test]
    fn save_get_and_list() {
        let root = tempfile::tempdir().unwrap();
        let st = Store::new(root.path().to_path_buf());

        let mut def = Definition {
            name: "api".into(),
            command: "bun run start".into(),
            location: root.path().join("app").to_string_lossy().into_owned(),
            runtime: "local".into(),
            status: "running".into(),
            pid: 1234,
            ..Definition::default()
        };

        st.save(&mut def).unwrap();
        let got = st.get("api").unwrap();
        assert_eq!(got.name, "api");
        assert_eq!(got.command, "bun run start");
        assert_eq!(got.runtime, "local");

        let list = st.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "api");
    }

    #[test]
    fn default_root_prefers_burner_home() {
        unsafe {
            std::env::set_var("BURNER_HOME", "/tmp/burner-home");
            std::env::set_var("HOME", "/tmp/ignored-home");
        }
        let root = Store::default_root().unwrap();
        assert_eq!(root, PathBuf::from("/tmp/burner-home"));
    }

    #[test]
    fn ensure_api_key_persists() {
        let root = tempfile::tempdir().unwrap();
        let st = Store::new(root.path().to_path_buf());

        let first = st.ensure_api_key().unwrap();
        let second = st.ensure_api_key().unwrap();

        assert_eq!(first, second);
        assert!(!first.is_empty());
    }

    #[test]
    fn backup_service_writes_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let st = Store::new(root.path().to_path_buf());

        let mut def = Definition {
            name: "api".into(),
            command: "bun run start".into(),
            location: root.path().join("app").to_string_lossy().into_owned(),
            runtime: "local".into(),
            status: "running".into(),
            pid: 1234,
            ..Definition::default()
        };
        st.save(&mut def).unwrap();
        fs::write(st.log_path("api"), "hello\n").unwrap();

        let backup_dir = st.backup_service(&def).unwrap();
        assert!(backup_dir.join("service.json").exists());
        assert!(backup_dir.join("service.log").exists());
    }
}
