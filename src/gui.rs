#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde::Serialize;
use shared_runtime_manager::{api, dependency_manager, manager};

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
    dependency_manager::provision_application(&app_id, &installation_path).map_err(error_text)
}
#[tauri::command]
fn install_shared_resource(resource: String, app_id: Option<String>) -> Result<(), String> {
    manager::install(&resource, app_id.as_deref()).map_err(error_text)
}
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
            dashboard_data, register_application, install_shared_resource,
            save_api_provider, authorize_api, revoke_api, save_api_key
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Shared Runtime Manager");
}
