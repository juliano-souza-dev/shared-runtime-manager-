use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const API_REGISTRY: &str = "registry/apis.json";
const CREDENTIAL_SERVICE: &str = "JulianoDev.SharedRuntime.Api";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub name: String,
    pub endpoint: String,
    pub model: String,
    #[serde(default)]
    pub credential: Option<EncryptedCredential>,
    pub authorized_apps: BTreeSet<String>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedCredential {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
}

/// Provider data copied into an application's local shared.config.json.
/// The credential remains encrypted until the consuming backend asks to use it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRuntimeConfig {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub model: String,
    pub credential: EncryptedCredential,
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
    migrate_legacy_credentials()?;
    let registry = read_registry()?;
    let config = registry.providers.get(provider)
        .ok_or_else(|| anyhow!("API provider '{provider}' is not configured."))?;
    let secret = config.credential.as_ref()
        .ok_or_else(|| anyhow!("Store the provider API key before refreshing its model list"))
        .and_then(decrypt_credential)?;
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
                    .header("x-api-key", secret.as_str())
                    .header("anthropic-version", "2026-10-21");
            }
            _ => { request = request.bearer_auth(&secret); }
        }
        let response = request.send()
            .with_context(|| format!("Could not contact {provider} models endpoint"))?
            .error_for_status()
            .with_context(|| format!("{provider} rejected the models request; check the API key and account access"))?;
        let body = response.bytes()?;
        let response: serde_json::Value = serde_json::from_slice(&body)?;
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
    let previous = registry.providers.get(provider);
    let authorized_apps = previous.map(|item| item.authorized_apps.clone()).unwrap_or_default();
    let credential = previous.and_then(|item| item.credential.clone());
    registry.providers.insert(provider.to_owned(), ApiConfig {
        name: name.trim().to_owned(),
        endpoint: endpoint.trim().to_owned(),
        model: model.trim().to_owned(),
        credential,
        authorized_apps,
        updated_at: timestamp(),
    });
    write_registry(&registry)?;
    super::manager::refresh_application_configs(&super::manager::root()?)?;
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
    if config.credential.is_none() {
        bail!("Set an API key for '{provider}' before authorizing an application.");
    }
    config.authorized_apps.insert(app.to_owned());
    config.updated_at = timestamp();
    write_registry(&registry)?;
    super::manager::refresh_application_configs(&super::manager::root()?)?;
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
        super::manager::refresh_application_configs(&super::manager::root()?)?;
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
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "name": config.name,
        "endpoint": config.endpoint,
        "model": config.model,
        "authorized_apps": config.authorized_apps,
        "updated_at": config.updated_at
    }))?);
    Ok(())
}

#[cfg(windows)]
fn credential_entry(provider: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(CREDENTIAL_SERVICE, &format!("api:{provider}"))?)
}

pub fn set_key(provider: &str, secret: &str) -> Result<()> {
    validate_id(provider)?;
    let secret = secret.trim();
    if secret.is_empty() {
        bail!("The API key cannot be empty.");
    }
    let encrypted = encrypt_credential(secret)?;
    let mut registry = read_registry()?;
    let config = registry.providers.get_mut(provider)
        .ok_or_else(|| anyhow!("Configure the provider before setting its key."))?;
    config.credential = Some(encrypted);
    config.updated_at = timestamp();
    write_registry(&registry)?;
    super::manager::refresh_application_configs(&super::manager::root()?)?;
    Ok(())
}

pub fn has_key(provider: &str) -> Result<bool> {
    validate_id(provider)?;
    migrate_legacy_credentials()?;
    let registry = read_registry()?;
    Ok(registry.providers.get(provider).and_then(|item| item.credential.as_ref()).is_some())
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
    set_key(provider, secret)?;
    println!("API credential encrypted and stored in Shared configuration for '{provider}'.");
    Ok(())
}

