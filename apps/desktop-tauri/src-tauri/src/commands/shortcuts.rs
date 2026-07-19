//! Validation for a user-recorded global shortcut.
//!
//! The settings mutation owns the actual atomic shortcut swap. This command
//! lets the Settings UI surface an unavailable accelerator before saving it.

#[tauri::command]
pub fn validate_global_shortcut(app: tauri::AppHandle, accelerator: String) -> Result<(), String> {
    crate::shortcut_bridge::validate_shortcut(&app, &accelerator)
}
