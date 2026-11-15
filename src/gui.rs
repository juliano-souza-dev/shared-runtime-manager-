#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde::Serialize;
use std::{fs, sync::atomic::{AtomicBool, Ordering}};
use shared_runtime_manager::{api, applications, dependency_queue, manager, process_manager};

static QUEUE_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
struct Dashboard {
    root: String,
    applications: Vec<manager::ApplicationInfo>,
    resources: Vec<manager::ResourceInfo>,
    providers: Vec<api::ProviderInfo>,
}
fn error_text(error: impl std::fmt::Display) -> String { error.to_string() }

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
    applications::catalog().map_err(error_text)
}
#[tauri::command]
fn install_ecosystem_application(app: tauri::AppHandle, app_id: String, mode: String) -> Result<(), String> {
    if QUEUE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Já existe um processamento em andamento. Aguarde antes de instalar outro aplicativo.".to_owned());
    }
    dependency_queue::begin_operation();
    std::thread::spawn(move || {
        match applications::install(&app_id, &mode) {
            Ok(()) if mode == "dev" => {
                match applications::dev_spec(&app_id).and_then(|(cwd, name, command)| {
                    process_manager::start_dev(app.clone(), app_id.clone(), name, cwd, command)
                        .map(|_| ())
                        .map_err(|error| anyhow::anyhow!(error))
                }) {
                    Ok(()) => { let _ = write_install_status("completed", &format!("{app_id} instalado e iniciado em DEV"), &app_id, None, serde_json::json!([{"id":app_id,"status":"completed","message":"Instalado e iniciado pelo Bash integrado."}])); }
                    Err(error) => { let _ = write_install_status("error", &format!("Instalado, mas não foi possível iniciar {app_id}"), &app_id, None, serde_json::json!([{"id":app_id,"status":"error","message":format!("{error:#}")}])); }
                }
            }
            Ok(()) => {}
            Err(error) => {
                let detail = format!("{error:#}");
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
fn open_ecosystem_application(app_id: String) -> Result<(), String> {
    applications::open(&app_id).map_err(error_text)
}

#[tauri::command]
fn dashboard_data() -> Result<Dashboard, String> {
    let root = manager::root().map_err(error_text)?;
    Ok(Dashboard {
        root: root.display().to_string(),
        applications: manager::applications().map_err(error_text)?,
        resources: manager::resources().map_err(error_text)?,
        providers: api::providers().map_err(error_text)?,
    })
}
#[tauri::command]
fn register_application(app_id: String, name: String, installation_path: String) -> Result<(), String> {
    manager::register_app_at_path(&app_id, &name, &installation_path).map_err(error_text)?;
    dependency_queue::enqueue_application(&app_id, &installation_path).map_err(error_text)
}
#[tauri::command]
fn delete_application(app_id: String) -> Result<(), String> {
    manager::unregister_application(&app_id).map_err(error_text)?;
    dependency_queue::remove_application(&app_id).map_err(error_text)
}
#[tauri::command]
fn synchronize_application(app_id: String) -> Result<(), String> {
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
        return Err("Já existe um processamento em andamento. Aguarde antes de iniciar outra instalação.".to_owned());
    }
    dependency_queue::begin_operation();
    if let Err(error) = write_install_status("installing", &format!("Preparando instalação de {resource_id}"), &resource_id, app_id.as_deref(), serde_json::json!([])) {
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
        return Err(error);
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
                let result = serde_json::json!([{"id":resource_id.clone(),"status":"error","message":format!("{error:#}")}]);
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
    manager::delete_resource(&dependency_id).map_err(error_text)
}
#[tauri::command]
fn start_dependency_queue() -> Result<(), String> {
    if QUEUE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("A fila de dependências já está em execução.".to_owned());
    }
    dependency_queue::begin_operation();
    std::thread::spawn(|| {
        if let Err(error) = dependency_queue::process_queue() {
            if dependency_queue::cancel_requested() {
                let _ = dependency_queue::mark_cancelled();
            } else {
                let _ = dependency_queue::set_failure(&format!("{error:#}"));
            }
        }
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(())
}
#[tauri::command]
fn cancel_processing() -> Result<(), String> {
    if !QUEUE_RUNNING.load(Ordering::SeqCst) {
        return Err("Não há processamento ativo para cancelar.".to_owned());
    }
    dependency_queue::request_cancel().map_err(error_text)
}
#[tauri::command]
fn restart_processing() -> Result<(), String> {
    if QUEUE_RUNNING.load(Ordering::SeqCst) {
        return Err("A operação atual ainda está encerrando. Aguarde antes de reiniciar.".to_owned());
    }
    let status = dependency_queue::status().map_err(error_text)?;
    if let Some(resource_id) = status.get("resourceId").and_then(serde_json::Value::as_str) {
        let app_id = status.get("appId").and_then(serde_json::Value::as_str).map(str::to_owned);
        spawn_install_shared_resource(resource_id.to_owned(), app_id)
    } else {
        start_dependency_queue()
    }
}
#[tauri::command]
fn dependency_queue_status() -> Result<serde_json::Value, String> {
    dependency_queue::status().map_err(error_text)
}
#[tauri::command]
fn voice_library() -> Result<serde_json::Value, String> {
    shared_runtime_manager::voices::list().map_err(error_text)
}
#[tauri::command]
fn create_voice_profile(name: String, filename: String, audio_base64: String, reference_text: String) -> Result<(), String> {
    if QUEUE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Já existe um processamento em andamento. Aguarde antes de preparar outra voz.".to_owned());
    }
    dependency_queue::begin_operation();
    if let Err(error) = write_voice_processing_status(
        "running",
        "Preparando o áudio de referência e executando o Qwen3-TTS Base...",
        None,
    ) {
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
        return Err(error);
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
                let detail = format!("{error:#}");
                let result = serde_json::json!({
                    "id": "Perfil de voz",
                    "status": "error",
                    "message": detail
                });
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
    shared_runtime_manager::voices::set_default(&voice_id).map_err(error_text)
}
#[tauri::command]
fn preview_voice(voice_id: String, text: String) -> Result<String, String> {
    shared_runtime_manager::voices::preview(&voice_id, &text).map_err(error_text)
}
#[tauri::command]
fn provider_catalog() -> Result<serde_json::Value, String> { api::provider_catalog().map_err(error_text) }
#[tauri::command]
fn available_api_models(provider: String) -> Result<Vec<String>, String> { api::available_models(&provider).map_err(error_text) }
#[tauri::command]
fn save_api_provider(provider: String, name: String, endpoint: String, model: String) -> Result<(), String> {
    api::add(&provider, &name, &endpoint, &model).map_err(error_text)
}
#[tauri::command]
fn authorize_api(provider: String, app_id: String) -> Result<(), String> {
    api::authorize(&provider, &app_id).map_err(error_text)
}
#[tauri::command]
fn revoke_api(provider: String, app_id: String) -> Result<(), String> {
    api::revoke(&provider, &app_id).map_err(error_text)
}
#[tauri::command]
fn save_api_key(provider: String, secret: String) -> Result<(), String> {
    api::set_key(&provider, &secret).map_err(error_text)
}
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            dashboard_data, applications_catalog, install_ecosystem_application, open_ecosystem_application, application_processes, stop_application_process, run_application_shell_command, register_application, delete_application, synchronize_application, delete_shared_dependency,
            start_dependency_queue, dependency_queue_status, install_shared_resource, cancel_processing, restart_processing,
            provider_catalog, available_api_models, save_api_provider, authorize_api,
            revoke_api, save_api_key, voice_library, create_voice_profile, set_default_voice, preview_voice
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Shared Runtime Manager");
}
