use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

const MODEL_ID: &str = "Qwen/Qwen3-TTS-12Hz-1.7B-Base";

fn silent_command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}


#[derive(Debug, Clone, Serialize, Deserialize)]
struct VoiceProfile {
    id: String,
    name: String,
    profile_path: String,
    source_audio: String,
    reference_text: String,
    duration_seconds: f64,
    created_at: u64,
    model_id: String,
    runtime_path: String,
    model_path: String,
    runner: String,
    python: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct VoiceRegistry {
    schema_version: String,
    default_voice_id: Option<String>,
    voices: Vec<VoiceProfile>,
}

fn root() -> Result<PathBuf> { super::manager::root() }
fn registry_path(root: &Path) -> PathBuf { root.join("registry/voices.json") }

fn read_registry(root: &Path) -> Result<VoiceRegistry> {
    let path = registry_path(root);
    if !path.is_file() {
        return Ok(VoiceRegistry { schema_version: "1.0".into(), ..VoiceRegistry::default() });
    }
    serde_json::from_slice(&fs::read(&path)?).context("A biblioteca de vozes contém um JSON inválido")
}

fn write_registry(root: &Path, registry: &VoiceRegistry) -> Result<()> {
    let path = registry_path(root);
    fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(registry)?)?;
    if path.exists() { fs::remove_file(&path)?; }
    fs::rename(temporary, path)?;
    Ok(())
}

fn find_file(directory: &Path, filename: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, filename) { return Some(found); }
        } else if path.file_name().and_then(|v| v.to_str()).is_some_and(|v| v.eq_ignore_ascii_case(filename)) {
            return Some(path);
        }
    }
    None
}

fn executable(root: &Path, name: &str) -> Result<PathBuf> {
    let filename = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    find_file(&root.join("tools"), &filename)
        .or_else(|| {
            let output = if cfg!(windows) {
                silent_command("where").arg(&filename).output().ok()?
            } else {
                silent_command("which").arg(&filename).output().ok()?
            };
            if !output.status.success() { return None; }
            let found = String::from_utf8_lossy(&output.stdout).lines().next()?.trim().to_owned();
            (!found.is_empty()).then(|| PathBuf::from(found))
        })
        .ok_or_else(|| anyhow!("{name} não está disponível. Instale FFmpeg e FFprobe pela Biblioteca de dependências."))
}

fn runtime_info(root: &Path) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let runtime = root.join("runtimes/qwen3-tts-clone");
    let manifest_path = runtime.join("runtime.json");
    if !manifest_path.is_file() {
        bail!("O runtime de clonagem Qwen3-TTS Base não está instalado. Instale 'Qwen3-TTS Base · Voice Cloning' na Biblioteca de dependências.");
    }
    let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    let python = manifest.get("python").and_then(Value::as_str).ok_or_else(|| anyhow!("Runtime sem caminho do Python"))?;
    let runner = PathBuf::from(manifest.get("runner").and_then(Value::as_str).ok_or_else(|| anyhow!("Runtime sem caminho do executor"))?);
    let model = PathBuf::from(manifest.get("model").and_then(Value::as_str).ok_or_else(|| anyhow!("Runtime sem caminho do modelo"))?);
    let python = PathBuf::from(python);
    // Keep already-installed Shared runtimes in sync with the bundled worker after app updates.
    let source = include_str!("../assets/qwen3_tts/voice_runner.py");
    if !runner.is_file() || fs::read_to_string(&runner).ok().as_deref() != Some(source) {
        fs::write(&runner, source).with_context(|| format!("Não foi possível atualizar o executor em {}", runner.display()))?;
    }
    Ok((python, runner, model))
}

