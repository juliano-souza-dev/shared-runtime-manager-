use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
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
const USER_AGENT: &str = "JulianoDev-SharedRuntimeManager/0.1";

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
        || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
        .with_context(|| format!("Registry is busy (lock: {}). If no manager is running, remove this stale lock file.", path.display()))?;
    Ok(RegistryLock { path, _file: file })
}

fn read_registry(base: &Path) -> Result<Registry> {
    let path = base.join(REGISTRY_FILE);
    if !path.exists() {
        return Ok(Registry { schema_version: 1, ..Registry::default() });
    }
    let bytes = fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?;
    let registry: Registry = serde_json::from_slice(&bytes).context("Registry file is invalid JSON")?;
    if registry.schema_version != 1 {
        bail!("Unsupported registry schema version: {}", registry.schema_version);
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
            registry.applications.insert(app_id.to_owned(), Application {
                name: name.trim().to_owned(),
                registered_at: timestamp(),
                resources: Vec::new(),
            });
        }
    }
    write_registry(&base, &registry)?;
    println!("Application registered: {app_id} ({})", name.trim());
    Ok(())
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
        other => bail!("Unknown resource '{other}'. Available resources: ffmpeg."),
    }
}

fn install_ffmpeg(app_id: Option<&str>) -> Result<()> {
    let base = ensure_layout()?;
    let _lock = lock_registry(&base)?;
    let mut registry = read_registry(&base)?;
    if let Some(id) = app_id {
        validate_id(id)?;
        if !registry.applications.contains_key(id) {
            bail!("Application '{id}' is not registered. Run: srm register {id} --name \"Application name\"");
        }
    }

    let version = "gyan-release-essentials";
    let destination = base.join("tools/ffmpeg").join(version);
    let ffmpeg = destination.join("ffmpeg.exe");
    let ffprobe = destination.join("ffprobe.exe");

    if !(ffmpeg.exists() && ffprobe.exists()) {
        let cache = base.join("cache/ffmpeg-release-essentials.zip");
        if !cache.exists() {
            println!("Downloading FFmpeg from the official Gyan.dev build endpoint...");
            download(FFMPEG_URL, &cache)?;
        }
        let staging = base.join("tools/ffmpeg").join(format!(".staging-{}", timestamp()));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        let extraction = extract_ffmpeg_binaries(&cache, &staging);
        if let Err(error) = extraction {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        fs::rename(&staging, &destination)?;
    }

    let mut files = BTreeMap::new();
    files.insert("ffmpeg.exe".to_owned(), sha256_file(&ffmpeg)?);
    files.insert("ffprobe.exe".to_owned(), sha256_file(&ffprobe)?);

    registry.resources.insert("ffmpeg".to_owned(), Resource {
        version: version.to_owned(),
        install_path: destination.to_string_lossy().into_owned(),
        installed_at: timestamp(),
        files,
        source: FFMPEG_URL.to_owned(),
    });
    if let Some(id) = app_id {
        let app = registry.applications.get_mut(id).expect("validated above");
        if !app.resources.iter().any(|item| item == "ffmpeg") {
            app.resources.push("ffmpeg".to_owned());
        }
    }
    write_registry(&base, &registry)?;
    println!("FFmpeg is ready: {}", destination.display());
    if let Some(id) = app_id {
        println!("Linked resource 'ffmpeg' to application '{id}'.");
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
    fs::rename(temporary, destination)?;
    Ok(())
}

fn extract_ffmpeg_binaries(archive: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive).context("Could not open the FFmpeg archive")?;
    let mut zip = zip::ZipArchive::new(file).context("Downloaded FFmpeg archive is not a valid ZIP")?;
    let mut found = 0;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(name) = entry.enclosed_name() else { continue };
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

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 { break; }
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
    let resource = registry.resources.get(resource_id)
        .ok_or_else(|| anyhow!("Resource '{resource_id}' is not installed."))?;

    if let Some(id) = app_id {
        validate_id(id)?;
        let app = registry.applications.get(id)
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