pub fn key_status(provider: &str) -> Result<()> {
    validate_id(provider)?;
    let present = has_key(provider)?;
    if present {
        println!("Credential is present for '{provider}'.");
    } else {
        println!("No credential stored for '{provider}'.");
    }
    Ok(())
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

/// Returns the encrypted provider entries that belong in one application's generated config.
pub fn providers_for_app(app: &str) -> Result<Vec<ProviderRuntimeConfig>> {
    validate_id(app)?;
    migrate_legacy_credentials()?;
    let registry = read_registry()?;
    registry.providers.into_iter()
        .filter(|(_, config)| config.authorized_apps.contains(app))
        .map(|(id, config)| {
            let credential = config.credential
                .ok_or_else(|| anyhow!("Provider '{id}' is authorized for '{app}' but has no API key."))?;
            Ok(ProviderRuntimeConfig {
                id,
                name: config.name,
                endpoint: config.endpoint,
                model: config.model,
                credential,
            })
        })
        .collect()
}

/// Reads a provider configuration from the application's own shared.config.json.
/// The application ID is checked to prevent accidentally loading another project's credentials.
pub fn get_provider_settings_from_config(
    config_path: impl AsRef<Path>,
    app: &str,
    provider: &str,
) -> Result<ProviderRuntimeConfig> {
    validate_id(app)?;
    validate_id(provider)?;
    let bytes = fs::read(config_path.as_ref())
        .with_context(|| format!("Could not read Shared config {}", config_path.as_ref().display()))?;
    let document: serde_json::Value = serde_json::from_slice(&bytes)
        .context("Shared config contains invalid JSON")?;
    let configured_app = document.pointer("/application/id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("Shared config does not declare application.id"))?;
    if configured_app != app {
        bail!("Shared config belongs to '{configured_app}', not '{app}'.");
    }
    let entries = document.pointer("/shared/apiProviders")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("No API providers are available in this application's Shared config."))?;
    let entry = entries.iter()
        .find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(provider))
        .ok_or_else(|| anyhow!("Application '{app}' is not authorized to use API '{provider}'."))?;
    serde_json::from_value(entry.clone())
        .with_context(|| format!("Provider '{provider}' has an invalid Shared configuration"))
}

/// Decrypts a credential from the application's local Shared config.
pub fn get_provider_credential_from_config(
    config_path: impl AsRef<Path>,
    app: &str,
    provider: &str,
) -> Result<String> {
    let settings = get_provider_settings_from_config(config_path, app, provider)?;
    decrypt_credential(&settings.credential)
}

/// Lists the providers provisioned for an application without exposing plaintext credentials.
pub fn get_authorized_providers_from_config(
    config_path: impl AsRef<Path>,
    app: &str,
) -> Result<Vec<ProviderRuntimeConfig>> {
    validate_id(app)?;
    let bytes = fs::read(config_path.as_ref())
        .with_context(|| format!("Could not read Shared config {}", config_path.as_ref().display()))?;
    let document: serde_json::Value = serde_json::from_slice(&bytes)
        .context("Shared config contains invalid JSON")?;
    let configured_app = document.pointer("/application/id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("Shared config does not declare application.id"))?;
    if configured_app != app {
        bail!("Shared config belongs to '{configured_app}', not '{app}'.");
    }
    let entries = document.pointer("/shared/apiProviders")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    entries.into_iter().map(|entry| serde_json::from_value(entry)
        .context("An API provider entry in Shared config is invalid"))
        .collect()
}

// Legacy Credential Manager support is retained only to migrate existing installations.
#[cfg(windows)]
fn credential_entry(provider: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(CREDENTIAL_SERVICE, &format!("api:{provider}"))?)
}

