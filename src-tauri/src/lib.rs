mod account;
mod device;
mod error;

mod secure_storage;
mod sideload;
mod install;
mod binding;

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;
static BUSY: AtomicBool = AtomicBool::new(false);
pub(crate) struct BusyGuard;
impl BusyGuard {
    pub fn take() -> Result<Self, error::AppError> {
        BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| error::AppError::Misc("Another operation is already running.".into()))?;
        Ok(Self)
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) { BUSY.store(false, Ordering::SeqCst); }
}
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init()).setup(|app| {
            app.manage(device::DeviceInfoMutex::new(None));
            app.manage(install::PackageState::default());
            app.manage(sideload::SideloaderMutex::new(None));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            binding::check_phone,
            account::restore_apple, account::login_apple, account::logout_apple,
            device::list_devices, device::select_phone,
            install::has_second_package, install::has_bundled_demo,
            install::choose_package, install::install_package
        ])
        .run(tauri::generate_context!())
        .expect("Could not start Highzone");
}