fn run_worker(python: &Path, runner: &Path, model: &Path, profile: &Path, request: Value) -> Result<Value> {
    let mut child = silent_command(python)
        .arg(runner).arg(model).arg(profile)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().context("Não foi possível iniciar o executor Qwen3-TTS Base")?;
    child.stdin.take().ok_or_else(|| anyhow!("Entrada do executor indisponível"))?
        .write_all(format!("{}\n", serde_json::to_string(&request)?).as_bytes())?;
    let output = child.wait_with_output()?;
    let response: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("O executor de voz retornou uma resposta inválida: {}", String::from_utf8_lossy(&output.stderr).trim()))?;
    if !output.status.success() {
        bail!("{}", response.get("error").and_then(Value::as_str).unwrap_or("Falha no executor de voz."));
    }
    Ok(response)
}

pub fn list() -> Result<Value> {
    let root = root()?;
    // Refresh the installed worker whenever the voice library is opened.
    let _ = runtime_info(&root);
    let registry = read_registry(&root)?;
    Ok(json!({ "default_voice_id": registry.default_voice_id, "voices": registry.voices }))
}

pub fn create_profile(name: &str, filename: &str, audio_base64: &str, reference_text: &str) -> Result<Value> {
    let name = name.trim();
    let reference_text = reference_text.trim();
    if name.is_empty() { bail!("Informe um nome para a voz."); }
    if reference_text.is_empty() { bail!("Informe a transcrição exata do áudio de referência."); }
    let extension = Path::new(filename).extension().and_then(|v| v.to_str()).unwrap_or("").to_ascii_lowercase();
    if !["mp3", "wav", "m4a", "flac", "ogg", "aac", "wma"].contains(&extension.as_str()) {
        bail!("Formato de áudio não suportado. Use MP3, WAV, M4A, FLAC, OGG, AAC ou WMA.");
    }
    let bytes = STANDARD.decode(audio_base64).context("O áudio enviado não está em Base64 válido")?;
    if bytes.is_empty() || bytes.len() > 100 * 1024 * 1024 { bail!("O arquivo está vazio ou excede o limite de 100 MB."); }

    let root = root()?;
    let ffmpeg = executable(&root, "ffmpeg")?;
    let ffprobe = executable(&root, "ffprobe")?;
    let (python, runner, model) = runtime_info(&root)?;
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let now = clock.as_secs();
    let unique_stamp = clock.as_millis();
    let slug: String = name.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let slug = slug.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
    let id = format!("{}-{}", if slug.is_empty() { "voice" } else { &slug }, unique_stamp);
    let profile = root.join("voices").join(&id);
    fs::create_dir_all(&profile)?;
    let source = profile.join(format!("reference.{extension}"));
    fs::write(&source, bytes)?;
    let duration_output = silent_command(&ffprobe).args([
        "-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1",
    ]).arg(&source).output().context("Não foi possível analisar a duração do áudio")?;
    if !duration_output.status.success() { bail!("O arquivo enviado não contém um áudio válido."); }
    let duration: f64 = String::from_utf8_lossy(&duration_output.stdout).trim().parse()
        .context("Não foi possível determinar a duração do áudio")?;
    if !(10.0..=200.0).contains(&duration) {
        let _ = fs::remove_dir_all(&profile);
        bail!("O áudio precisa ter entre 10 e 200 segundos. Duração detectada: {:.1}s.", duration);
    }
    let normalized = profile.join("reference.wav");
    let normalize = silent_command(&ffmpeg).args(["-y", "-i"]).arg(&source)
        .args(["-vn", "-ac", "1", "-ar", "24000", "-c:a", "pcm_s16le"])
        .arg(&normalized).output().context("Não foi possível preparar o áudio de referência")?;
    if !normalize.status.success() {
        let _ = fs::remove_dir_all(&profile);
        bail!("Falha ao converter o áudio: {}", String::from_utf8_lossy(&normalize.stderr).trim());
    }
    let prepared = run_worker(&python, &runner, &model, &profile, json!({
        "operation": "prepare_voice",
        "reference_audio": normalized,
        "reference_text": reference_text
    }))?;
    if prepared.get("status").and_then(Value::as_str) != Some("completed") {
        let _ = fs::remove_dir_all(&profile);
        bail!("O modelo não conseguiu preparar a voz: {}", prepared.get("error").and_then(Value::as_str).unwrap_or("erro desconhecido"));
    }
    let voice = VoiceProfile {
        id: id.clone(), name: name.to_owned(), profile_path: profile.display().to_string(),
        source_audio: source.display().to_string(), reference_text: reference_text.to_owned(),
        duration_seconds: duration, created_at: now, model_id: MODEL_ID.to_owned(),
        runtime_path: root.join("runtimes/qwen3-tts-clone").display().to_string(),
        model_path: model.display().to_string(), runner: runner.display().to_string(), python: python.display().to_string(),
    };
    let mut registry = read_registry(&root)?;
    registry.schema_version = "1.0".into();
    if registry.default_voice_id.is_none() { registry.default_voice_id = Some(id.clone()); }
    registry.voices.push(voice.clone());
    write_registry(&root, &registry)?;
    super::manager::refresh_application_configs(&root)?;
    Ok(serde_json::to_value(voice)?)
}