#[cfg(windows)]
fn migrate_legacy_credentials() -> Result<()> {
    let mut registry = read_registry()?;
    let mut changed = false;
    for (provider, config) in registry.providers.iter_mut() {
        if config.credential.is_some() { continue; }
        match credential_entry(provider)?.get_password() {
            Ok(secret) => {
                config.credential = Some(encrypt_credential(&secret)?);
                config.updated_at = timestamp();
                changed = true;
            }
            Err(keyring::Error::NoEntry) => {}
            Err(error) => return Err(error).context("Could not read a legacy API credential"),
        }
    }
    if changed { write_registry(&registry)?; }
    Ok(())
}

#[cfg(not(windows))]
fn migrate_legacy_credentials() -> Result<()> { Ok(()) }

#[cfg(windows)]
fn encrypt_credential(secret: &str) -> Result<EncryptedCredential> {
    use std::{ffi::c_void, ptr::null, slice};
    use windows_sys::Win32::{
        Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN},
        System::Memory::LocalFree,
    };
    let input_bytes = secret.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: input_bytes.len() as u32,
        pbData: input_bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let success = unsafe {
        CryptProtectData(&input, null(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
    };
    if success == 0 {
        bail!("Windows DPAPI could not encrypt the API credential.");
    }
    let encrypted = unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut c_void); }
    Ok(EncryptedCredential { kind: "dpapi".to_owned(), value: BASE64.encode(encrypted) })
}

#[cfg(not(windows))]
fn encrypt_credential(_secret: &str) -> Result<EncryptedCredential> {
    bail!("API credential encryption currently requires Windows DPAPI.")
}

#[cfg(windows)]
fn decrypt_credential(credential: &EncryptedCredential) -> Result<String> {
    use std::{ffi::c_void, ptr::{null, null_mut}, slice};
    use windows_sys::Win32::{
        Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN},
        System::Memory::LocalFree,
    };
    if credential.kind != "dpapi" {
        bail!("Unsupported API credential encryption type '{}'.", credential.kind);
    }
    let encrypted = BASE64.decode(&credential.value).context("Encrypted API credential is not valid base64")?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let success = unsafe {
        CryptUnprotectData(&input, null_mut(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
    };
    if success == 0 {
        bail!("Windows DPAPI could not decrypt the API credential for this Windows user.");
    }
    let plaintext = unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut c_void); }
    String::from_utf8(plaintext).context("Decrypted API credential is not valid UTF-8")
}

#[cfg(not(windows))]
fn decrypt_credential(_credential: &EncryptedCredential) -> Result<String> {
    bail!("API credential decryption currently requires Windows DPAPI.")
}

/// Decrypts the centrally stored credential after checking the application's authorization.
#[cfg(windows)]
pub fn get_secret_for_app(provider: &str, app: &str) -> Result<String> {
    validate_id(provider)?;
    validate_id(app)?;
    migrate_legacy_credentials()?;
    let registry = read_registry()?;
    let config = registry.providers.get(provider)
        .ok_or_else(|| anyhow!("API provider '{provider}' is not configured."))?;
    if !config.authorized_apps.contains(app) {
        bail!("Application '{app}' is not authorized to use API '{provider}'.");
    }
    let credential = config.credential.as_ref()
        .ok_or_else(|| anyhow!("API credential is missing for provider '{provider}'"))?;
    decrypt_credential(credential)
}

#[cfg(not(windows))]
pub fn get_secret_for_app(_provider: &str, _app: &str) -> Result<String> {
    bail!("API credential decryption currently requires Windows DPAPI.")
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

    #[test]
    fn legacy_registry_entries_without_credentials_remain_readable() {
        let json = serde_json::json!({
            "schema_version": 1,
            "providers": {
                "groq": {
                    "name": "Groq",
                    "endpoint": "https://api.groq.com/openai/v1",
                    "model": "llama-3.3-70b-versatile",
                    "authorized_apps": ["ttk-creator"],
                    "updated_at": 1
                }
            }
        });
        let registry: ApiRegistry = serde_json::from_value(json).unwrap();
        assert!(registry.providers["groq"].credential.is_none());
    }
}
