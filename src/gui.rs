#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde::Serialize;
use std::{fs, sync::{atomic::{AtomicBool, Ordering}, Mutex}};
use shared_runtime_manager::{api, applications, dependency_queue, manager, process_manager};

static QUEUE_RUNNING: AtomicBool = AtomicBool::new(false);
static PROCESSING_LOG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
struct Dashboard {
    root: String,
    applications: Vec<manager::ApplicationInfo>,
    resources: Vec<manager::ResourceInfo>,
    providers: Vec<api::ProviderInfo>,
}
fn error_text(error: impl std::fmt::Display) -> String { error.to_string() }
fn detailed_error(error: &anyhow::Error) -> String { format!("{error:#}\n\nDiagnóstico técnico:\n{error:?}") }

fn record_processing_failure(task_type: &str, title: &str, detail: &str) -> String {
    if let Err(log_error) = append_processing_log(task_type, title, detail) {
        return format!("{detail}\n\nTambém não foi possível gravar o log: {log_error}");
    }
    detail.to_owned()
}

fn record_processing_error(task_type: &str, title: &str, error: impl std::fmt::Display) -> String {
    record_processing_failure(task_type, title, &error.to_string())
}

fn append_processing_log(task_type: &str, title: &str, detail: &str) -> Result<(), String> {
    let _guard = PROCESSING_LOG_LOCK.lock().map_err(|_| "O registro de logs está indisponível.".to_owned())?;
    let root = manager::root().map_err(error_text)?;
    let registry = root.join("registry");
    fs::create_dir_all(&registry).map_err(error_text)?;
    let path = registry.join("processing-logs.json");
    let mut logs: Vec<serde_json::Value> = if path.is_file() {
        serde_json::from_slice(&fs::read(&path).map_err(error_text)?).map_err(error_text)?
    } else { Vec::new() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let timestamp = now.as_secs();
    logs.insert(0, serde_json::json!({
        "id": format!("{}-{}", now.as_millis(), logs.len()),
        "timestamp": timestamp,
        "taskType": task_type,
        "title": title,
        "detail": detail
    }));
    logs.truncate(200);
    let temporary = registry.join("processing-logs.json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&logs).map_err(error_text)?).map_err(error_text)?;
    if path.exists() { fs::remove_file(&path).map_err(error_text)?; }
    fs::rename(&temporary, &path).map_err(error_text)
}

fn record_status_failures(task_type: &str, title: &str) {
    if let Ok(status) = dependency_queue::status() {
        if let Some(results) = status.get("results").and_then(serde_json::Value::as_array) {
            for result in results.iter().filter(|item| item.get("status").and_then(serde_json::Value::as_str) == Some("error")) {
                let item_title = result.get("id").and_then(serde_json::Value::as_str).unwrap_or(title);
                let detail = result.get("message").and_then(serde_json::Value::as_str).unwrap_or("Falha sem detalhes.");
                let _ = append_processing_log(task_type, &format!("{title} · {item_title}"), detail);
            }
        }
    }
}

fn write_voice_processing_status(status: &str, current: &str, result: Option<serde_json::Value>) -> Result<(), String> {
    let root = manager::root().map_err(error_text)?;
    let registry = root.join("registry");
    fs::create_dir_all(&registry).map_err(error_text)?;
    let target = registry.join("dependency-queue-status.json");
    let temporary = registry.join("dependency-queue-status.json.tmp");
    let terminal = status == "completed" || status == "error";
    let value = serde_json::json!({
        "status": status,
        "progress": if status == "completed" { serde_json::Value::from(100) } else { serde_json::Value::Null },
        "progressMode": if status == "completed" { "determinate" } else { "indeterminate" },
        "current": current,
        "taskType": "voice-profile",
        "completed": if terminal { 1 } else { 0 },
        "total": 1,
        "results": result.into_iter().collect::<Vec<_>>()
    });
    fs::write(&temporary, serde_json::to_vec_pretty(&value).map_err(error_text)?).map_err(error_text)?;
    if target.exists() { fs::remove_file(&target).map_err(error_text)?; }
    fs::rename(&temporary, &target).map_err(error_text)
}

#[tauri::command]
fn applications_catalog() -> Result<Vec<applications::CatalogApplication>, String> {
    applications::catalog().map_err(|error| record_processing_error("catalog", "Falha ao carregar o catálogo de aplicativos", error))
}
#[tauri::command]
fn install_ecosystem_application(app: tauri::AppHandle, app_id: String, mode: String) -> Result<(), String> {
    if QUEUE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err(record_processing_error("application-install", &format!("Não foi possível iniciar a instalação de {app_id}"), "Já existe um processamento em andamento. Aguarde antes de instalar outro aplicativo."));
    }
    dependency_queue::begin_operation();
    std::thread::spawn(move || {
        match applications::install_with_output(&app_id, &mode, |stream, line| {
            process_manager::emit_line(&app, &app_id, stream, line.to_owned());
        }) {
            Ok(()) if mode == "dev" => {
                match applications::dev_spec(&app_id).and_then(|(cwd, name, command)| {
                    process_manager::start_dev(app.clone(), app_id.clone(), name, cwd, command)
                        .map(|_| ())
                        .map_err(|error| anyhow::anyhow!(error))
                }) {
                    Ok(()) => { let _ = write_install_status("completed", &format!("{app_id} instalado e iniciado em DEV"), &app_id, None, serde_json::json!([{"id":app_id,"status":"completed","message":"Instalado e iniciado pelo Bash integrado."}])); }
                    Err(error) => {
                        let detail = detailed_error(&error);
                        let _ = append_processing_log("application-start", &format!("Falha ao iniciar {app_id} em DEV"), &detail);
                        let _ = write_install_status("error", &format!("Instalado, mas não foi possível iniciar {app_id}"), &app_id, None, serde_json::json!([{"id":app_id,"status":"error","message":detail}]));
                    }
                }
            }
            Ok(()) => {}
            Err(error) => {
                let detail = detailed_error(&error);
                let _ = append_processing_log("application-install", &format!("Falha ao instalar {app_id}"), &detail);
                let _ = write_install_status("error", &format!("Falha ao instalar {app_id}"), &app_id, None,
                    serde_json::json!([{"id":app_id,"status":"error","message":detail}]));
                let _ = dependency_queue::record_history();
            }
        }
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(())
}
#[tauri::command]
fn open_ecosystem_application(app: tauri::AppHandle, app_id: String) -> Result<serde_json::Value, String> {
    let title = format!("Falha ao abrir/iniciar o aplicativo {app_id}");
    let result = (|| -> Result<serde_json::Value, String> {
        let entry = applications::catalog().map_err(|error| detailed_error(&error))?
            .into_iter().find(|item| item.id == app_id)
            .ok_or_else(|| format!("Aplicativo '{app_id}' não encontrado no catálogo."))?;
        if entry.mode.as_deref() == Some("dev") {
            if let Some(process) = process_manager::list().map_err(|error| detailed_error(&error))?.into_iter().find(|item| item.app_id == app_id) {
                return serde_json::to_value(process).map_err(error_text);
            }
            let (cwd, name, command) = applications::dev_spec(&app_id).map_err(|error| detailed_error(&error))?;
            let process = process_manager::start_dev(app, app_id.clone(), name, cwd, command).map_err(|error| detailed_error(&error))?;
            serde_json::to_value(process).map_err(error_text)
        } else {
            applications::open(&app_id).map_err(|error| detailed_error(&error))?;
            Ok(serde_json::json!({"status":"launched","mode":"executable"}))
        }
    })();
    result.map_err(|error| record_processing_error("application-launch", &title, error))
}
#[tauri::command]
fn application_processes() -> Result<Vec<process_manager::ProcessInfo>, String> {
    let processes = process_manager::list().map_err(|error| record_processing_error(
        "application-monitor", "Falha ao consultar processos dos aplicativos", error
    ))?;
    // list() removes terminated processes after returning their final status, so
    // each abnormal exit is logged exactly once by this polling command.
    for process in processes.iter().filter(|process| process.status == "error") {
        let output = process_manager::recent_output(&process.app_id);
        let numbered_output = if output.is_empty() {
            "O processo não forneceu saída capturada.".to_owned()
        } else {
            output.iter().enumerate()
                .map(|(index, line)| format!("[process:{:04}] {line}", index + 1))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let detail = format!(
            "O processo do aplicativo '{}' foi encerrado com falha.\nPID: {}\nIniciado em (Unix): {}\nStatus: {}\n\nSaída capturada (stdout/stderr):\n{}",
            process.name, process.pid, process.started_at, process.status, numbered_output
        );
        let _ = append_processing_log(
            "application-process",
            &format!("O aplicativo {} encerrou com erro", process.name),
            &detail
        );
    }
    Ok(processes)
}
#[tauri::command]
fn stop_application_process(app: tauri::AppHandle, app_id: String) -> Result<(), String> {
    process_manager::stop(app, &app_id).map_err(|error| record_processing_error(
        "application-stop", &format!("Falha ao encerrar o aplicativo {app_id}"), error
    ))
}
#[tauri::command]
fn start_application_terminal(app: tauri::AppHandle, app_id: String) -> Result<u32, String> {
    let (cwd, _, _) = applications::dev_spec(&app_id).map_err(|error| record_processing_error(
        "terminal-start", &format!("Falha ao preparar terminal de {app_id}"), error
    ))?;
    process_manager::start_terminal(app, app_id.clone(), cwd).map_err(|error| record_processing_error(
        "terminal-start", &format!("Falha ao iniciar terminal interativo de {app_id}"), error
    ))
}
#[tauri::command]
fn write_application_terminal(app_id: String, data: String) -> Result<(), String> {
    process_manager::write_terminal(&app_id, &data).map_err(|error| record_processing_error(
        "integrated-terminal", &format!("Falha ao enviar entrada ao terminal de {app_id}"), error
    ))
}
#[tauri::command]
fn resize_application_terminal(app_id: String, cols: u16, rows: u16) -> Result<(), String> {
    process_manager::resize_terminal(&app_id, cols, rows).map_err(|error| record_processing_error(
        "integrated-terminal", &format!("Falha ao redimensionar terminal de {app_id}"), error
    ))
}
#[tauri::command]
fn stop_application_terminal(app: tauri::AppHandle, app_id: String) -> Result<(), String> {
    process_manager::stop_terminal(app, &app_id).map_err(|error| record_processing_error(
        "terminal-stop", &format!("Falha ao encerrar terminal de {app_id}"), error
    ))
}
#[tauri::command]
fn run_application_shell_command(app: tauri::AppHandle, app_id: String, command: String) -> Result<(), String> {
    let title = format!("Falha ao enviar comando ao terminal integrado ({app_id})");
    let result = (|| -> Result<(), String> {
        let (cwd, _, _) = applications::dev_spec(&app_id).map_err(|error| detailed_error(&error))?;
        process_manager::run_command(app, app_id.clone(), cwd.clone(), command.clone()).map_err(|error| {
            format!("Comando: {command}\nDiretório: {}\nDiagnóstico técnico:\n{error:#?}", cwd.display())
        })
    })();
    result.map_err(|error| record_processing_error("integrated-terminal", &title, error))
}
#[tauri::command]
fn dashboard_data() -> Result<Dashboard, String> {
    let result = (|| -> Result<Dashboard, String> {
        let root = manager::root().map_err(error_text)?;
        Ok(Dashboard {
            root: root.display().to_string(),
            applications: manager::applications().map_err(error_text)?,
            resources: manager::resources().map_err(error_text)?,
            providers: api::providers().map_err(error_text)?,
        })
    })();
    result.map_err(|error| record_processing_error("dashboard", "Falha ao carregar os dados do gerenciador", error))
}
#[tauri::command]
fn register_application(app_id: String, name: String, installation_path: String) -> Result<(), String> {
    let title = format!("Falha ao registrar o aplicativo {app_id}");
    let result = (|| -> Result<(), String> {
        manager::register_app_at_path(&app_id, &name, &installation_path).map_err(error_text)?;
        dependency_queue::enqueue_application(&app_id, &installation_path).map_err(error_text)
    })();
    result.map_err(|error| record_processing_error("application-register", &title, error))
}
#[tauri::command]
fn delete_application(app_id: String) -> Result<(), String> {
    let result = manager::unregister_application(&app_id).map_err(error_text)
        .and_then(|_| dependency_queue::remove_application(&app_id).map_err(error_text));
    result.map_err(|error| record_processing_error("application-delete", &format!("Falha ao remover o vínculo de {app_id}"), error))
}
#[tauri::command]
fn synchronize_application(app_id: String) -> Result<(), String> {
    let title = format!("Falha ao sincronizar dependências de {app_id}");
    let result = (|| -> Result<(), String> {
        if QUEUE_RUNNING.load(Ordering::SeqCst) {
            return Err("A fila de dependências já está em execução. Aguarde a conclusão antes de sincronizar outro aplicativo.".to_owned());
        }
        let app = manager::applications().map_err(error_text)?
            .into_iter().find(|item| item.id == app_id)
            .ok_or_else(|| format!("Application '{app_id}' is not registered."))?;
        if app.installation_path.is_empty() {
            return Err(format!("Application '{app_id}' has no installation path."));
        }
        if !std::path::Path::new(&app.config_path).is_file() {
            manager::register_app_at_path(&app.id, &app.name, &app.installation_path).map_err(error_text)?;
        }
        dependency_queue::enqueue_application(&app_id, &app.installation_path).map_err(error_text)?;
        start_dependency_queue()
    })();
    result.map_err(|error| record_processing_error("dependency-sync", &title, error))
}
fn write_install_status(
    status: &str,
    current: &str,
    resource_id: &str,
    app_id: Option<&str>,
    results: serde_json::Value,
) -> Result<(), String> {
    let root = manager::root().map_err(error_text)?;
    let registry = root.join("registry");
    std::fs::create_dir_all(&registry).map_err(error_text)?;
    let target = registry.join("dependency-queue-status.json");
    let temporary = registry.join("dependency-queue-status.json.tmp");
    let value = serde_json::json!({
        "status": status,
        "progress": serde_json::Value::Null,
        "progressMode": "indeterminate",
        "current": current,
        "application": app_id,
        "resourceId": resource_id,
        "appId": app_id,
        "completed": 0,
        "total": 1,
        "results": results
    });
    std::fs::write(&temporary, serde_json::to_vec_pretty(&value).map_err(error_text)?).map_err(error_text)?;
    if target.exists() { std::fs::remove_file(&target).map_err(error_text)?; }
    std::fs::rename(&temporary, &target).map_err(error_text)
}

fn spawn_install_shared_resource(resource_id: String, app_id: Option<String>) -> Result<(), String> {
    if QUEUE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err(record_processing_error("shared-resource", &format!("Não foi possível instalar {resource_id}"), "Já existe um processamento em andamento. Aguarde antes de iniciar outra instalação."));
    }
    dependency_queue::begin_operation();
    if let Err(error) = write_install_status("installing", &format!("Preparando instalação de {resource_id}"), &resource_id, app_id.as_deref(), serde_json::json!([])) {
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
        return Err(record_processing_error("shared-resource", &format!("Falha ao preparar instalação de {resource_id}"), error));
    }
    std::thread::spawn(move || {
        match manager::install(&resource_id, app_id.as_deref()) {
            Ok(()) => {
                let result = serde_json::json!([{"id":resource_id.clone(),"status":"installed","message":"Recurso instalado e vínculos atualizados."}]);
                if write_install_status("completed", &format!("Instalação de {resource_id} concluída"), &resource_id, app_id.as_deref(), result).is_ok() {
                    let _ = dependency_queue::record_history();
                }
            }
            Err(_error) if dependency_queue::cancel_requested() => {
                let _ = dependency_queue::mark_cancelled();
            }
            Err(error) => {
                let detail = detailed_error(&error);
                let _ = append_processing_log("shared-resource", &format!("Falha ao instalar {resource_id}"), &detail);
                let result = serde_json::json!([{"id":resource_id.clone(),"status":"error","message":detail}]);
                if write_install_status("error", &format!("Falha ao instalar {resource_id}"), &resource_id, app_id.as_deref(), result).is_ok() {
                    let _ = dependency_queue::record_history();
                }
            }
        }
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[tauri::command]
fn install_shared_resource(resource_id: String, app_id: Option<String>) -> Result<(), String> {
    spawn_install_shared_resource(resource_id, app_id)
}
#[tauri::command]
fn delete_shared_dependency(dependency_id: String) -> Result<(), String> {
    manager::delete_resource(&dependency_id).map_err(|error| record_processing_error(
        "shared-resource-delete", &format!("Falha ao remover o recurso {dependency_id}"), error
    ))
}
#[tauri::command]
fn start_dependency_queue() -> Result<(), String> {
    if QUEUE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err(record_processing_error(
            "dependency-queue", "Não foi possível iniciar a fila de dependências",
            "A fila de dependências já está em execução."
        ));
    }
    dependency_queue::begin_operation();
    std::thread::spawn(|| {
        if let Err(error) = dependency_queue::process_queue() {
            if dependency_queue::cancel_requested() {
                let _ = dependency_queue::mark_cancelled();
            } else {
                let detail = detailed_error(&error);
                let _ = append_processing_log("dependency-queue", "Falha no processamento de dependências", &detail);
                let _ = dependency_queue::set_failure(&detail);
            }
        } else {
            record_status_failures("dependency-queue", "Falha na dependência");
        }
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(())
}
#[tauri::command]
fn cancel_processing() -> Result<(), String> {
    if !QUEUE_RUNNING.load(Ordering::SeqCst) {
        return Err(record_processing_error("processing-cancel", "Não foi possível cancelar o processamento", "Não há processamento ativo para cancelar."));
    }
    dependency_queue::request_cancel().map_err(|error| record_processing_error("processing-cancel", "Falha ao solicitar cancelamento do processamento", error))
}
#[tauri::command]
fn restart_processing(app: tauri::AppHandle) -> Result<(), String> {
    let result = (|| -> Result<(), String> {
        if QUEUE_RUNNING.load(Ordering::SeqCst) {
            return Err("A operação atual ainda está encerrando. Aguarde antes de reiniciar.".to_owned());
        }
        let status = dependency_queue::status().map_err(error_text)?;
        if let Some(resource_id) = status.get("resourceId").and_then(serde_json::Value::as_str) {
            let app_id = status.get("appId").and_then(serde_json::Value::as_str).map(str::to_owned);
            let resources = manager::resources().map_err(error_text)?;
            if resources.iter().any(|resource| resource.id == resource_id) {
                return spawn_install_shared_resource(resource_id.to_owned(), app_id);
            }
            // Ecosystem application installs also store their application ID in
            // resourceId for legacy status compatibility. Retry through the app installer.
            if let Some(application) = applications::catalog().map_err(error_text)?
                .into_iter().find(|application| application.id == resource_id)
            {
                let mode = application.mode.unwrap_or_else(|| "dev".to_owned());
                return install_ecosystem_application(app, resource_id.to_owned(), mode);
            }
            return Err(format!("Não foi possível identificar a operação '{resource_id}' para reiniciá-la."));
        }
        start_dependency_queue()
    })();
    result.map_err(|error| record_processing_error("processing-restart", "Falha ao reiniciar o processamento", error))
}
#[tauri::command]
fn dependency_queue_status() -> Result<serde_json::Value, String> {
    dependency_queue::status().map_err(|error| record_processing_error("queue-status", "Falha ao consultar o estado da fila", error))
}
#[tauri::command]
fn processing_logs() -> Result<Vec<serde_json::Value>, String> {
    let root = manager::root().map_err(error_text)?;
    let path = root.join("registry/processing-logs.json");
    if !path.is_file() { return Ok(Vec::new()); }
    serde_json::from_slice(&fs::read(path).map_err(error_text)?).map_err(error_text)
}
#[tauri::command]
fn voice_library() -> Result<serde_json::Value, String> {
    shared_runtime_manager::voices::list().map_err(|error| record_processing_error("voice-library", "Falha ao carregar a biblioteca de vozes", error))
}
#[tauri::command]
fn create_voice_profile(name: String, filename: String, audio_base64: String, reference_text: String) -> Result<(), String> {
    if QUEUE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err(record_processing_error("voice-profile", &format!("Não foi possível preparar o perfil {name}"), "Já existe um processamento em andamento. Aguarde antes de preparar outra voz."));
    }
    dependency_queue::begin_operation();
    if let Err(error) = write_voice_processing_status(
        "running",
        "Preparando o áudio de referência e executando o Qwen3-TTS Base...",
        None,
    ) {
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
        return Err(record_processing_error("voice-profile", "Falha ao iniciar a preparação do perfil de voz", error));
    }
    std::thread::spawn(move || {
        match shared_runtime_manager::voices::create_profile(&name, &filename, &audio_base64, &reference_text) {
            Ok(voice) => {
                let voice_name = voice.get("name").and_then(serde_json::Value::as_str).unwrap_or(&name);
                let result = serde_json::json!({
                    "id": "Perfil de voz",
                    "status": "completed",
                    "message": format!("Perfil '{voice_name}' criado e publicado nos aplicativos registrados.")
                });
                if write_voice_processing_status(
                    "completed",
                    &format!("Perfil de voz '{voice_name}' criado com sucesso."),
                    Some(result),
                ).is_ok() {
                    let _ = dependency_queue::record_history();
                }
            }
            Err(_error) if dependency_queue::cancel_requested() => {
                let _ = dependency_queue::mark_cancelled();
            }
            Err(error) => {
                let detail = detailed_error(&error);
                let result = serde_json::json!({
                    "id": "Perfil de voz",
                    "status": "error",
                    "message": detail
                });
                let _ = append_processing_log("voice-profile", "Falha ao preparar o perfil de voz", &detail);
                if write_voice_processing_status(
                    "error",
                    "Falha ao preparar o perfil de voz.",
                    Some(result),
                ).is_ok() {
                    let _ = dependency_queue::record_history();
                }
            }
        }
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(())
}
#[tauri::command]
fn set_default_voice(voice_id: String) -> Result<(), String> {
    shared_runtime_manager::voices::set_default(&voice_id).map_err(|error| record_processing_error("voice-default", &format!("Falha ao definir a voz padrão {voice_id}"), error))
}
#[tauri::command]
fn preview_voice(voice_id: String, text: String) -> Result<String, String> {
    shared_runtime_manager::voices::preview(&voice_id, &text).map_err(|error| record_processing_error(
        "voice-preview", &format!("Falha ao gerar prévia da voz {voice_id}"), error
    ))
}
#[tauri::command]
fn provider_catalog() -> Result<serde_json::Value, String> { api::provider_catalog().map_err(|error| record_processing_error("api-catalog", "Falha ao carregar o catálogo de provedores", error)) }
#[tauri::command]
fn available_api_models(provider: String) -> Result<Vec<String>, String> { api::available_models(&provider).map_err(|error| record_processing_error("api-models", &format!("Falha ao consultar modelos do provedor {provider}"), error)) }
#[tauri::command]
fn save_api_provider(provider: String, name: String, endpoint: String, model: String) -> Result<(), String> {
    api::add(&provider, &name, &endpoint, &model).map_err(|error| record_processing_error("api-provider-save", &format!("Falha ao salvar provedor {provider}"), error))
}
#[tauri::command]
fn authorize_api(provider: String, app_id: String) -> Result<(), String> {
    api::authorize(&provider, &app_id).map_err(|error| record_processing_error("api-authorize", &format!("Falha ao autorizar {app_id} no provedor {provider}"), error))
}
#[tauri::command]
fn revoke_api(provider: String, app_id: String) -> Result<(), String> {
    api::revoke(&provider, &app_id).map_err(|error| record_processing_error("api-revoke", &format!("Falha ao revogar {app_id} no provedor {provider}"), error))
}
#[tauri::command]
fn save_api_key(provider: String, secret: String) -> Result<(), String> {
    api::set_key(&provider, &secret).map_err(|error| record_processing_error("api-key-save", &format!("Falha ao armazenar chave do provedor {provider}"), error))
}
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            dashboard_data, applications_catalog, install_ecosystem_application, open_ecosystem_application, application_processes, stop_application_process, run_application_shell_command, start_application_terminal, write_application_terminal, resize_application_terminal, stop_application_terminal, register_application, delete_application, synchronize_application, delete_shared_dependency,
            start_dependency_queue, dependency_queue_status, processing_logs, install_shared_resource, cancel_processing, restart_processing,
            provider_catalog, available_api_models, save_api_provider, authorize_api,
            revoke_api, save_api_key, voice_library, create_voice_profile, set_default_voice, preview_voice
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Shared Runtime Manager")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                process_manager::stop_all(app_handle.clone());
            }
        });
}
