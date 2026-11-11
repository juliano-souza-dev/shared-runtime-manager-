use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const USER_AGENT: &str = "JulianoDev-SharedRuntimeManager/0.5";
const FFMPEG_ARCHIVE: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
const YT_DLP_WINDOWS: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const YT_DLP_LINUX: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";

const DEFAULT_DEPENDENCIES: &[(&str, bool)] = &[
    ("ffmpeg", true),
    ("ffprobe", true),
    ("yt-dlp", true),
    ("python", true),
    ("faster-whisper", true),
    ("pyav", true),
    ("whisper-models", true),
];

fn shared_root() -> Result<PathBuf> {
    super::manager::root()
}

fn download(url: &str, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let response = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()?
        .get(url)
        .send()
        .with_context(|| format!("Could not download dependency from {url}"))?
        .error_for_status()?;
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

fn command_output(command: &str, argument: &str) -> Option<String> {
    #[cfg(windows)]
    let output = Command::new("where").arg(command).output().ok()?;
    #[cfg(not(windows))]
    let output = Command::new("which").arg(command).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let found = text.lines().next()?.trim().to_owned();
    if found.is_empty() {
        None
    } else if argument == "path" {
        Some(found)
    } else {
        Some(found)
    }
}

fn command_exists(command: &str) -> bool {
    command_output(command, "path").is_some()
}

fn find_existing_binary(directory: &Path, filename: &str) -> Result<Option<PathBuf>> {
    if !directory.is_dir() {
        return Ok(None);
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|value| value.to_str()).unwrap_or("");
        if name.starts_with(".staging") || name.contains(".backup-") {
            continue;
        }
        if path.is_dir() {
            if let Some(found) = find_existing_binary(&path, filename)? {
                return Ok(Some(found));
            }
        } else if name.eq_ignore_ascii_case(filename) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn find_python() -> Option<String> {
    for candidate in ["python", "python3", "py"] {
        if command_exists(candidate) {
            let check = if candidate == "py" {
                Command::new(candidate).args(["-3", "--version"]).output()
            } else {
                Command::new(candidate).arg("--version").output()
            };
            if check.map(|out| out.status.success()).unwrap_or(false) {
                return Some(candidate.to_owned());
            }
        }
    }
    None
}

fn ensure_python() -> Result<String> {
    if let Some(python) = find_python() {
        return Ok(python);
    }

    #[cfg(windows)]
    {
        if command_exists("winget") {
            println!("Python was not found. Installing Python 3.12 through Windows Package Manager...");
            let status = Command::new("winget")
                .args([
                    "install", "--id", "Python.Python.3.12", "--exact",
                    "--silent", "--accept-package-agreements", "--accept-source-agreements",
                ])
                .status()
                .context("Could not start winget to install Python")?;
            if !status.success() {
                bail!("Python installation through winget failed. Install Python and run registration again.");
            }
            if let Some(python) = find_python() {
                return Ok(python);
            }
        }
    }
    bail!("Python was not found and could not be provisioned automatically. Install Python 3 and run registration again.")
}

fn run_python(python: &str, args: &[&str]) -> Result<()> {
    let status = if python == "py" {
        Command::new(python).arg("-3").args(args).status()?
    } else {
        Command::new(python).args(args).status()?
    };
    if !status.success() {
        bail!("Python dependency command failed: {}", args.join(" "));
    }
    Ok(())
}

fn python_import_exists(python: &str, module: &str) -> bool {
    let script = format!("import {module}");
    let output = if python == "py" {
        Command::new(python).args(["-3", "-c", &script]).output()
    } else {
        Command::new(python).args(["-c", &script]).output()
    };
    output.map(|result| result.status.success()).unwrap_or(false)
}

fn ensure_python_package(python: &str, module: &str, package: &str) -> Result<()> {
    if python_import_exists(python, module) {
        println!("{package} is already available; keeping the installed version.");
        return Ok(());
    }
    println!("Installing missing Python package: {package}");
    run_python(python, &["-m", "pip", "install", package])
        .with_context(|| format!("Could not install missing Python package {package}"))
}

fn extract_ffmpeg_component(archive: &Path, destination: &Path, executable: &str) -> Result<bool> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).context("FFmpeg download is not a valid ZIP archive")?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(name) = entry.enclosed_name() else { continue };
        if name.file_name().and_then(|value| value.to_str()) == Some(executable)
            && name.components().any(|part| part.as_os_str() == "bin")
        {
            fs::create_dir_all(destination)?;
            let target = destination.join(executable);
            let mut output = File::create(target)?;
            std::io::copy(&mut entry, &mut output)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_ffmpeg_component(root: &Path, id: &str, executable: &str) -> Result<PathBuf> {
    let command = executable.trim_end_matches(".exe");
    if let Some(found) = command_output(command, "path") {
        let path = PathBuf::from(found);
        println!("{id} is already available on PATH; keeping the existing installation.");
        return Ok(path.parent().unwrap_or(Path::new(".")).to_path_buf());
    }

    if let Some(existing) = find_existing_binary(&root.join("tools"), executable)? {
        println!("{id} already exists in the Shared tools directory; keeping the existing installation.");
        return Ok(existing.parent().unwrap_or(Path::new(".")).to_path_buf());
    }

    let directory = root.join("tools").join(id);
    let target = directory.join(executable);
    if target.is_file() {
        println!("{id} already exists; keeping the existing installation.");
        return Ok(directory);
    }

    #[cfg(windows)]
    {
        let archive = root.join("cache/ffmpeg-release-essentials.zip");
        if !archive.is_file() {
            println!("Downloading the FFmpeg package (it may contain FFmpeg and FFprobe)...");
            download(FFMPEG_ARCHIVE, &archive)?;
        }
        if !extract_ffmpeg_component(&archive, &directory, executable)? {
            bail!("The downloaded FFmpeg archive does not contain {executable}.");
        }
        return Ok(directory);
    }

    #[cfg(not(windows))]
    {
        let _ = (root, id, executable, target, FFMPEG_ARCHIVE);
        bail!("{id} is not installed. Install it with your operating system package manager, then register the application again.")
    }
}

fn ensure_yt_dlp(root: &Path) -> Result<PathBuf> {
    let command = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    if let Some(found) = command_output(command, "path") {
        let path = PathBuf::from(found);
        println!("yt-dlp is already available on PATH; keeping the existing installation.");
        return Ok(path.parent().unwrap_or(Path::new(".")).to_path_buf());
    }
    let directory = root.join("tools/yt-dlp");
    let executable = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    if let Some(existing) = find_existing_binary(&root.join("tools"), executable)? {
        println!("yt-dlp already exists in the Shared tools directory; keeping the existing installation.");
        return Ok(existing.parent().unwrap_or(Path::new(".")).to_path_buf());
    }
    let target = directory.join(executable);
    if target.is_file() {
        println!("yt-dlp already exists; keeping the existing installation.");
        return Ok(directory);
    }
    let url = if cfg!(windows) { YT_DLP_WINDOWS } else { YT_DLP_LINUX };
    download(url, &target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
    }
    Ok(directory)
}

fn ensure_dependency(root: &Path, id: &str) -> Result<PathBuf> {
    match id {
        "ffmpeg" => ensure_ffmpeg_component(root, "ffmpeg", if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }),
        "ffprobe" => ensure_ffmpeg_component(root, "ffprobe", if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" }),
        "yt-dlp" => ensure_yt_dlp(root),
        "python" => {
            let python = ensure_python()?;
            Ok(PathBuf::from(python))
        }
        "faster-whisper" | "pyav" => {
            let python = ensure_python()?;
            if id == "faster-whisper" {
                ensure_python_package(&python, "faster_whisper", "faster-whisper")?;
            } else {
                ensure_python_package(&python, "av", "av")?;
            }
            Ok(PathBuf::from(python))
        }
        "whisper-models" => {
            let directory = root.join("models/whisper");
            fs::create_dir_all(&directory)?;
            Ok(directory)
        }
        other => bail!("Unknown dependency '{other}'. No installer is registered in the Shared catalog."),
    }
}

fn default_contract() -> Value {
    json!({
        "dependencies": DEFAULT_DEPENDENCIES.iter().map(|(id, required)| {
            json!({ "id": id, "required": required })
        }).collect::<Vec<_>>()
    })
}

fn atomic_write(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

/// Reads the application's dependency contract and provisions only missing resources.
/// Existing resources are never version-compared or replaced.
pub fn provision_application(app_id: &str, installation_path: &str) -> Result<()> {
    let app_root = PathBuf::from(installation_path);
    let config_path = app_root.join("shared.config.json");
    if !config_path.is_file() {
        bail!("Shared configuration file was not created: {}", config_path.display());
    }

    let mut config: Value = serde_json::from_slice(&fs::read(&config_path)?)
        .context("shared.config.json contains invalid JSON")?;
    if !config.is_object() {
        bail!("shared.config.json root must be a JSON object.");
    }

    let declared = config.get("dependencies")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| config.get("shared")
            .and_then(|shared| shared.get("dependencies"))
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
            .cloned())
        .unwrap_or_else(|| default_contract()["dependencies"].as_array().unwrap().clone());

    // Keep the public root contract and the Shared-owned declaration section in sync.
    config["dependencies"] = Value::Array(declared.clone());
    config["shared"]["dependencies"] = Value::Array(declared.clone());
    let dependencies = declared;

    let root = shared_root()?;
    for directory in ["tools", "runtimes", "models", "cache", "registry"] {
        fs::create_dir_all(root.join(directory))?;
    }

    let mut resolved = BTreeMap::<String, String>::new();
    for dependency in dependencies {
        let id = dependency.get("id").and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Every dependency must contain a string id."))?;
        let required = dependency.get("required").and_then(Value::as_bool).unwrap_or(false);
        if !required {
            continue;
        }
        let path = ensure_dependency(&root, id)
            .with_context(|| format!("Could not provision required dependency '{id}'"))?;
        resolved.insert(id.to_owned(), path.to_string_lossy().into_owned());
    }

    // Application-owned data is deliberately kept outside the shared resource directories.
    let data_root = app_root.join("data");
    let mut app_data = BTreeMap::<String, String>::new();
    for name in ["projects", "database", "sounds", "logs", "user-files"] {
        let directory = data_root.join(name);
        fs::create_dir_all(&directory)?;
        app_data.insert(name.to_owned(), directory.to_string_lossy().into_owned());
    }

    config["shared"]["resolvedDependencies"] = serde_json::to_value(resolved)?;
    config["application"]["data"] = serde_json::to_value(app_data)?;
    atomic_write(&config_path, &config)?;

    println!("Dependencies provisioned for application '{app_id}'.");
    println!("Shared configuration updated: {}", config_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_contract_has_independent_ffmpeg_components() {
        let dependencies = default_contract()["dependencies"].as_array().unwrap().clone();
        let ids: Vec<&str> = dependencies.iter().filter_map(|item| item["id"].as_str()).collect();
        assert!(ids.contains(&"ffmpeg"));
        assert!(ids.contains(&"ffprobe"));
        assert_ne!(ids.iter().position(|id| *id == "ffmpeg"), ids.iter().position(|id| *id == "ffprobe"));
    }

    #[test]
    fn default_contract_contains_requested_python_dependencies() {
        let dependencies = default_contract()["dependencies"].as_array().unwrap().clone();
        let ids: Vec<&str> = dependencies.iter().filter_map(|item| item["id"].as_str()).collect();
        for id in ["yt-dlp", "python", "faster-whisper", "pyav", "whisper-models"] {
            assert!(ids.contains(&id));
        }
    }
}
