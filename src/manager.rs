use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const REGISTRY_FILE: &str = "registry/state.json";
const FFMPEG_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
const SOX_URL: &str = "https://sourceforge.net/projects/sox/files/sox/14.4.2/sox-14.4.2-win32.zip/download";
const SOX_VERSION: &str = "14.4.2";
const SOX_SHA1: &str = "825b218c275687a38e96bf838dcfdd2e9bd55a25";
const USER_AGENT: &str = "JulianoDev-SharedRuntimeManager/0.2";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    schema_version: u32,
    applications: BTreeMap<String, Application>,
    resources: BTreeMap<String, Resource>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Application {
    name: String,
    registered_at: u64,
    #[serde(default)]
    installation_path: String,
    resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Resource {
    version: String,
    install_path: String,
    installed_at: u64,
    files: BTreeMap<String, String>,
    source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplicationInfo {
    pub id: String,
    pub name: String,
    pub registered_at: u64,
    pub installation_path: String,
    pub config_path: String,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceInfo {
    pub id: String,
    pub version: String,
    pub install_path: String,
    pub installed_at: u64,
}

struct RegistryLock {
    path: PathBuf,
    _file: File,
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn root() -> Result<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("LOCALAPPDATA is not defined")?;

    #[cfg(not(windows))]
    let base = dirs::data_local_dir().context("Could not determine the local data directory")?;

    Ok(base.join("JulianoDev").join("SharedRuntime"))
}

fn ensure_layout() -> Result<PathBuf> {
    let base = root()?;
    for directory in ["registry", "cache", "tools", "runtimes", "models"] {
        fs::create_dir_all(base.join(directory))?;
    }
    Ok(base)
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("Invalid identifier '{value}'. Use up to 64 letters, numbers, '-' or '_'.");
    }
    Ok(())
}

fn lock_registry(base: &Path) -> Result<RegistryLock> {
    let path = base.join("registry/manager.lock");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| {
            format!(
                "Registry is busy (lock: {}). If no manager is running, remove this stale lock file.",
                path.display()
            )
        })?;
    Ok(RegistryLock { path, _file: file })
}

fn read_registry(base: &Path) -> Result<Registry> {
    let path = base.join(REGISTRY_FILE);
    if !path.exists() {
        return Ok(Registry {
            schema_version: 1,
            ..Registry::default()
        });
    }
    let bytes = fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?;
    let registry: Registry =
        serde_json::from_slice(&bytes).context("Registry file is invalid JSON")?;
    if registry.schema_version != 1 {
        bail!(
            "Unsupported registry schema version: {}",
            registry.schema_version
        );
    }
    Ok(registry)
}

fn write_registry(base: &Path, registry: &Registry) -> Result<()> {
    let target = base.join(REGISTRY_FILE);
    let temporary = target.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(registry)?;
    {
        let mut file = File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    if target.exists() {
        fs::remove_file(&target)?;
    }
    fs::rename(&temporary, &target)?;
    Ok(())
}

pub fn register_app(app_id: &str, name: &str) -> Result<()> {
    validate_id(app_id)?;
    if name.trim().is_empty() {
        bail!("Application name cannot be empty.");
    }
    let base = ensure_layout()?;
    let _lock = lock_registry(&base)?;
    let mut registry = read_registry(&base)?;
    match registry.applications.get_mut(app_id) {
        Some(app) => app.name = name.trim().to_owned(),
        None => {
            registry.applications.insert(
                app_id.to_owned(),
                Application {
                    name: name.trim().to_owned(),
                    registered_at: timestamp(),
                    installation_path: String::new(),
                    resources: Vec::new(),
                },
            );
        }
    }
    write_registry(&base, &registry)?;
    println!("Application registered: {app_id} ({})", name.trim());
    Ok(())
}


/// Register an application and provision its root-level Shared configuration file.
pub fn register_app_at_path(app_id: &str, name: &str, installation_path: &str) -> Result<()> {
    validate_id(app_id)?;
    if name.trim().is_empty() {
        bail!("Application name cannot be empty.");
    }
    let requested_path = PathBuf::from(installation_path.trim());
    if !requested_path.is_absolute() {
        bail!("Application installation path must be absolute.");
    }
    if !requested_path.is_dir() {
        bail!("Application installation directory does not exist: {}", requested_path.display());
    }
    let canonical_path = requested_path.canonicalize()
        .with_context(|| format!("Could not resolve application directory: {}", requested_path.display()))?;
    let base = ensure_layout()?;
    let _lock = lock_registry(&base)?;
    let mut registry = read_registry(&base)?;
    let app = registry.applications.entry(app_id.to_owned()).or_insert_with(|| Application {
        name: name.trim().to_owned(),
        registered_at: timestamp(),
        installation_path: canonical_path.display().to_string(),
        resources: Vec::new(),
    });
    app.name = name.trim().to_owned();
    app.installation_path = canonical_path.display().to_string();
    write_registry(&base, &registry)?;
    write_application_config(&base, app_id, app)?;
    println!("Application registered and configured: {app_id} ({})", name.trim());
    println!("Configuration: {}", canonical_path.join("shared.config.json").display());
    Ok(())
}

fn write_application_config(shared_root: &Path, app_id: &str, app: &Application) -> Result<()> {
    if app.installation_path.is_empty() {
        return Ok(());
    }
    let app_root = PathBuf::from(&app.installation_path);
    if !app_root.is_dir() {
        bail!("Registered application directory is no longer available: {}", app_root.display());
    }
    let config_path = app_root.join("shared.config.json");
    let mut document: serde_json::Value = if config_path.exists() {
        let bytes = fs::read(&config_path)
            .with_context(|| format!("Could not read existing {}", config_path.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("Existing {} contains invalid JSON; refusing to overwrite it", config_path.display()))?
    } else {
        serde_json::json!({})
    };
    if !document.is_object() {
        bail!("{} must contain a JSON object at its root.", config_path.display());
    }

    let resource_registry = read_registry(shared_root)?;
    let installed_resources: BTreeMap<String, String> = resource_registry.resources.iter()
        .map(|(id, resource)| (id.clone(), resource.install_path.clone()))
        .collect();

    let mut application = document.get("application").cloned()
        .filter(|value| value.is_object()).unwrap_or_else(|| serde_json::json!({}));
    application["id"] = serde_json::Value::String(app_id.to_owned());
    application["name"] = serde_json::Value::String(app.name.clone());

    let shared = serde_json::json!({
        "enabled": true,
        "root": shared_root.display().to_string(),
        "directories": {
            "tools": shared_root.join("tools").display().to_string(),
            "runtimes": shared_root.join("runtimes").display().to_string(),
            "models": shared_root.join("models").display().to_string(),
            "cache": shared_root.join("cache").display().to_string(),
            "registry": shared_root.join("registry").display().to_string()
        },
        "files": {
            "applications": shared_root.join(REGISTRY_FILE).display().to_string(),
            "apiProviders": shared_root.join("registry/apis.json").display().to_string()
        },
        "resources": installed_resources
    });

    document["schemaVersion"] = serde_json::Value::String("1.0".to_owned());
    document["application"] = application;
    document["shared"] = shared;

    let temporary = config_path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&document)?;
    {
        let mut file = File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    if config_path.exists() {
        fs::remove_file(&config_path)?;
    }
    fs::rename(&temporary, &config_path)
        .with_context(|| format!("Could not replace {}", config_path.display()))?;
    Ok(())
}

pub fn applications() -> Result<Vec<ApplicationInfo>> {
    let base = ensure_layout()?;
    let registry = read_registry(&base)?;
    Ok(registry.applications.into_iter().map(|(id, app)| ApplicationInfo {
        id,
        name: app.name,
        registered_at: app.registered_at,
        installation_path: app.installation_path.clone(),
        config_path: if app.installation_path.is_empty() { String::new() } else { PathBuf::from(&app.installation_path).join("shared.config.json").display().to_string() },
        resources: app.resources,
    }).collect())
}

pub fn resources() -> Result<Vec<ResourceInfo>> {
    let base = ensure_layout()?;
    let registry = read_registry(&base)?;
    Ok(registry.resources.into_iter().map(|(id, resource)| ResourceInfo {
        id,
        version: resource.version,
        install_path: resource.install_path,
        installed_at: resource.installed_at,
    }).collect())
}

pub fn list_apps() -> Result<()> {
    let base = ensure_layout()?;
    let registry = read_registry(&base)?;
    if registry.applications.is_empty() {
        println!("No applications registered yet.");
    }
    for (id, app) in registry.applications {
        println!("{id}\t{}\t{} resource(s)", app.name, app.resources.len());
    }
    Ok(())
}

pub fn install(resource: &str, app_id: Option<&str>) -> Result<()> {
    match resource {
        "ffmpeg" => install_ffmpeg(app_id),
        "sox" => install_sox(app_id),
        other => bail!("Unknown resource '{other}'. Available resources: ffmpeg, sox."),
    }
}

fn validate_app_link(registry: &Registry, app_id: Option<&str>) -> Result<()> {
    if let Some(id) = app_id {
        validate_id(id)?;
        if !registry.applications.contains_key(id) {
            bail!("Application '{id}' is not registered. Run: srm register {id} --name \"Application name\"");
        }
    }
    Ok(())
}

fn link_resource(registry: &mut Registry, resource_id: &str, app_id: Option<&str>) {
    if let Some(id) = app_id {
        let app = registry.applications.get_mut(id).expect("validated above");
        if !app.resources.iter().any(|item| item == resource_id) {
            app.resources.push(resource_id.to_owned());
        }
    }
}

fn install_ffmpeg(app_id: Option<&str>) -> Result<()> {
    let base = ensure_layout()?;
    let _lock = lock_registry(&base)?;
    let mut registry = read_registry(&base)?;
    validate_app_link(&registry, app_id)?;

    let version = "gyan-release-essentials";
    let destination = base.join("tools/ffmpeg").join(version);
    let ffmpeg = destination.join("ffmpeg.exe");
    let ffprobe = destination.join("ffprobe.exe");

    if !(ffmpeg.exists() && ffprobe.exists()) {
        let cache = base.join("cache/ffmpeg-release-essentials.zip");
        if !cache.exists() {
            println!("Downloading FFmpeg from Gyan.dev...");
            download(FFMPEG_URL, &cache)?;
        }
        let staging = base
            .join("tools/ffmpeg")
            .join(format!(".staging-{}", timestamp()));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        let extraction = extract_ffmpeg_binaries(&cache, &staging);
        if let Err(error) = extraction {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        replace_directory(&staging, &destination)?;
    }

    let mut files = BTreeMap::new();
    files.insert("ffmpeg.exe".to_owned(), sha256_file(&ffmpeg)?);
    files.insert("ffprobe.exe".to_owned(), sha256_file(&ffprobe)?);

    registry.resources.insert(
        "ffmpeg".to_owned(),
        Resource {
            version: version.to_owned(),
            install_path: destination.to_string_lossy().into_owned(),
            installed_at: timestamp(),
            files,
            source: FFMPEG_URL.to_owned(),
        },
    );
    link_resource(&mut registry, "ffmpeg", app_id);
    write_registry(&base, &registry)?;
    println!("FFmpeg is ready: {}", destination.display());
    if let Some(id) = app_id {
        println!("Linked resource 'ffmpeg' to application '{id}'.");
    }
    Ok(())
}

fn install_sox(app_id: Option<&str>) -> Result<()> {
    #[cfg(not(windows))]
    {
        let _ = app_id;
        bail!("The current SoX package is a Windows x86 build. This command is only available on Windows.");
    }

    #[cfg(windows)]
    {
        let base = ensure_layout()?;
        let _lock = lock_registry(&base)?;
        let mut registry = read_registry(&base)?;
        validate_app_link(&registry, app_id)?;

        let cache = base.join("cache/sox-14.4.2-win32.zip");
        if !cache.exists() {
            println!("Downloading SoX {SOX_VERSION} from SourceForge...");
            download(SOX_URL, &cache)?;
        }
        let archive_hash = sha1_file(&cache)?;
        if archive_hash != SOX_SHA1 {
            let _ = fs::remove_file(&cache);
            bail!("SoX archive checksum mismatch. The cached download was removed; refusing to install it.");
        }

        let destination = base.join("tools/sox").join(SOX_VERSION);
        let staging = base
            .join("tools/sox")
            .join(format!(".staging-{}", timestamp()));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        if let Err(error) = extract_zip_safely(&cache, &staging) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        replace_directory(&staging, &destination)?;

        let executable = find_file(&destination, "sox.exe")?
            .ok_or_else(|| anyhow!("The SoX archive did not contain sox.exe."))?;
        let install_path = executable
            .parent()
            .ok_or_else(|| anyhow!("Could not determine the SoX installation directory."))?;
        let files = collect_hashes(install_path)?;
        let relative_install_path = install_path.strip_prefix(&destination)
            .context("SoX executable resolved outside its installation directory")?;
        let registered_path = if relative_install_path.as_os_str().is_empty() {
            destination.clone()
        } else {
            destination.join(relative_install_path)
        };

        registry.resources.insert(
            "sox".to_owned(),
            Resource {
                version: SOX_VERSION.to_owned(),
                install_path: registered_path.to_string_lossy().into_owned(),
                installed_at: timestamp(),
                files,
                source: SOX_URL.to_owned(),
            },
        );
        link_resource(&mut registry, "sox", app_id);
        write_registry(&base, &registry)?;
        println!("SoX {SOX_VERSION} is ready: {}", registered_path.display());
        if let Some(id) = app_id {
            println!("Linked resource 'sox' to application '{id}'.");
        }
        Ok(())
    }
}

fn replace_directory(staging: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        let backup = destination.with_extension(format!("backup-{}", timestamp()));
        fs::rename(destination, &backup)
            .with_context(|| format!("Could not move existing installation to {}", backup.display()))?;
        if let Err(error) = fs::rename(staging, destination) {
            let _ = fs::rename(&backup, destination);
            return Err(error.into());
        }
        fs::remove_dir_all(backup)?;
    } else {
        fs::rename(staging, destination)?;
    }
    Ok(())
}

fn download(url: &str, destination: &Path) -> Result<()> {
    let response = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()?
        .get(url)
        .send()
        .context("Could not connect to the resource download server")?
        .error_for_status()
        .context("The resource server returned an unsuccessful status")?;
    let temporary = destination.with_extension("download");
    let mut output = File::create(&temporary)?;
    let mut response = response;
    std::io::copy(&mut response, &mut output)?;
    output.sync_all()?;
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(temporary, destination)?;
    Ok(())
}

fn extract_ffmpeg_binaries(archive: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive).context("Could not open the FFmpeg archive")?;
    let mut zip =
        zip::ZipArchive::new(file).context("Downloaded FFmpeg archive is not a valid ZIP")?;
    let mut found = 0;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let filename = name.file_name().and_then(|v| v.to_str()).unwrap_or("");
        if (filename == "ffmpeg.exe" || filename == "ffprobe.exe")
            && name.components().any(|part| part.as_os_str() == "bin")
        {
            let target = destination.join(filename);
            let mut output = File::create(target)?;
            std::io::copy(&mut entry, &mut output)?;
            found += 1;
        }
    }
    if found != 2 {
        bail!("The archive did not contain both ffmpeg.exe and ffprobe.exe in a bin folder.");
    }
    Ok(())
}

fn extract_zip_safely(archive: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive).context("Could not open the SoX archive")?;
    let mut zip =
        zip::ZipArchive::new(file).context("Downloaded SoX archive is not a valid ZIP")?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            continue;
        }
        let target = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = File::create(target)?;
        std::io::copy(&mut entry, &mut output)?;
    }
    Ok(())
}

