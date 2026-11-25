use isideload::util::{keyring_storage::KeyringStorage, storage::SideloadingStorage};
use tauri::AppHandle;
use crate::error::AppError;
const SERVICE: &str = "app.highzone.ipa-loader";
pub fn create_sideloading_storage(_app: &AppHandle) -> Result<Box<dyn SideloadingStorage>, AppError> {
    // No plaintext fallback and no shared credential namespace with other loaders.
    Ok(Box::new(KeyringStorage::new(SERVICE.into())))
}

