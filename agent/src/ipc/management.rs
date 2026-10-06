// SPDX-License-Identifier: AGPL-3.0-or-later
// Validated worker management, sharing the desktop's store and persona rules.

use super::{Agent, Profile, Proxy};
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ManagementError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

fn invalid(message: impl Into<String>) -> anyhow::Error {
    ManagementError {
        status: 400,
        code: "invalid_input",
        message: message.into(),
    }
    .into()
}

fn missing(kind: &str) -> anyhow::Error {
    ManagementError {
        status: 404,
        code: "not_found",
        message: format!("no such {kind}"),
    }
    .into()
}

fn profile_input(mut input: Value, id: &str) -> anyhow::Result<Profile> {
    let object = input
        .as_object_mut()
        .ok_or_else(|| invalid("expected a JSON object"))?;
    // Never accept a caller-supplied seed or inline proxy secrets through this API.
    if object.contains_key("fp_seed") || object.contains_key("proxy") {
        return Err(invalid(
            "use persona_id and proxy_id; fp_seed and inline proxy are not writable",
        ));
    }
    object.insert("id".into(), json!(id));
    object.insert("fp_seed".into(), json!(0));
    object.insert("inline_lists".into(), json!([]));
    object.insert("last_opened_at".into(), Value::Null);
    for (key, default) in [
        ("project_id", Value::Null),
        ("notes", json!("")),
        ("tags", json!([])),
        ("timezone", Value::Null),
        ("languages", Value::Null),
        ("start_urls", json!([])),
    ] {
        object.entry(key).or_insert(default);
    }
    let profile: Profile = serde_json::from_value(input).map_err(|e| invalid(e.to_string()))?;
    if profile.name.trim().is_empty() {
        return Err(invalid("profile name must not be empty"));
    }
    Ok(profile)
}

