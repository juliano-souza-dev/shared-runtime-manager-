use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub app_id: String,
    pub name: String,
    pub pid: u32,
    pub started_at: u64,
    pub status: String,
}

struct ManagedProcess {
    child: Child,
    info: ProcessInfo,
}

static PROCESSES: OnceLock<Mutex<BTreeMap<String, ManagedProcess>>> = OnceLock::new();

fn processes() -> &'static Mutex<BTreeMap<String, ManagedProcess>> {
    PROCESSES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub use crate::shell::bash_path;
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn emit_line(app: &AppHandle, app_id: &str, stream: &str, line: String) {
    let _ = app.emit("srm-terminal-output", serde_json::json!({
        "appId": app_id, "stream": stream, "line": line, "timestamp": now()
    }));
}

fn stream_output<R: std::io::Read + Send + 'static>(
    app: AppHandle,
    app_id: String,
    stream: &'static str,
    reader: R,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => emit_line(&app, &app_id, stream, line),
                Err(error) => {
                    emit_line(&app, &app_id, "system", format!("Falha ao ler saída: {error}"));
                    break;
                }
            }
        }
    });
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

    let mut child = Command::new(bash)
        .arg("--noprofile")
        .arg("--norc")
        .arg("-lc")
        .arg(format!("exec {command}"))
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Não foi possível iniciar o Bash para {name}."))?;

    let info = ProcessInfo {
        app_id: app_id.clone(),
        name,
        pid: child.id(),
        started_at: now(),
        status: "running".to_owned(),
    };
    if let Some(stdout) = child.stdout.take() {
        stream_output(app.clone(), app_id.clone(), "stdout", stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        stream_output(app.clone(), app_id.clone(), "stderr", stderr);
    }
    emit_line(&app, &app_id, "system", format!("Iniciado via Bash · PID {}", info.pid));
    registry.insert(app_id, ManagedProcess { child, info: info.clone() });
    Ok(info)
}

pub fn list() -> Result<Vec<ProcessInfo>> {
    let mut registry = processes().lock().map_err(|_| anyhow!("O registro de processos está indisponível."))?;
    let mut result = Vec::new();
    let mut finished = Vec::new();
    for (id, process) in registry.iter_mut() {
        if let Some(status) = process.child.try_wait()? {
            process.info.status = if status.success() { "stopped" } else { "error" }.to_owned();
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
        let _ = Command::new("taskkill").args(["/PID", &process.info.pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = process.child.kill();
    let _ = process.child.wait();
    emit_line(&app, app_id, "system", "Processo encerrado pelo usuário.".to_owned());
    registry.remove(app_id);
    Ok(())
}

pub fn run_command(app: AppHandle, app_id: String, cwd: PathBuf, command: String) -> Result<()> {
    if command.trim().is_empty() { return Ok(()); }
    let bash = bash_path()?;
    emit_line(&app, &app_id, "command", format!("$ {}", command));
    let mut child = Command::new(bash)
        .arg("--noprofile").arg("--norc").arg("-lc").arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().context("Não foi possível executar o comando no Bash.")?;
    if let Some(stdout) = child.stdout.take() { stream_output(app.clone(), app_id.clone(), "stdout", stdout); }
    if let Some(stderr) = child.stderr.take() { stream_output(app.clone(), app_id.clone(), "stderr", stderr); }
    std::thread::spawn(move || {
        match child.wait() {
            Ok(status) => emit_line(&app, &app_id, "system", format!("Comando finalizado · código {}", status.code().unwrap_or(-1))),
            Err(error) => emit_line(&app, &app_id, "system", format!("Falha ao aguardar comando: {error}")),
        }
    });
    Ok(())
}
