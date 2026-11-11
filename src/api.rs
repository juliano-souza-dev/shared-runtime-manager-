use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const API_REGISTRY: &str = "registry/apis.json";
const CREDENTIAL_SERVICE: &str = "JulianoDev.SharedRuntime.Api";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub name: String,
    pub endpoint: String,
    pub model: String,
    pub authorized_apps: BTreeSet<String>,
    pub updated_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ApiRegistry {
    schema_version: u32,
    providers: BTreeMap<String, ApiConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub model: String,
    pub authorized_apps: Vec<String>,
    pub updated_at: u64,
}

fn timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn validate_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 64 ||
        !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("Invalid identifier '{value}'. Use up to 64 letters, numbers, '-' or '_'.");
    }
    Ok(())
}

fn registry_path() -> Result<PathBuf> {
    let root = super::manager::root()?;
    fs::create_dir_all(root.join("registry"))?;
    Ok(root.join(API_REGISTRY))
}

fn read_registry() -> Result<ApiRegistry> {
    let path = registry_path()?;
    if !path.exists() {
        return Ok(ApiRegistry { schema_version: 1, ..ApiRegistry::default() });
    }
    let registry: ApiRegistry = serde_json::from_slice(&fs::read(path)?)?;
    if registry.schema_version != 1 {
        bail!("Unsupported API registry schema version: {}", registry.schema_version);
    }
    Ok(registry)
}

fn write_registry(registry: &ApiRegistry) -> Result<()> {
    let path = registry_path()?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(registry)?;
    {
        let mut file = File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}


/// Bundled, non-secret metadata used to prefill known API providers.
pub fn provider_catalog() -> Result<serde_json::Value> {
    Ok(serde_json::from_str(include_str!("../assets/api-providers.json"))?)
}

/// Retrieves the model IDs exposed by a provider's official models endpoint.
pub fn available_models(provider: &str) -> Result<Vec<String>> {
    validate_id(provider)?;
    let catalog = provider_catalog()?;
    let entry = catalog.get("providers").and_then(serde_json::Value::as_array)
        .and_then(|items| items.iter().find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(provider)))
        .ok_or_else(|| anyhow!("Unknown API provider '{provider}'."))?;
    let endpoint = entry.get("modelsEndpoint").and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("Provider '{provider}' has no models endpoint."))?;
    let auth = entry.get("auth").and_then(serde_json::Value::as_str).unwrap_or("bearer");
    #[cfg(windows)]
    let secret = credential_entry(provider)?.get_password()
        .context("Store the provider API key before refreshing its model list")?;
    #[cfg(not(windows))]
    bail!("Secure API credential storage is currently supported on Windows only.");

    #[cfg(windows)]
    {
        let client = reqwest::blocking::Client::builder()
            .user_agent("JulianoDev-SharedRuntimeManager/0.5")
            .build()?;
        let mut request = client.get(endpoint);
        match auth {
            "query" => { request = request.query(&[("key", secret.as_str())]); }
            "anthropic" => {
                request = request
                    .header("x-api-key", &secret)
                    .header("anthropic-version", "2023-06-01");
            }
            _ => { request = request.bearer_auth(&secret); }
        }
        let response: serde_json::Value = request.send()
            .with_context(|| format!("Could not contact {provider} models endpoint"))?
            .error_for_status()
            .with_context(|| format!("{provider} rejected the models request; check the API key and account access"))?
            .json()?;
        let entries = response.get("data").or_else(|| response.get("models"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow!("Unexpected models response from {provider}."))?;
        let mut models = Vec::new();
        for item in entries {
            if item.get("active").and_then(serde_json::Value::as_bool) == Some(false) { continue; }
            if let Some(methods) = item.get("supportedGenerationMethods").and_then(serde_json::Value::as_array) {
                if !methods.iter().any(|method| method.as_str() == Some("generateContent")) { continue; }
            }
            let raw = item.get("id").and_then(serde_json::Value::as_str)
                .or_else(|| item.get("name").and_then(serde_json::Value::as_str));
            if let Some(raw) = raw {
                let id = raw.strip_prefix("models/").unwrap_or(raw);
                if !id.is_empty() { models.push(id.to_owned()); }
            }
        }
        models.sort();
        models.dedup();
        if models.is_empty() { bail!("No usable models were returned by {provider}."); }
        Ok(models)
    }
}

pub fn add(provider: &str, name: &str, endpoint: &str, model: &str) -> Result<()> {
    validate_id(provider)?;
    if name.trim().is_empty() || endpoint.trim().is_empty() || model.trim().is_empty() {
        bail!("Name, endpoint, and model are required.");
    }
    let mut registry = read_registry()?;
    let authorized_apps = registry.providers.get(provider)
        .map(|item| item.authorized_apps.clone()).unwrap_or_default();
    registry.providers.insert(provider.to_owned(), ApiConfig {
        name: name.trim().to_owned(),
        endpoint: endpoint.trim().to_owned(),
        model: model.trim().to_owned(),
        authorized_apps,
        updated_at: timestamp(),
    });
    write_registry(&registry)?;
    println!("Global API configuration saved: {provider}");
    println!("Credentials are managed separately by the operating system.");
    Ok(())
}

pub fn authorize(provider: &str, app: &str) -> Result<()> {
    validate_id(provider)?;
    validate_id(app)?;
    let mut registry = read_registry()?;
    let config = registry.providers.get_mut(provider)
        .ok_or_else(|| anyhow!("API provider '{provider}' is not configured."))?;
    config.authorized_apps.insert(app.to_owned());
    config.updated_at = timestamp();
    write_registry(&registry)?;
    println!("Application '{app}' is authorized to use '{provider}'.");
    Ok(())
}