fn find_file(directory: &Path, filename: &str) -> Result<Option<PathBuf>> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, filename)? {
                return Ok(Some(found));
            }
        } else if path.file_name().and_then(|name| name.to_str()) == Some(filename) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn collect_hashes(directory: &Path) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    collect_hashes_recursive(directory, directory, &mut hashes)?;
    Ok(hashes)
}

fn collect_hashes_recursive(
    root: &Path,
    directory: &Path,
    hashes: &mut BTreeMap<String, String>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_hashes_recursive(root, &path, hashes)?;
        } else if path.is_file() {
            let relative = path.strip_prefix(root)?;
            hashes.insert(
                relative.to_string_lossy().into_owned(),
                sha256_file(&path)?,
            );
        }
    }
    Ok(())
}

fn sha1_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn list_resources() -> Result<()> {
    let base = ensure_layout()?;
    let registry = read_registry(&base)?;
    if registry.resources.is_empty() {
        println!("No shared resources installed yet.");
    }
    for (id, resource) in registry.resources {
        println!("{id}\t{}\t{}", resource.version, resource.install_path);
    }
    Ok(())
}

pub fn resolve(resource_id: &str, app_id: Option<&str>) -> Result<()> {
    validate_id(resource_id)?;
    let base = ensure_layout()?;
    let registry = read_registry(&base)?;
    let resource = registry
        .resources
        .get(resource_id)
        .ok_or_else(|| anyhow!("Resource '{resource_id}' is not installed."))?;

    if let Some(id) = app_id {
        validate_id(id)?;
        let app = registry
            .applications
            .get(id)
            .ok_or_else(|| anyhow!("Application '{id}' is not registered."))?;
        if !app.resources.iter().any(|item| item == resource_id) {
            bail!("Application '{id}' is not linked to resource '{resource_id}'.");
        }
    }

    let path = PathBuf::from(&resource.install_path);
    if !path.exists() {
        bail!("Registry points to a missing installation: {}", path.display());
    }
    for (filename, expected_hash) in &resource.files {
        let file = path.join(filename);
        if !file.exists() || sha256_file(&file)? != *expected_hash {
            bail!("Integrity check failed for {}. Reinstall the resource.", file.display());
        }
    }
    println!("{}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_safe_ids() {
        assert!(validate_id("ttk-media").is_ok());
        assert!(validate_id("nova_generator2").is_ok());
    }

    #[test]
    fn rejects_unsafe_ids() {
        assert!(validate_id("").is_err());
        assert!(validate_id("../outside").is_err());
        assert!(validate_id("app name").is_err());
        assert!(validate_id(&"a".repeat(65)).is_err());
    }

    #[test]
    fn registry_round_trips_json() {
        let mut registry = Registry {
            schema_version: 1,
            ..Registry::default()
        };
        registry.applications.insert(
            "ttk-media".to_owned(),
            Application {
                name: "TTK Media".to_owned(),
                registered_at: 123,
                resources: vec!["sox".to_owned()],
            },
        );
        let encoded = serde_json::to_vec(&registry).unwrap();
        let decoded: Registry = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.applications["ttk-media"].resources, vec!["sox"]);
    }
}
