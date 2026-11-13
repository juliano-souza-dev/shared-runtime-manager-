use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}, sync::Mutex};

static QUEUE_LOCK: Mutex<()> = Mutex::new(());

fn root() -> Result<PathBuf> { super::manager::root() }
fn queue_path(root: &Path) -> PathBuf { root.join("registry/dependency-queue.json") }
fn status_path(root: &Path) -> PathBuf { root.join("registry/dependency-queue-status.json") }
fn resolved_path(root: &Path) -> PathBuf { root.join("registry/resolved-dependencies.json") }

fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(value)?)?;
    if path.exists() { fs::remove_file(path)?; }
    fs::rename(temp, path)?;
    Ok(())
}
fn read_json(path: &Path) -> Result<Value> {
    if !path.exists() { return Ok(json!({"items":[]})); }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn dependencies_for(config: &Value) -> Vec<String> {
    let top = config.get("dependencies").and_then(Value::as_array);
    let shared = config.pointer("/shared/dependencies").and_then(Value::as_array);
    let declared_count = top.map(|items| items.len()).unwrap_or(0) + shared.map(|items| items.len()).unwrap_or(0);
    let mut ids = BTreeSet::new();
    for value in top.into_iter().flatten().chain(shared.into_iter().flatten()) {
        if value.get("required").and_then(Value::as_bool).unwrap_or(true) {
            if let Some(id) = value.get("id").and_then(Value::as_str) { ids.insert(id.to_owned()); }
        }
    }
    if declared_count == 0 {
        ["ffmpeg","ffprobe","yt-dlp","python","faster-whisper","pyav","whisper-models"].iter().map(|s| s.to_string()).collect()
    } else { ids.into_iter().collect() }
}

/// Merge dependency declarations from the application's checked-in example into its
/// generated local config. Existing custom dependencies are preserved; matching IDs
/// inherit the template's required/version policy so changed requirements take effect
/// on both first registration and later synchronization.
fn merge_example_dependencies(installation_path: &str, config: &mut Value) -> Result<()> {
    let example_path = PathBuf::from(installation_path).join("shared.config.example.json");
    if !example_path.is_file() {
        return Ok(());
    }
    let example: Value = serde_json::from_slice(&fs::read(&example_path)
        .with_context(|| format!("Could not read {}", example_path.display()))?)
        .with_context(|| format!("{} contains invalid JSON", example_path.display()))?;
    let Some(template_dependencies) = example.pointer("/shared/dependencies").and_then(Value::as_array) else {
        return Ok(());
    };

    if !config.get("shared").is_some_and(Value::is_object) {
        config["shared"] = json!({});
    }
    if !config.pointer("/shared/dependencies").is_some_and(Value::is_array) {
        config["shared"]["dependencies"] = json!([]);
    }
    let current = config["shared"]["dependencies"].as_array_mut()
        .expect("dependencies array was initialized above");

    for template in template_dependencies {
        let Some(id) = template.get("id").and_then(Value::as_str) else { continue };
        if let Some(existing) = current.iter_mut()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
        {
            for field in ["required", "version"] {
                if let Some(value) = template.get(field) {
                    existing[field] = value.clone();
                }
            }
        } else {
            current.push(template.clone());
        }
    }
    Ok(())
}

/// Adds a project to the durable queue without attempting to install dependencies.
pub fn enqueue_application(app_id: &str, installation_path: &str) -> Result<()> {
    let _guard = QUEUE_LOCK.lock().map_err(|_| anyhow::anyhow!("Dependency queue is busy"))?;
    let root = root()?;
    let config_path = PathBuf::from(installation_path).join("shared.config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&config_path)
        .with_context(|| format!("Could not read {}", config_path.display()))?)?;
    merge_example_dependencies(installation_path, &mut config)?;
    let dependencies = dependencies_for(&config);
    // Keep the root compatibility list limited to auto-installed requirements,
    // but preserve the full application-owned shared.dependencies declarations,
    // including optional resources that can be installed explicitly later.
    config["dependencies"] = Value::Array(dependencies.iter().map(|id| json!({"id":id,"required":true})).collect());
    let data_root = PathBuf::from(installation_path).join("data");
    let mut app_data = BTreeMap::<String, String>::new();
    for name in ["projects", "database", "sounds", "logs", "user-files"] {
        let directory = data_root.join(name);
        fs::create_dir_all(&directory)?;
        app_data.insert(name.to_owned(), directory.to_string_lossy().into_owned());
    }
    config["application"]["data"] = serde_json::to_value(app_data)?;
    write_json(&config_path, &config)?;
    let path = queue_path(&root);
    let mut queue = read_json(&path)?;
    let items = queue.get_mut("items").and_then(Value::as_array_mut);
    let items = items.ok_or_else(|| anyhow::anyhow!("Dependency queue has an invalid format"))?;
    items.retain(|item| item.get("app_id").and_then(Value::as_str) != Some(app_id));
    items.push(json!({"app_id":app_id,"installation_path":installation_path,"dependencies":dependencies,"status":"queued"}));
    write_json(&path, &queue)?;
    write_json(&status_path(&root), &json!({
        "status":"queued","progress":0,"current":"Aguardando instalação das dependências",
        "application":app_id,"results":[]
    }))?;
    Ok(())
}

/// Installs each shared dependency once, records individual outcomes, then refreshes
/// every registered project's config with the paths for dependencies it declares.
pub fn process_queue() -> Result<()> {
    let _guard = QUEUE_LOCK.lock().map_err(|_| anyhow::anyhow!("Dependency queue is busy"))?;
    let root = root()?;
    let queue = read_json(&queue_path(&root))?;
    let items = queue.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut unique = BTreeSet::new();
    for item in &items {
        if let Some(deps) = item.get("dependencies").and_then(Value::as_array) {
            for dep in deps { if let Some(id) = dep.as_str() { unique.insert(id.to_owned()); } }
        }
    }
    let total = unique.len().max(1);
    let resolved_file = resolved_path(&root);
    let mut resolved: Value = if resolved_file.exists() { serde_json::from_slice(&fs::read(&resolved_file)?)? } else { json!({}) };
    if !resolved.is_object() { resolved = json!({}); }
    let mut results = Vec::new();
    for (index, id) in unique.iter().enumerate() {
        write_json(&status_path(&root), &json!({
            "status":"installing","progress":((index * 100) / total),
            "current":format!("Instalando {id}"),"completed":index,"total":total,
            "results":results
        }))?;
        match super::dependency_manager::ensure_dependency(&root, id) {
            Ok(path) => {
                let value = path.to_string_lossy().into_owned();
                resolved[id.as_str()] = Value::String(value.clone());
                results.push(json!({"id":id,"status":"installed","path":value,"message":"Instalado ou já disponível"}));
                write_json(&resolved_path(&root), &resolved)?;
                refresh_registered_configs(&root, &resolved)?;
            }
            Err(error) => {
                results.push(json!({"id":id,"status":"error","message":format!("{error:#}")}));
            }
        }
        write_json(&status_path(&root), &json!({
            "status":"installing","progress":(((index + 1) * 100) / total),
            "current":format!("Processado: {id}"),"completed":index+1,"total":total,
            "results":results
        }))?;
    }
    refresh_registered_configs(&root, &resolved)?;
    write_json(&queue_path(&root), &json!({"items":[]}))?;
    write_json(&status_path(&root), &json!({
        "status":"completed","progress":100,"current":"Processamento concluído",
        "completed":total,"total":total,"results":results
    }))?;
    Ok(())
}

fn refresh_registered_configs(root: &Path, resolved: &Value) -> Result<()> {
    let registry_path = root.join("registry/state.json");
    let registry = read_json(&registry_path)?;
    let applications = registry.get("applications").and_then(Value::as_object)
        .cloned().unwrap_or_default();
    for (app_id, app) in applications {
        let installation = app.get("installation_path").and_then(Value::as_str).unwrap_or("");
        if installation.is_empty() { continue; }
        let config_path = PathBuf::from(installation).join("shared.config.json");
        if !config_path.is_file() { continue; }
        let mut config: Value = match serde_json::from_slice::<Value>(&fs::read(&config_path)?) {
            Ok(value) if value.is_object() => value,
            _ => continue,
        };
        let deps = dependencies_for_without_default(&config);
        let mut app_resolved = BTreeMap::<String,String>::new();
        for id in deps {
            if let Some(path) = resolved.get(&id).and_then(Value::as_str) {
                app_resolved.insert(id, path.to_owned());
            }
        }
        let resolved_count = app_resolved.len();
        let mut shared_resources = config.pointer("/shared/resources")
            .filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}));
        for (id, path) in &app_resolved {
            shared_resources[id] = Value::String(path.clone());
        }
        config["shared"]["resources"] = shared_resources;
        config["shared"]["resolvedDependencies"] = serde_json::to_value(app_resolved)?;
        config["shared"]["dependencyStatus"] = json!({
            "updatedAt": chrono_placeholder_timestamp(),
            "resolved": resolved_count
        });
        write_json(&config_path, &config)
            .with_context(|| format!("Could not refresh config for application {app_id}"))?;
    }
    Ok(())
}
fn dependencies_for_without_default(config: &Value) -> Vec<String> {
    let top = config.get("dependencies").and_then(Value::as_array);
    let shared = config.pointer("/shared/dependencies").and_then(Value::as_array);
    let mut ids = BTreeSet::new();
    for value in top.into_iter().flatten().chain(shared.into_iter().flatten()) {
        // Link optional declarations too, but only when their resource is installed.
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            ids.insert(id.to_owned());
        }
    }
    ids.into_iter().collect()
}
fn chrono_placeholder_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
/// Installs an optional Shared resource on demand and links it to every
/// registered application that declares it in shared.dependencies.
pub fn install_dependency(resource_id: &str, app_id: Option<&str>) -> Result<()> {
    let _guard = QUEUE_LOCK.lock().map_err(|_| anyhow::anyhow!("Dependency queue is busy"))?;
    let root = root()?;
    let registry = read_json(&root.join("registry/state.json"))?;
    if let Some(id) = app_id {
        let application = registry.pointer(&format!("/applications/{id}"))
            .ok_or_else(|| anyhow::anyhow!("Application '{id}' is not registered in Shared Runtime"))?;
        let installation = application.get("installation_path").and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Application '{id}' has no registered installation path"))?;
        let config_path = PathBuf::from(installation).join("shared.config.json");
        let config: Value = serde_json::from_slice(&fs::read(&config_path)
            .with_context(|| format!("Could not read {}", config_path.display()))?)?;
        let declared = config.pointer("/shared/dependencies").and_then(Value::as_array)
            .into_iter().flatten().any(|entry| entry.get("id").and_then(Value::as_str) == Some(resource_id));
        if !declared {
            anyhow::bail!("Application '{id}' does not declare '{resource_id}' in shared.dependencies.");
        }
    }

    let installed_path = super::dependency_manager::ensure_dependency(&root, resource_id)?;
    let resolved_file = resolved_path(&root);
    let mut resolved = read_json(&resolved_file)?;
    if !resolved.is_object() { resolved = json!({}); }
    resolved[resource_id] = Value::String(installed_path.to_string_lossy().into_owned());
    write_json(&resolved_file, &resolved)?;
    refresh_registered_configs(&root, &resolved)?;
    println!("Shared resource '{resource_id}' is ready: {}", installed_path.display());
    Ok(())
}

