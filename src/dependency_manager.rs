use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const USER_AGENT: &str = "JulianoDev-SharedRuntimeManager/0.5";
const FFMPEG_ARCHIVE: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
const YT_DLP_WINDOWS: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const YT_DLP_LINUX: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";


/// Spawn dependency tools without opening a separate Windows console window.
/// All stdout/stderr handling stays inside the Shared Runtime Manager.
fn silent_command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command
}

const DEFAULT_DEPENDENCIES: &[(&str, bool)] = &[
    ("piper-tts", false),
    ("qwen3-tts", false),
    ("ffmpeg", true),
    ("ffprobe", true),
    ("yt-dlp", true),
    ("python", true),
    ("faster-whisper", true),
    ("pyav", true),
    ("whisper-models", true),
    ("forced-alignment", false),
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
    let output = silent_command("where").arg(command).output().ok()?;
    #[cfg(not(windows))]
    let output = silent_command("which").arg(command).output().ok()?;
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
                silent_command(candidate).args(["-3", "--version"]).output()
            } else {
                silent_command(candidate).arg("--version").output()
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
            let status = silent_command("winget")
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
        silent_command(python).arg("-3").args(args).status()?
    } else {
        silent_command(python).args(args).status()?
    };
    if !status.success() {
        bail!("Python dependency command failed: {}", args.join(" "));
    }
    Ok(())
}

