#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use shared_runtime_manager::{api, dependency_queue, manager};

static QUEUE_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
struct Dashboard {
    root: String,
    applications: Vec<manager::ApplicationInfo>,
    resources: Vec<manager::ResourceInfo>,
    providers: Vec<api::ProviderInfo>,
}
fn error_text(error: impl std::fmt::Display) -> String { error.to_string() }

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
    let app = manager::applications().map_err(error_text)?
        .into_iter().find(|item| item.id == app_id)
        .ok_or_else(|| format!("Application '{app_id}' is not registered."))?;
    if app.installation_path.is_empty() {
        return Err(format!("Application '{app_id}' has no installation path."));
    }
    dependency_queue::enqueue_application(&app_id, &app.installation_path).map_err(error_text)?;
    start_dependency_queue()
}
#[tauri::command]
fn delete_shared_dependency(dependency_id: String) -> Result<(), String> {
    manager::delete_resource(&dependency_id).map_err(error_text)
}
#[tauri::command]
fn start_dependency_queue() -> Result<(), String> {
    if QUEUE_RUNNING.swap(true, Ordering::SeqCst) { return Ok(()); }
    std::thread::spawn(|| {
        if let Err(error) = dependency_queue::process_queue() {
            let _ = dependency_queue::set_failure(&format!("{error:#}"));
        }
        QUEUE_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(())
}
#[tauri::command]
fn dependency_queue_status() -> Result<serde_json::Value, String> {
    dependency_queue::status().map_err(error_text)
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
            dashboard_data, register_application, delete_application, synchronize_application, delete_shared_dependency,
            start_dependency_queue, dependency_queue_status,
            provider_catalog, available_api_models, save_api_provider, authorize_api,
            revoke_api, save_api_key
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Shared Runtime Manager");
}
