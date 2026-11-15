use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize, PtySystem, SlavePty};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub app_id: String,
    pub name: String,
    pub pid: u32,
    pub started_at: u64,
    pub status: String,
    pub exit_code: Option<u32>,
    pub exit_signal: Option<String>,
}

struct ManagedProcess {
    child: Box<dyn PtyChild + Send + Sync>,
    _master: Box<dyn MasterPty + Send>,
    info: ProcessInfo,
}

struct ShellSession {
    child: Box<dyn PtyChild + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
}

static PROCESSES: OnceLock<Mutex<BTreeMap<String, ManagedProcess>>> = OnceLock::new();
static SHELLS: OnceLock<Mutex<BTreeMap<String, ShellSession>>> = OnceLock::new();
static OUTPUTS: OnceLock<Mutex<BTreeMap<String, Vec<String>>>> = OnceLock::new();

fn processes() -> &'static Mutex<BTreeMap<String, ManagedProcess>> {
    PROCESSES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn shells() -> &'static Mutex<BTreeMap<String, ShellSession>> {
    SHELLS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn outputs() -> &'static Mutex<BTreeMap<String, Vec<String>>> {
    OUTPUTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Returns the most recent terminal output for an application. This is used
/// to attach stdout/stderr context to persisted crash diagnostics.
pub fn recent_output(app_id: &str) -> Vec<String> {
    outputs().lock()
        .ok()
        .and_then(|items| items.get(app_id).cloned())
        .unwrap_or_default()
}

pub use crate::shell::bash_path;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub fn emit_line(app: &AppHandle, app_id: &str, stream: &str, line: String) {
    if let Ok(mut all) = outputs().lock() {
        let lines = all.entry(app_id.to_owned()).or_default();
        lines.push(format!("[{stream}] {line}"));
        if lines.len() > 1000 {
            let excess = lines.len() - 1000;
            lines.drain(0..excess);
        }
    }
    let _ = app.emit("srm-terminal-output", serde_json::json!({
        "appId": app_id, "stream": stream, "line": line, "timestamp": now()
    }));
}


pub fn start_dev(app: AppHandle, app_id: String, name: String, cwd: PathBuf, command: String) -> Result<ProcessInfo> {
    let bash = bash_path()?;
    let mut registry = processes().lock().map_err(|_| anyhow!("O registro de processos está indisponível."))?;
    if let Some(existing) = registry.get_mut(&app_id) {
        if existing.child.try_wait()?.is_none() {
            return Err(anyhow!("{} já está em execução no Monitor de Aplicações.", name));
        }
        registry.remove(&app_id);
    }

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 32,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }).context("Não foi possível criar o terminal PTY para iniciar a aplicação.")?;
    let mut process_command = CommandBuilder::new(bash);
    process_command.args(["--noprofile", "--norc", "-lc"]);
    #[cfg(windows)]
    process_command.arg(&command);
    #[cfg(not(windows))]
    process_command.arg(format!("exec {command}"));
    process_command.cwd(cwd.clone());
    process_command.env("TERM", "xterm-256color");

    let child = pair.slave.spawn_command(process_command)
        .with_context(|| format!("Não foi possível iniciar {name} dentro do terminal PTY em {}.", cwd.display()))?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()
        .context("Não foi possível capturar a saída do terminal da aplicação.")?;
    let master = pair.master;
    let pid = child.process_id().unwrap_or_default();
    let info = ProcessInfo {
        app_id: app_id.clone(),
        name,
        pid,
        started_at: now(),
        status: "running".to_owned(),
        exit_code: None,
        exit_signal: None,
    };

    let output_app = app.clone();
    let output_id = app_id.clone();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => emit_line(&output_app, &output_id, "pty", String::from_utf8_lossy(&buffer[..size]).into_owned()),
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe || error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => {
                    emit_line(&output_app, &output_id, "system", format!("Falha ao ler o terminal da aplicação: {error}"));
                    break;
                }
            }
        }
        emit_line(&output_app, &output_id, "system", "Saída do processo DEV encerrada.".to_owned());
    });

    emit_line(&app, &app_id, "system", format!("Aplicação iniciada em terminal PTY · PID {pid} · {}", cwd.display()));
    registry.insert(app_id, ManagedProcess { child, _master: master, info: info.clone() });
    Ok(info)
}

pub fn list() -> Result<Vec<ProcessInfo>> {
    let mut registry = processes().lock().map_err(|_| anyhow!("O registro de processos está indisponível."))?;
    let mut result = Vec::new();
    let mut finished = Vec::new();
    for (id, process) in registry.iter_mut() {
        if let Some(status) = process.child.try_wait()? {
            process.info.status = if status.success() { "stopped" } else { "error" }.to_owned();
            process.info.exit_code = Some(status.exit_code());
            process.info.exit_signal = status.signal().map(str::to_owned);
            finished.push(id.clone());
        }
        result.push(process.info.clone());
    }
    for id in finished { registry.remove(&id); }
    Ok(result)
}

