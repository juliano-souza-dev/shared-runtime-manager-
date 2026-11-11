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
    let values = config.get("dependencies").and_then(Value::as_array)
        .or_else(|| config.pointer("/shared/dependencies").and_then(Value::as_array));
    let ids: Vec<String> = values.into_iter().flatten().filter_map(|v| {
        if v.get("required").and_then(Value::as_bool).unwrap_or(true) {
            v.get("id").and_then(Value::as_str).map(str::to_owned)
        } else { None }
    }).collect();
    if ids.is_empty() {
        ["ffmpeg","ffprobe","yt-dlp","python","faster-whisper","pyav","whisper-models"].iter().map(|s| s.to_string()).collect()
    } else { ids }
}

/// Adds a project to the durable queue without attempting to install dependencies.
pub fn enqueue_application(app_id: &str, installation_path: &str) -> Result<()> {
    let _guard = QUEUE_LOCK.lock().map_err(|_| anyhow::anyhow!("Dependency queue is busy"))?;
    let root = root()?;
    let config_path = PathBuf::from(installation_path).join("shared.config.json");
    let config: Value = serde_json::from_slice(&fs::read(&config_path)
        .with_context(|| format!("Could not read {}", config_path.display()))?)?;
    let dependencies = dependencies_for(&config);
    let mut config = config;
    config["dependencies"] = Value::Array(dependencies.iter().map(|id| json!({"id":id,"required":true})).collect());
    let declared = config["dependencies"].clone();
    config["shared"]["dependencies"] = declared;
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
    let mut queue = read_json(&queue_path(&root))?;
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
        let mut config: Value = match serde_json::from_slice(&fs::read(&config_path)?) {
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
    config.get("dependencies").and_then(Value::as_array)
        .or_else(|| config.pointer("/shared/dependencies").and_then(Value::as_array))
        .into_iter().flatten().filter_map(|v| {
            if v.get("required").and_then(Value::as_bool).unwrap_or(true) {
                v.get("id").and_then(Value::as_str).map(str::to_owned)
            } else { None }
        }).collect()
}
fn chrono_placeholder_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
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
