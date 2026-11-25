use idevice::{lockdown::LockdownClient, provider::UsbmuxdProvider, IdeviceService};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::State;
use crate::{device::{get_provider, DeviceInfoMutex, DeviceInfo}, error::AppError};

pub fn problem(message: &str) -> AppError { AppError::Misc(message.into()) }
pub fn hash_id(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub fn selected(state: &DeviceInfoMutex) -> Result<DeviceInfo, AppError> {
    state.lock().unwrap().clone().ok_or(AppError::NoDeviceSelected)
}

async fn live_phone(device: &DeviceInfo) -> Result<(UsbmuxdProvider, String), AppError> {
    let provider = get_provider(device).await?;
    let mut phone = LockdownClient::connect(&provider).await
        .map_err(|_| problem("Reconnect the unlocked iPhone and tap Trust."))?;
    let kind = phone.get_value(Some("DeviceClass"), None).await
        .map_err(|_| problem("Unable to identify the connected iPhone."))?;
    if kind.as_string() != Some("iPhone") { return Err(problem("Select an iPhone to continue.")); }
    let id = phone.get_value(Some("UniqueDeviceID"), None).await
        .map_err(|_| problem("Could not read the connected iPhone identifier."))?;
    let udid = id.as_string().filter(|s| !s.is_empty() && s.eq_ignore_ascii_case(&device.udid))
        .ok_or_else(|| problem("The connected iPhone changed. Select it again."))?;
    Ok((provider, hash_id(&udid.to_ascii_lowercase())))
}

#[derive(Serialize, Deserialize)]
struct Binding { account: String }
impl Binding {
    fn check(&self, account: &str) -> Result<(), AppError> {
        if self.account != account { return Err(problem("This loader is linked to a different Apple ID. Sign in with the original Apple ID.")); }
        Ok(())
    }
}
fn entry() -> Result<keyring::Entry, AppError> {
    keyring::Entry::new("app.highzone.ipa-loader", "apple-account-and-iphone-v1")
        .map_err(|_| problem("Cannot open Windows Credential Manager."))
}
fn read_binding() -> Result<Option<Binding>, AppError> {
    match entry()?.get_password() {
        Ok(s) => serde_json::from_str(&s).map(Some).map_err(|_| problem("The saved Apple ID binding could not be read.")),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(problem("Cannot read the account binding in Windows Credential Manager.")),
    }
}
pub async fn preflight(device: &DeviceInfo) -> Result<(), AppError> {
    live_phone(device).await?;
    Ok(())
}
#[tauri::command]
pub async fn check_phone(device: State<'_, DeviceInfoMutex>) -> Result<(), AppError> {
    preflight(&selected(&device)?).await
}

// Call only after successful Apple authentication and developer session creation.
pub async fn enroll(device: &DeviceInfo, account: &str) -> Result<(), AppError> {
    live_phone(device).await?;
    let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(std::env::temp_dir().join("app.highzone.ipa-loader.binding.lock"))
        .map_err(|_| problem("Cannot open the account binding lock."))?;
    lock.lock().map_err(|_| problem("Cannot lock the account binding record."))?;
    if let Some(binding) = read_binding()? { return binding.check(account); }
    let record = serde_json::to_string(&Binding {account:account.into()})
        .map_err(|_| problem("Cannot encode the account binding."))?;
    entry()?.set_password(&record).map_err(|_| problem("Cannot save the Apple ID binding."))
}
pub async fn require(device: &DeviceInfo, account: &str) -> Result<UsbmuxdProvider, AppError> {
    let (provider, _) = live_phone(device).await?;
    read_binding()?.ok_or_else(|| problem("Sign in to your Apple ID first."))?.check(account)?;
    Ok(provider)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_enrolled_account_matches() {
        let binding = Binding {account:hash_id("apple-a")};
        assert!(binding.check(&hash_id("apple-a")).is_ok());
        assert!(binding.check(&hash_id("apple-b")).is_err());
    }
}