pub fn stop(app: AppHandle, app_id: &str) -> Result<()> {
    let mut registry = processes().lock().map_err(|_| anyhow!("O registro de processos está indisponível."))?;
    let process = registry.get_mut(app_id).ok_or_else(|| anyhow!("Não há processo ativo para {app_id}."))?;
    #[cfg(windows)]
    {
        let pid = process.info.pid.to_string();
        let _ = Command::new("taskkill").args(["/PID", &pid, "/T", "/F"])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    #[cfg(target_os = "linux")]
    {
        let process_group = format!("-{}", process.info.pid);
        let _ = Command::new("kill").args(["-TERM", "--", &process_group])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let pid = process.info.pid.to_string();
        let _ = Command::new("pkill").args(["-TERM", "-P", &pid])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = process.child.kill();
    let _ = process.child.wait();
    emit_line(&app, app_id, "system", "Processo encerrado pelo usuário.".to_owned());
    registry.remove(app_id);
    Ok(())
}

/// Starts a persistent interactive Bash attached to a real pseudo-terminal (PTY).
pub fn start_terminal(app: AppHandle, app_id: String, cwd: PathBuf) -> Result<u32> {
    let bash = bash_path()?;
    let mut sessions = shells().lock().map_err(|_| anyhow!("O registro dos terminais está indisponível."))?;
    if let Some(session) = sessions.get_mut(&app_id) {
        if session.child.try_wait()?.is_none() {
            return Ok(session.child.process_id().unwrap_or_default() as u32);
        }
        sessions.remove(&app_id);
    }

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 28,
        cols: 110,
        pixel_width: 0,
        pixel_height: 0,
    }).context("Não foi possível criar o pseudo-terminal (PTY).")?;
    let mut command = CommandBuilder::new(bash);
    command.args(["--noprofile", "--norc", "-i"]);
    command.cwd(cwd.clone());
    let child = pair.slave.spawn_command(command)
        .with_context(|| format!("Não foi possível iniciar o Bash interativo em {}.", cwd.display()))?;
    let reader = pair.master.try_clone_reader()
        .context("Não foi possível abrir a saída do pseudo-terminal.")?;
    let writer = pair.master.take_writer()
        .context("Não foi possível abrir a entrada do pseudo-terminal.")?;
    let master = pair.master;
    let pid = child.process_id().unwrap_or_default() as u32;

    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => emit_line(&app, &app_id, "pty", String::from_utf8_lossy(&buffer[..size]).into_owned()),
                Err(error) => {
                    emit_line(&app, &app_id, "system", format!("Leitura do terminal encerrada: {error}"));
                    break;
                }
            }
        }
        emit_line(&app, &app_id, "system", "Sessão PTY encerrada.".to_owned());
    });

    emit_line(&app, &app_id, "system", format!("Terminal Bash interativo iniciado · PTY · PID {pid} · {}", cwd.display()));
    sessions.insert(app_id, ShellSession { child, master, writer });
    Ok(pid)
}

/// Sends raw keyboard input to the interactive terminal, including control keys.
pub fn write_terminal(app_id: &str, data: &str) -> Result<()> {
    let mut sessions = shells().lock().map_err(|_| anyhow!("O registro dos terminais está indisponível."))?;
    let session = sessions.get_mut(app_id)
        .ok_or_else(|| anyhow!("O terminal de {app_id} ainda não foi iniciado."))?;
    session.writer.write_all(data.as_bytes())
        .with_context(|| format!("Não foi possível enviar entrada ao terminal de {app_id}."))?;
    session.writer.flush()
        .with_context(|| format!("Não foi possível atualizar a entrada do terminal de {app_id}."))?;
    Ok(())
}

/// Resizes the PTY to match the visible terminal dimensions.
pub fn resize_terminal(app_id: &str, cols: u16, rows: u16) -> Result<()> {
    let mut sessions = shells().lock().map_err(|_| anyhow!("O registro dos terminais está indisponível."))?;
    let session = sessions.get_mut(app_id)
        .ok_or_else(|| anyhow!("O terminal de {app_id} ainda não foi iniciado."))?;
    session.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .with_context(|| format!("Não foi possível redimensionar o terminal de {app_id}."))?;
    Ok(())
}

/// Compatibility entry point for callers that submit a complete command line.
pub fn run_command(app: AppHandle, app_id: String, cwd: PathBuf, command: String) -> Result<()> {
    if command.trim().is_empty() {
        start_terminal(app, app_id, cwd)?;
        return Ok(());
    }
    start_terminal(app.clone(), app_id.clone(), cwd)?;
    emit_line(&app, &app_id, "command", format!("$ {command}"));
    write_terminal(&app_id, &format!("{command}\r"))
}

/// Closes one interactive terminal session.
pub fn stop_terminal(app: AppHandle, app_id: &str) -> Result<()> {
    let mut sessions = shells().lock().map_err(|_| anyhow!("O registro dos terminais está indisponível."))?;
    if let Some(mut session) = sessions.remove(app_id) {
        let _ = session.child.kill();
        let _ = session.child.wait();
        emit_line(&app, app_id, "system", "Terminal encerrado pelo usuário.".to_owned());
    }
    Ok(())
}

pub fn stop_all(app: AppHandle) {
    let ids = processes().lock().map(|registry| registry.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    for id in ids {
        let _ = stop(app.clone(), &id);
    }
    if let Ok(mut sessions) = shells().lock() {
        for (id, mut session) in std::mem::take(&mut *sessions) {
            let _ = session.child.kill();
            let _ = session.child.wait();
            emit_line(&app, &id, "system", "Terminal PTY encerrado com o Shared Runtime Manager.".to_owned());
        }
    }
}