pub fn set_default(id: &str) -> Result<()> {
    let root = root()?;
    let mut registry = read_registry(&root)?;
    if !registry.voices.iter().any(|voice| voice.id == id) { bail!("A voz selecionada não existe na biblioteca."); }
    registry.default_voice_id = Some(id.to_owned());
    write_registry(&root, &registry)?;
    super::manager::refresh_application_configs(&root)?;
    Ok(())
}

pub fn preview(id: &str, text: &str) -> Result<String> {
    let root = root()?;
    let registry = read_registry(&root)?;
    let voice = registry.voices.iter().find(|voice| voice.id == id)
        .ok_or_else(|| anyhow!("Voz não encontrada: {id}"))?;
    let output = PathBuf::from(&voice.profile_path).join("preview.wav");
    let result = run_worker(Path::new(&voice.python), Path::new(&voice.runner), Path::new(&voice.model_path),
        Path::new(&voice.profile_path), json!({
            "operation": "synthesize",
            "text": text,
            "language": "Portuguese",
            "output_path": output
        }))?;
    if result.get("status").and_then(Value::as_str) != Some("completed") {
        bail!("Falha ao gerar prévia: {}", result.get("error").and_then(Value::as_str).unwrap_or("erro desconhecido"));
    }
    Ok(output.display().to_string())
}

pub(crate) fn config_for_consumers(root: &Path) -> Result<Value> {
    let registry = read_registry(root)?;
    let available: Vec<Value> = registry.voices.iter().map(|voice| json!({
        "id": voice.id, "name": voice.name, "profile_path": voice.profile_path,
        "runtime_path": voice.runtime_path, "model_path": voice.model_path,
        "runner": voice.runner, "python": voice.python, "model_id": voice.model_id
    })).collect();
    Ok(json!({ "default": registry.default_voice_id, "available": available }))
}