impl Agent {
    pub(super) fn ensure_accepting(&self) -> anyhow::Result<()> {
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ManagementError {
                status: 503,
                code: "shutting_down",
                message: "worker is shutting down".into(),
            }
            .into());
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut ids: std::collections::HashSet<String> = self
            .lifecycle
            .lock()
            .expect("profile lock registry poisoned")
            .keys()
            .cloned()
            .collect();
        ids.extend(self.running.lock().await.keys().cloned());
        let mut errors = Vec::new();
        for id in ids {
            if let Err(e) = self.stop(&id).await {
                errors.push(e.to_string());
            }
        }
        if !errors.is_empty() {
            anyhow::bail!("shutdown cleanup errors: {}", errors.join("; "));
        }
        Ok(())
    }

    pub(super) async fn profile_guard(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self
                .lifecycle
                .lock()
                .expect("profile lock registry poisoned");
            locks.retain(|_, lock| lock.strong_count() > 0);
            let lock = locks
                .get(id)
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| std::sync::Arc::new(tokio::sync::Mutex::new(())));
            locks.insert(id.to_string(), std::sync::Arc::downgrade(&lock));
            lock
        };
        lock.lock_owned().await
    }

    pub(super) async fn ensure_stopped(&self, id: &str) -> anyhow::Result<()> {
        if self
            .running
            .lock()
            .await
            .get_mut(id)
            .map(|r| r.child.try_wait().map(|s| s.is_none()))
            .transpose()?
            .unwrap_or(false)
        {
            return Err(ManagementError {
                status: 409,
                code: "profile_running",
                message: "stop the profile before changing or deleting it".into(),
            }
            .into());
        }
        Ok(())
    }

    pub(super) async fn validate_profile(&self, profile: &Profile) -> anyhow::Result<()> {
        let persona = crate::personas::load(&profile.persona_id)
            .map_err(|_| invalid(format!("unknown persona {:?}", profile.persona_id)))?;
        persona.validate().map_err(|errs| {
            invalid(
                errs.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })?;
        profile
            .overrides
            .apply(&persona, &crate::personas::all())
            .map_err(|errs| invalid(errs.join("\n")))?;
        let proxy_id = profile
            .proxy_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .or_else(|| profile.proxy.as_ref().map(|p| p.id.as_str()));
        if let Some(id) = proxy_id {
            if !self.store.proxies().await?.iter().any(|p| p.id == id) {
                return Err(invalid("unknown proxy_id"));
            }
        }
        if let Some(id) = profile.project_id.as_deref() {
            if !self
                .store
                .projects()
                .await?
                .iter()
                .any(|project| project.id == id)
            {
                return Err(invalid("unknown project_id"));
            }
        }
        Ok(())
    }

    pub async fn create_profile(&self, input: Value) -> anyhow::Result<Value> {
        let id = uuid::Uuid::now_v7().to_string();
        let _guard = self.profile_guard(&id).await;
        self.ensure_accepting()?;
        let profile = profile_input(input, &id)?;
        self.validate_profile(&profile).await?;
        self.store.upsert_profile(&profile).await?;
        Ok(json!({"id": id}))
    }

    pub async fn get_profile(&self, id: &str) -> anyhow::Result<Value> {
        // The list representation deliberately omits proxy passwords and rotation links.
        let profile = self
            .store
            .profiles(None)
            .await?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| missing("profile"))?;
        let mut value = serde_json::to_value(profile)?;
        value["running"] = json!(
            self.running
                .lock()
                .await
                .get_mut(id)
                .map(|r| r.child.try_wait().map(|s| s.is_none()))
                .transpose()?
                .unwrap_or(false)
        );
        Ok(value)
    }

    pub async fn update_profile(&self, id: &str, input: Value) -> anyhow::Result<Value> {
        let _guard = self.profile_guard(id).await;
        self.ensure_accepting()?;
        self.ensure_stopped(id).await?;
        self.get_profile(id).await?;
        let profile = profile_input(input, id)?;
        self.validate_profile(&profile).await?;
        self.store.upsert_profile(&profile).await?;
        Ok(json!({"id": id}))
    }

    pub async fn delete_profile(&self, id: &str) -> anyhow::Result<Value> {
        let _guard = self.profile_guard(id).await;
        self.ensure_accepting()?;
        self.ensure_stopped(id).await?;
        self.get_profile(id).await?;
        self.store.delete_profile(id).await?;
        Ok(json!({"deleted": true}))
    }

    pub async fn save_proxy(&self, id: Option<&str>, mut input: Value) -> anyhow::Result<Value> {
        self.ensure_accepting()?;
        if let Some(id) = id {
            if !self.store.proxies().await?.iter().any(|p| p.id == id) {
                return Err(missing("proxy"));
            }
        }
        let object = input
            .as_object_mut()
            .ok_or_else(|| invalid("expected a JSON object"))?;
        object.insert("id".into(), json!(id.unwrap_or("")));
        for key in ["last_country", "last_ip", "last_location"] {
            object.insert(key.into(), Value::Null);
        }
        let proxy: Proxy = serde_json::from_value(input).map_err(|e| invalid(e.to_string()))?;
        if !["http", "https", "socks5"].contains(&proxy.kind.as_str())
            || proxy.host.trim().is_empty()
            || proxy.port == 0
            || proxy.name.trim().is_empty()
        {
            return Err(invalid(
                "proxy requires a name, host, nonzero port and supported kind",
            ));
        }
        Ok(json!({"id": self.store.upsert_proxy(&proxy).await?}))
    }

    #[cfg(test)]
    pub(crate) async fn test_running(&self, id: &str) {
        #[cfg(windows)]
        let child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 60"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        #[cfg(unix)]
        let child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        self.running.lock().await.insert(
            id.into(),
            super::Running {
                child,
                server: None,
                profile_key: None,
                lock_token: None,
                heartbeat: None,
                relay_port: 0,
                ws_endpoint: None,
                relay: tokio::spawn(std::future::pending()),
            },
        );
    }

    #[cfg(test)]
    pub(crate) async fn remove_test_running(&self, id: &str) {
        let mut running = self.running.lock().await.remove(id).unwrap();
        running.child.kill().unwrap();
        running.child.wait().unwrap();
        running.relay.abort();
    }

    #[cfg(test)]
    pub(crate) async fn for_tests(path: &std::path::Path) -> anyhow::Result<std::sync::Arc<Self>> {
        let agent = std::sync::Arc::new(Self {
            store: crate::store::Store::open_for_tests(path).await?,
            running: Default::default(),
            lifecycle: Default::default(),
            shutting_down: Default::default(),
            core_download: Default::default(),
            mirror: crate::mirror::Hub::new(),
            warmer: crate::warm::Warmer::new(),
            me: std::sync::OnceLock::new(),
        });
        let _ = agent.me.set(std::sync::Arc::downgrade(&agent));
        Ok(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Value {
        json!({"name": "Worker profile", "persona_id": fury_shared::catalogue::all()[0].id})
    }

    #[tokio::test]
    async fn shutdown_cleans_up_owned_children_and_relays_and_refuses_new_work() {
        let dir = crate::tmp::TempDir::new("management-shutdown");
        let agent = Agent::for_tests(&dir.join("test.db")).await.unwrap();
        let child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
            .args(if cfg!(windows) {
                ["/c", "exit 0"]
            } else {
                ["-c", "exit 0"]
            })
            .spawn()
            .unwrap();
        let relay = tokio::spawn(std::future::pending());
        let relay_done = relay.abort_handle();
        agent.running.lock().await.insert(
            "test".into(),
            super::super::Running {
                child,
                server: None,
                profile_key: None,
                lock_token: None,
                heartbeat: None,
                relay_port: 0,
                ws_endpoint: None,
                relay,
            },
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        agent
            .dispatch_public("profiles.list", json!({}))
            .await
            .unwrap();
        assert!(
            agent.running.lock().await.contains_key("test"),
            "listing must retain pending relay cleanup"
        );
        agent.shutdown().await.unwrap();
        tokio::task::yield_now().await;
        assert!(agent.running.lock().await.is_empty());
        assert!(relay_done.is_finished());
        assert_eq!(
            agent
                .create_profile(input())
                .await
                .unwrap_err()
                .downcast_ref::<ManagementError>()
                .unwrap()
                .status,
            503
        );
    }

    #[tokio::test]
    async fn lifecycle_lock_serializes_pending_mutations_and_checks_active_browser() {
        let dir = crate::tmp::TempDir::new("management-lifecycle");
        let agent = Agent::for_tests(&dir.join("test.db")).await.unwrap();
        let created = agent.create_profile(input()).await.unwrap();
        let id = created["id"].as_str().unwrap().to_string();
        let launching = agent.profile_guard(&id).await;
        let other = agent.clone();
        let target = id.clone();
        let mut pending = tokio::spawn(async move { other.update_profile(&target, input()).await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut pending)
                .await
                .is_err()
        );
        #[cfg(windows)]
        let child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 60"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        #[cfg(unix)]
        let child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        agent.running.lock().await.insert(
            id.clone(),
            super::super::Running {
                child,
                server: None,
                profile_key: None,
                lock_token: None,
                heartbeat: None,
                relay_port: 0,
                ws_endpoint: None,
                relay: tokio::spawn(std::future::pending()),
            },
        );
        drop(launching);
        let update = pending.await.unwrap();
        let delete = agent.delete_profile(&id).await;
        let ipc_update = agent
            .dispatch_public(
                "profiles.upsert",
                serde_json::to_value(agent.store.profiles(None).await.unwrap().remove(0)).unwrap(),
            )
            .await;
        let ipc_delete = agent
            .dispatch_public("profiles.delete", json!({"id": id}))
            .await;
        let mut running = agent.running.lock().await.remove(&id).unwrap();
        running.child.kill().unwrap();
        running.child.wait().unwrap();
        running.relay.abort();
        for result in [update, delete, ipc_update, ipc_delete] {
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<ManagementError>()
                    .unwrap()
                    .status,
                409
            );
        }
        assert!(agent.get_profile(&id).await.is_ok());
        agent.delete_profile(&id).await.unwrap();
    }

    #[tokio::test]
    async fn profile_management_retains_identity_and_rejects_invalid_references() {
        let dir = crate::tmp::TempDir::new("management");
        let agent = Agent::for_tests(&dir.join("test.db")).await.unwrap();
        let created = agent.create_profile(input()).await.unwrap();
        let id = created["id"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(id).is_ok());
        let before = agent.get_profile(id).await.unwrap();
        assert!(before["fp_seed"].as_i64().unwrap() > 0);
        let mut changed = input();
        changed["name"] = json!("Renamed");
        agent.update_profile(id, changed).await.unwrap();
        let after = agent.get_profile(id).await.unwrap();
        assert_eq!(before["fp_seed"], after["fp_seed"]);
        assert_eq!(after["name"], "Renamed");
        assert_eq!(after["id"], id);
        assert_eq!(
            agent
                .update_profile("missing", input())
                .await
                .unwrap_err()
                .downcast_ref::<ManagementError>()
                .unwrap()
                .status,
            404
        );
        for (key, value) in [
            ("persona_id", json!("unknown")),
            ("proxy_id", json!("unknown")),
            ("fp_seed", json!(10)),
        ] {
            let mut bad = input();
            bad[key] = value;
            assert!(agent.create_profile(bad).await.is_err());
        }
        assert_eq!(agent.store.profiles(None).await.unwrap().len(), 1);
        let mut bad_override = input();
        bad_override["overrides"] = json!({"cores": 999});
        assert!(agent.create_profile(bad_override).await.is_err());
        agent.delete_profile(id).await.unwrap();
        assert!(agent.get_profile(id).await.is_err());
        assert!(agent.store.deleted_profile_exists(id).await.unwrap());
    }

    #[tokio::test]
    async fn profile_retrieval_redacts_secrets_and_proxy_validation_is_explicit() {
        let dir = crate::tmp::TempDir::new("management-proxy");
        let agent = Agent::for_tests(&dir.join("test.db")).await.unwrap();
        let proxy = agent
            .save_proxy(
                None,
                json!({"name":"Exit", "kind":"socks5",
            "host":"127.0.0.1", "port":1080, "username":"user", "password":"secret",
            "rotate_url":"https://example.com/secret-rotation"}),
            )
            .await
            .unwrap();
        let mut profile = input();
        profile["proxy_id"] = proxy["id"].clone();
        let created = agent.create_profile(profile).await.unwrap();
        let value = agent
            .get_profile(created["id"].as_str().unwrap())
            .await
            .unwrap();
        assert!(value["proxy"]["password"].is_null());
        assert!(value["proxy"]["rotate_url"].is_null());
        assert_eq!(value["proxy_id"], proxy["id"]);
        assert!(agent.save_proxy(Some("missing"), json!({})).await.is_err());
        assert!(
            agent
                .save_proxy(
                    None,
                    json!({"name":"Bad", "kind":"ftp", "host":"x", "port":1})
                )
                .await
                .is_err()
        );
    }
}