pub fn providers() -> Result<Vec<ProviderInfo>> {
    let registry = read_registry()?;
    Ok(registry.providers.into_iter().map(|(id, config)| ProviderInfo {
        id,
        name: config.name,
        endpoint: config.endpoint,
        model: config.model,
        authorized_apps: config.authorized_apps.into_iter().collect(),
        updated_at: config.updated_at,
    }).collect())
}

pub fn revoke(provider: &str, app: &str) -> Result<()> {
    validate_id(provider)?;
    validate_id(app)?;
    let mut registry = read_registry()?;
    let config = registry.providers.get_mut(provider)
        .ok_or_else(|| anyhow!("API provider '{provider}' is not configured."))?;
    if config.authorized_apps.remove(app) {
        config.updated_at = timestamp();
        write_registry(&registry)?;
        println!("Application '{app}' access to '{provider}' was revoked.");
    }
    Ok(())
}

pub fn list() -> Result<()> {
    let registry = read_registry()?;
    if registry.providers.is_empty() {
        println!("No global API configurations registered.");
    }
    for (provider, config) in registry.providers {
        println!("{provider}\t{}\tmodel={}\t{} authorized app(s)",
            config.name, config.model, config.authorized_apps.len());
    }
    Ok(())
}

pub fn resolve(provider: &str, app: &str) -> Result<()> {
    validate_id(provider)?;
    validate_id(app)?;
    let registry = read_registry()?;
    let config = registry.providers.get(provider)
        .ok_or_else(|| anyhow!("API provider '{provider}' is not configured."))?;
    if !config.authorized_apps.contains(app) {
        bail!("Application '{app}' is not authorized to use API '{provider}'.");
    }
    println!("{}", serde_json::to_string_pretty(config)?);
    Ok(())
}

#[cfg(windows)]
fn credential_entry(provider: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(CREDENTIAL_SERVICE, &format!("api:{provider}"))?)
}

pub fn set_key(provider: &str, secret: &str) -> Result<()> {
    validate_id(provider)?;
    let registry = read_registry()?;
    if !registry.providers.contains_key(provider) {
        bail!("Configure the provider before setting its key.");
    }
    let secret = secret.trim();
    if secret.is_empty() {
        bail!("The API key cannot be empty.");
    }
    #[cfg(windows)]
    {
        credential_entry(provider)?.set_password(secret)
            .context("Could not store the API key in Windows Credential Manager")?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = secret;
        bail!("Secure API credential storage is currently supported on Windows only.");
    }
}

pub fn has_key(provider: &str) -> Result<bool> {
    validate_id(provider)?;
    #[cfg(windows)]
    {
        match credential_entry(provider)?.get_password() {
            Ok(_) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(error) => Err(error).context("Could not query Windows Credential Manager"),
        }
    }
    #[cfg(not(windows))]
    {
        bail!("Secure API credential storage is currently supported on Windows only.")
    }
}

pub fn set_key_from_stdin(provider: &str) -> Result<()> {
    validate_id(provider)?;
    let registry = read_registry()?;
    if !registry.providers.contains_key(provider) {
        bail!("Configure the provider before setting its key: srm api add {provider} ...");
    }
    let mut secret = String::new();
    std::io::stdin().read_to_string(&mut secret)
        .context("Could not read API key from stdin")?;
    let secret = secret.trim();
    if secret.is_empty() {
        bail!("The API key cannot be empty.");
    }
    #[cfg(windows)]
    {
        credential_entry(provider)?.set_password(secret)
            .context("Could not store the API key in Windows Credential Manager")?;
        println!("API credential stored securely for '{provider}'.");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = secret;
        bail!("Secure API credential storage is currently supported on Windows only.");
    }
}

pub fn key_status(provider: &str) -> Result<()> {
    validate_id(provider)?;
    #[cfg(windows)]
    {
        match credential_entry(provider)?.get_password() {
            Ok(_) => println!("Credential is present for '{provider}'."),
            Err(keyring::Error::NoEntry) => println!("No credential stored for '{provider}'."),
            Err(error) => return Err(error).context("Could not query Windows Credential Manager"),
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        bail!("Secure API credential storage is currently supported on Windows only.")
    }
}

/// Returns non-secret provider settings only when the application is authorized.
pub fn get_settings_for_app(provider: &str, app: &str) -> Result<ApiConfig> {
    validate_id(provider)?;
    validate_id(app)?;
    let registry = read_registry()?;
    let config = registry.providers.get(provider)
        .ok_or_else(|| anyhow!("API provider '{provider}' is not configured."))?;
    if !config.authorized_apps.contains(app) {
        bail!("Application '{app}' is not authorized to use API '{provider}'.");
    }
    Ok(config.clone())
}

// Shared by Windows desktop applications through the same Credential Manager service/account convention.
// The secret is deliberately never printed by the CLI.
#[cfg(windows)]
#[allow(dead_code)]
pub fn get_secret_for_app(provider: &str, app: &str) -> Result<String> {
    validate_id(provider)?;
    validate_id(app)?;
    let registry = read_registry()?;
    let config = registry.providers.get(provider)
        .ok_or_else(|| anyhow!("API provider '{provider}' is not configured."))?;
    if !config.authorized_apps.contains(app) {
        bail!("Application '{app}' is not authorized to use API '{provider}'.");
    }
    credential_entry(provider)?.get_password()
        .context("API credential is missing from Windows Credential Manager")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_and_app_ids_are_restricted() {
        assert!(validate_id("groq").is_ok());
        assert!(validate_id("ttk-media").is_ok());
        assert!(validate_id("../secret").is_err());
        assert!(validate_id("app name").is_err());
    }
}