pub fn status() -> Result<Value> {
    let root = root()?;
    read_json(&status_path(&root))
}

pub fn remove_application(app_id: &str) -> Result<()> {
    let root = root()?;
    let path = queue_path(&root);
    let mut queue = read_json(&path)?;
    if let Some(items) = queue.get_mut("items").and_then(Value::as_array_mut) {
        items.retain(|item| item.get("app_id").and_then(Value::as_str) != Some(app_id));
    }
    write_json(&path, &queue)?;
    Ok(())
}

pub fn set_failure(message: &str) -> Result<()> {
    let root = root()?;
    write_json(&status_path(&root), &json!({
        "status":"error","progress":0,"current":"Falha no processamento da fila",
        "error":message,"results":[]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_resources_are_not_queued_but_can_be_linked_when_installed() {
        let config = json!({
            "dependencies": [
                {"id": "ffmpeg", "required": true}
            ],
            "shared": {
                "dependencies": [
                    {"id": "ffmpeg", "required": true},
                    {"id": "forced-alignment", "required": false}
                ]
            }
        });

        assert_eq!(dependencies_for(&config), vec!["ffmpeg".to_owned()]);
        let linked = dependencies_for_without_default(&config);
        assert!(linked.contains(&"forced-alignment".to_owned()));
    }

    #[test]
    fn example_template_promotes_forced_alignment_and_preserves_custom_dependencies() {
        let directory = std::env::temp_dir().join(format!(
            "shared-runtime-template-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("shared.config.example.json"),
            json!({
                "shared": {
                    "dependencies": [
                        {"id": "forced-alignment", "required": true},
                        {"id": "ffmpeg", "required": true}
                    ]
                }
            }).to_string()
        ).unwrap();

        let mut config = json!({
            "shared": {
                "dependencies": [
                    {"id": "forced-alignment", "required": false},
                    {"id": "custom-tool", "required": true}
                ]
            }
        });
        merge_example_dependencies(directory.to_str().unwrap(), &mut config).unwrap();

        assert_eq!(
            config.pointer("/shared/dependencies/0/required"),
            Some(&Value::Bool(true))
        );
        assert!(config.pointer("/shared/dependencies/1/id")
            .and_then(Value::as_str) == Some("custom-tool"));
        assert!(dependencies_for(&config).contains(&"forced-alignment".to_owned()));

        fs::remove_dir_all(directory).unwrap();
    }
}