fn python_import_exists(python: &str, module: &str) -> bool {
    let script = format!("import {module}");
    let output = if python == "py" {
        silent_command(python).args(["-3", "-c", &script]).output()
    } else {
        silent_command(python).args(["-c", &script]).output()
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

fn python_command(python: &str, args: &[&str]) -> Result<()> {
    let status = if python == "py" {
        silent_command(python).arg("-3").args(args).status()?
    } else {
        silent_command(python).args(args).status()?
    };
    if !status.success() {
        bail!("Python command failed: {}", args.join(" "));
    }
    Ok(())
}

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

fn python_module_exists(python: &Path, module: &str) -> bool {
    let script = format!("import {module}");
    silent_command(python)
        .arg("-c")
        .arg(script)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn python_script_succeeds(python: &Path, script: &str) -> bool {
    silent_command(python)
        .arg("-c")
        .arg(script)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Provisions WhisperX in a Shared-owned virtual environment. The application's
/// own Python installation and site-packages are never modified.
const QWEN3_TTS_MODEL_ID: &str = "Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice";
const PIPER_DEFAULT_VOICE: &str = "en_US-mike-medium";

fn run_venv_command(python: &Path, args: &[&str], description: &str) -> Result<()> {
    let output = silent_command(python)
        .args(args)
        .output()
        .with_context(|| format!("Could not start {description}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() { stderr.trim() } else { stdout.trim() };
        let detail: String = detail.chars().rev().take(4000).collect::<String>().chars().rev().collect();
        bail!("{description} failed: {detail}");
    }
    Ok(())
}

fn ensure_shared_venv(runtime: &Path) -> Result<PathBuf> {
    let venv = runtime.join("venv");
    let interpreter = venv_python(&venv);
    if !interpreter.is_file() {
        let base_python = ensure_python()?;
        let venv_path = venv.to_string_lossy().into_owned();
        if base_python == "py" {
            python_command(&base_python, &["-3", "-m", "venv", &venv_path])?;
        } else {
            python_command(&base_python, &["-m", "venv", &venv_path])?;
        }
    }
    if !interpreter.is_file() {
        bail!("The isolated Shared Python environment was not created: {}", interpreter.display());
    }
    Ok(interpreter)
}

fn write_runtime_manifest(runtime: &Path, manifest: &Value) -> Result<()> {
    let path = runtime.join("runtime.json");
    fs::write(&path, serde_json::to_vec_pretty(manifest)?)
        .with_context(|| format!("Could not write runtime manifest {}", path.display()))?;
    Ok(())
}

fn ensure_piper_tts(root: &Path) -> Result<PathBuf> {
    let runtime = root.join("runtimes/piper-tts");
    let voices = root.join("models/piper-tts");
    fs::create_dir_all(&runtime)?;
    fs::create_dir_all(&voices)?;
    super::dependency_queue::report_install_progress(5, "Preparando o ambiente isolado do Piper TTS")?;

    let interpreter = ensure_shared_venv(&runtime)?;
    if !python_module_exists(&interpreter, "piper") {
        super::dependency_queue::report_install_progress(25, "Instalando Piper TTS e o mecanismo ONNX para CPU")?;
        run_venv_command(
            &interpreter,
            &["-m", "pip", "install", "--upgrade", "pip"],
            "Piper pip preparation",
        )?;
        run_venv_command(
            &interpreter,
            &["-m", "pip", "install", "piper-tts>=1.5"],
            "Piper TTS installation",
        )?;
    }

    let model = voices.join(format!("{PIPER_DEFAULT_VOICE}.onnx"));
    let model_config = voices.join(format!("{PIPER_DEFAULT_VOICE}.onnx.json"));
    if !(model.is_file() && model_config.is_file()) {
        super::dependency_queue::report_install_progress(65, "Baixando a voz inglesa padrão do Piper")?;
        let voice_dir = voices.to_string_lossy().into_owned();
        run_venv_command(
            &interpreter,
            &["-m", "piper.download_voices", "--data-dir", &voice_dir, PIPER_DEFAULT_VOICE],
            "Piper English voice download",
        )?;
    }

    super::dependency_queue::report_install_progress(90, "Preparando o executor de síntese Piper")?;
    let runner = runtime.join("runner.py");
    let source = include_str!("../assets/piper_tts/runner.py");
    if !runner.is_file() || fs::read_to_string(&runner).ok().as_deref() != Some(source) {
        fs::write(&runner, source)?;
    }
    if !python_module_exists(&interpreter, "onnxruntime") || !(model.is_file() && model_config.is_file()) {
        bail!("Piper TTS did not pass validation. Check the isolated Python environment and voice files.");
    }
    let providers_script = "import onnxruntime as ort; providers = ort.get_available_providers(); assert 'CPUExecutionProvider' in providers; assert 'CUDAExecutionProvider' not in providers";
    run_venv_command(&interpreter, &["-c", providers_script], "Piper CPU provider validation")?;

    write_runtime_manifest(&runtime, &json!({
        "schemaVersion": "1.0",
        "runtime": "piper-tts",
        "python": interpreter,
        "runner": runner,
        "device": "cpu",
        "voice": PIPER_DEFAULT_VOICE,
        "model": model,
        "modelConfig": model_config,
        "language": "en"
    }))?;
    super::dependency_queue::report_install_progress(100, "Piper TTS pronto para uso em CPU")?;
    println!("Piper TTS CPU runtime is ready: {}", runtime.display());
    Ok(runtime)
}

fn ensure_qwen3_tts(root: &Path) -> Result<PathBuf> {
    let runtime = root.join("runtimes/qwen3-tts");
    let model = root.join("models/qwen3-tts/0.6B-CustomVoice");
    let cache = root.join("cache/huggingface");
    fs::create_dir_all(&runtime)?;
    fs::create_dir_all(&model)?;
    fs::create_dir_all(&cache)?;
    super::dependency_queue::report_install_progress(5, "Preparando o ambiente isolado do Qwen3-TTS 0.6B")?;

    let interpreter = ensure_shared_venv(root, &runtime)?;
    let cpu_torch_check = "import torch; assert torch.version.cuda is None; assert torch.cuda.is_available() is False";
    if !python_script_succeeds(&interpreter, cpu_torch_check) {
        super::dependency_queue::report_install_progress(20, "Instalando PyTorch exclusivamente para CPU, sem CUDA")?;
        run_venv_command(
            &interpreter,
            &["-m", "pip", "install", "--upgrade", "pip"],
            "Qwen3-TTS pip preparation",
        )?;
        run_venv_command(
            &interpreter,
            &[
                "-m", "pip", "install", "--force-reinstall", "torch", "torchaudio",
                "--index-url", "https://download.pytorch.org/whl/cpu",
            ],
            "PyTorch CPU installation",
        )?;
    }

    if !python_module_exists(&interpreter, "qwen_tts") {
        super::dependency_queue::report_install_progress(45, "Instalando Qwen3-TTS e dependências de síntese")?;
        run_venv_command(
            &interpreter,
            &["-m", "pip", "install", "qwen-tts"],
            "Qwen3-TTS package installation",
        )?;
    }

    // Re-check after package installation because pip may have changed the Torch build.
    if !python_script_succeeds(&interpreter, cpu_torch_check) {
        super::dependency_queue::report_install_progress(60, "Restaurando PyTorch CPU após resolver as dependências do Qwen3-TTS")?;
        run_venv_command(
            &interpreter,
            &[
                "-m", "pip", "install", "--force-reinstall", "torch", "torchaudio",
                "--index-url", "https://download.pytorch.org/whl/cpu",
            ],
            "PyTorch CPU-only restoration",
        )?;
    }

    let model_config = model.join("config.json");
    let model_weights = model.join("model.safetensors");
    if !(model_config.is_file() && model_weights.is_file()) {
        super::dependency_queue::report_install_progress(70, "Baixando o modelo Qwen3-TTS 0.6B CustomVoice. O download pode ultrapassar 2 GB.")?;
        let model_path = model.to_string_lossy().into_owned();
        let cache_path = cache.to_string_lossy().into_owned();
        let script = "import sys; from huggingface_hub import snapshot_download; snapshot_download(repo_id=sys.argv[1], local_dir=sys.argv[2], cache_dir=sys.argv[3])";
        let output = silent_command(&interpreter)
            .arg("-c")
            .arg(script)
            .arg(QWEN3_TTS_MODEL_ID)
            .arg(&model_path)
            .arg(&cache_path)
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .output()
            .context("Could not start the Qwen3-TTS model download")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if !stderr.trim().is_empty() { stderr.trim() } else { stdout.trim() };
            let detail: String = detail.chars().rev().take(4000).collect::<String>().chars().rev().collect();
            bail!("Qwen3-TTS model download failed: {detail}");
        }
    }

    super::dependency_queue::report_install_progress(92, "Preparando e validando o executor Qwen3-TTS para CPU")?;
    let runner = runtime.join("runner.py");
    let source = include_str!("../assets/qwen3_tts/runner.py");
    if !runner.is_file() || fs::read_to_string(&runner).ok().as_deref() != Some(source) {
        fs::write(&runner, source)?;
    }
    if !python_module_exists(&interpreter, "qwen_tts")
        || !python_module_exists(&interpreter, "soundfile")
        || !(model_config.is_file() && model_weights.is_file())
    {
        bail!("Qwen3-TTS did not pass validation. Check the isolated Python environment and model files.");
    }
    let cpu_script = "import torch; assert torch.version.cuda is None, 'CUDA-enabled PyTorch detected'; assert torch.cuda.is_available() is False";
    run_venv_command(&interpreter, &["-c", cpu_script], "Qwen3-TTS CPU-only validation")?;

    write_runtime_manifest(&runtime, &json!({
        "schemaVersion": "1.0",
        "runtime": "qwen3-tts",
        "python": interpreter,
        "runner": runner,
        "device": "cpu",
        "dtype": "float32",
        "modelId": QWEN3_TTS_MODEL_ID,
        "model": model,
        "defaultLanguage": "English",
        "defaultSpeaker": "Aiden",
        "supportsVoiceCloning": false,
        "supportsCustomVoice": true
    }))?;
    super::dependency_queue::report_install_progress(100, "Qwen3-TTS 0.6B pronto para uso em CPU")?;
    println!("Qwen3-TTS 0.6B CPU runtime is ready: {}", runtime.display());
    Ok(runtime)
}

fn ensure_forced_alignment(root: &Path) -> Result<PathBuf> {
    let runtime = root.join("runtimes/forced-alignment");
    let venv = runtime.join("venv");
    let interpreter = venv_python(&venv);
    fs::create_dir_all(&runtime)?;
    fs::create_dir_all(root.join("models/forced-alignment"))?;

    super::dependency_queue::report_install_progress(5, "Preparando o ambiente isolado do WhisperX")?;
    if !interpreter.is_file() {
        super::dependency_queue::report_install_progress(10, "Localizando Python para criar o ambiente do WhisperX")?;
        let base_python = ensure_python()?;
        super::dependency_queue::report_install_progress(20, "Criando ambiente virtual exclusivo do Shared Runtime")?;
        let venv_path = venv.to_string_lossy().into_owned();
        if base_python == "py" {
            python_command(&base_python, &["-3", "-m", "venv", &venv_path])?;
        } else {
            python_command(&base_python, &["-m", "venv", &venv_path])?;
        }
    }

    super::dependency_queue::report_install_progress(35, "Verificando se WhisperX já está instalado")?;
    if !python_module_exists(&interpreter, "whisperx") {
        super::dependency_queue::report_install_progress(40, "Instalando WhisperX, PyTorch e dependências. Esta etapa pode demorar.")?;
        let output = silent_command(&interpreter)
            .args(["-m", "pip", "install", "--upgrade", "pip"])
            .output()
            .context("Não foi possível atualizar o pip no ambiente isolado do WhisperX")?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            bail!("Falha ao preparar o pip do WhisperX: {}", detail.trim());
        }
        super::dependency_queue::report_install_progress(50, "Baixando e instalando WhisperX e dependências de alinhamento")?;
        let output = silent_command(&interpreter)
            .args(["-m", "pip", "install", "whisperx"])
            .output()
            .context("Não foi possível iniciar o pip no ambiente isolado do Shared Runtime")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if !stderr.trim().is_empty() { stderr.trim() } else { stdout.trim() };
            let detail: String = detail.chars().rev().take(3500).collect::<String>().chars().rev().collect();
            bail!("A instalação do WhisperX falhou. Detalhes do pip: {detail}");
        }
    }

    super::dependency_queue::report_install_progress(90, "Preparando o executor de alinhamento do Nova Generator")?;
    let runner = runtime.join("runner.py");
    let runner_source = include_str!("../assets/forced_alignment/runner.py");
    if !runner.is_file() || fs::read_to_string(&runner).ok().as_deref() != Some(runner_source) {
        fs::write(&runner, runner_source)
            .with_context(|| format!("Não foi possível preparar {}", runner.display()))?;
    }

    super::dependency_queue::report_install_progress(98, "Validando o runtime compartilhado do WhisperX")?;
    if !interpreter.is_file() || !runner.is_file() || !python_module_exists(&interpreter, "whisperx") {
        bail!("O runtime do WhisperX não passou na validação final. Verifique a instalação do Python e tente novamente.");
    }
    println!("Forced Alignment runtime is ready: {}", runtime.display());
    Ok(runtime)
}

pub(crate) fn ensure_dependency(root: &Path, id: &str) -> Result<PathBuf> {
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
        "forced-alignment" => ensure_forced_alignment(root),
        "piper-tts" => ensure_piper_tts(root),
        "qwen3-tts" => ensure_qwen3_tts(root),
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

    let declared = config.get("shared")
        .and_then(|shared| shared.get("dependencies"))
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| config.get("dependencies").and_then(Value::as_array).cloned())
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
    fn piper_and_qwen3_tts_are_optional_by_default() {
        let dependencies = default_contract()["dependencies"].as_array().unwrap().clone();
        for id in ["piper-tts", "qwen3-tts"] {
            let dependency = dependencies.iter().find(|item| item["id"] == id).unwrap();
            assert_eq!(dependency["required"], false);
        }
    }

    #[test]
    fn forced_alignment_is_optional_by_default() {
        let dependencies = default_contract()["dependencies"].as_array().unwrap().clone();
        let alignment = dependencies.iter().find(|item| item["id"] == "forced-alignment").unwrap();
        assert_eq!(alignment["required"], false);
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