pub(crate) fn ensure_runtime(root: &Path) -> Result<PathBuf> {
    super::dependency_queue::report_install_progress(5, "Preparando o runtime Qwen3-TTS Base para clonagem")?;
    let runtime = root.join("runtimes/qwen3-tts-clone");
    let model = root.join("models/qwen3-tts-clone/1.7B-Base");
    let cache = root.join("cache/huggingface");
    fs::create_dir_all(&runtime)?; fs::create_dir_all(&model)?; fs::create_dir_all(&cache)?;
    let venv = runtime.join("venv");
    let python = if cfg!(windows) { venv.join("Scripts/python.exe") } else { venv.join("bin/python") };
    if !python.is_file() {
        let base = if cfg!(windows) { "py" } else { "python3" };
        let mut command = silent_command(base);
        if cfg!(windows) { command.arg("-3"); }
        let status = command.args(["-m", "venv"]).arg(&venv).status()?;
        if !status.success() { bail!("Não foi possível criar o ambiente Python do Qwen3-TTS Base."); }
    }
    let cpu_check = "import torch; assert torch.version.cuda is None and not torch.cuda.is_available()";
    let check = silent_command(&python).args(["-c", cpu_check]).status().map(|s| s.success()).unwrap_or(false);
    if !check {
        let status = silent_command(&python).args(["-m", "pip", "install", "--upgrade", "pip"]).status()?;
        if !status.success() { bail!("Falha ao preparar pip para Qwen3-TTS Base."); }
        let status = silent_command(&python).args(["-m", "pip", "install", "--force-reinstall", "torch", "torchaudio", "--index-url", "https://download.pytorch.org/whl/cpu"]).status()?;
        if !status.success() { bail!("Falha ao instalar PyTorch CPU para Qwen3-TTS Base."); }
    }
    let package_check = silent_command(&python).args(["-c", "import qwen_tts, soundfile"]).status().map(|s| s.success()).unwrap_or(false);
    if !package_check {
        super::dependency_queue::report_install_progress(42, "Instalando Qwen3-TTS Base e dependências de áudio")?;
        let status = silent_command(&python).args(["-m", "pip", "install", "qwen-tts", "soundfile"]).status()?;
        if !status.success() { bail!("Falha ao instalar qwen-tts e soundfile."); }
    }
    let cpu_still_valid = silent_command(&python).args(["-c", cpu_check]).status().map(|s| s.success()).unwrap_or(false);
    if !cpu_still_valid {
        super::dependency_queue::report_install_progress(58, "Restaurando PyTorch CPU após instalar o Qwen3-TTS")?;
        let status = silent_command(&python).args(["-m", "pip", "install", "--force-reinstall", "torch", "torchaudio", "--index-url", "https://download.pytorch.org/whl/cpu"]).status()?;
        if !status.success() { bail!("Falha ao restaurar PyTorch CPU."); }
    }
    let model_config = model.join("config.json");
    let model_weights = model.join("model.safetensors");
    let model_index = model.join("model.safetensors.index.json");
    if !model_config.is_file() || !(model_weights.is_file() || model_index.is_file()) {
        super::dependency_queue::report_install_progress(68, "Baixando o modelo oficial Qwen3-TTS 1.7B Base")?;
        let script = "import sys; from huggingface_hub import snapshot_download; snapshot_download(repo_id='Qwen/Qwen3-TTS-12Hz-1.7B-Base', local_dir=sys.argv[1], cache_dir=sys.argv[2])";
        let status = silent_command(&python).arg("-c").arg(script).arg(&model).arg(&cache).env("HF_HUB_DISABLE_TELEMETRY", "1").status()?;
        if !status.success() { bail!("Falha ao baixar o modelo Qwen3-TTS 1.7B Base."); }
    }
    if !model_config.is_file() || !(model_weights.is_file() || model_index.is_file()) {
        bail!("O download do Qwen3-TTS Base terminou sem todos os arquivos do modelo.");
    }
    super::dependency_queue::report_install_progress(94, "Preparando o executor de clonagem de voz")?;
    let runner = runtime.join("voice_runner.py");
    let source = include_str!("../assets/qwen3_tts/voice_runner.py");
    fs::write(&runner, source)?;
    let manifest = json!({"schemaVersion":"1.0","runtime":"qwen3-tts-clone","python":python,"runner":runner,"device":"cpu","dtype":"float32","modelId":MODEL_ID,"model":model,"supportsVoiceCloning":true});
    fs::write(runtime.join("runtime.json"), serde_json::to_vec_pretty(&manifest)?)?;
    super::dependency_queue::report_install_progress(100, "Qwen3-TTS Base pronto para criar perfis de voz")?;
    Ok(runtime)
}
