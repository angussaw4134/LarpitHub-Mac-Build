use std::sync::Mutex;
use idevice::{IdeviceService, lockdown::LockdownClient, provider::UsbmuxdProvider,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection}};
use serde::Serialize;
use tauri::State;
use crate::{binding::problem, error::AppError, BusyGuard};

#[derive(Clone, Serialize)]
pub struct DeviceInfo { pub name: String, pub udid: String }

pub type DeviceInfoMutex = Mutex<Option<DeviceInfo>>;

async fn usb() -> Result<UsbmuxdConnection, AppError> {
    UsbmuxdConnection::default().await.map_err(|_| problem("Open Apple Devices or iTunes on Windows, then reconnect your iPhone by USB."))
}

#[tauri::command]
pub async fn list_devices() -> Result<Vec<DeviceInfo>, AppError> {
    let mut conn = usb().await?;
    let devices = conn.get_devices().await.map_err(|_| problem("Cannot read the connected devices."))?;
    let addr = UsbmuxdAddr::from_env_var().map_err(|_| problem("Invalid USB service address."))?;
    let mut result = Vec::new();
    for device in devices.into_iter().filter(|d| matches!(d.connection_type, Connection::Usb)) {
        let provider = device.to_provider(addr.clone(), "AccessHub");
        let mut name = "USB device · unlock and tap Trust".to_owned();
        if let Ok(mut client) = LockdownClient::connect(&provider).await {
            if let Ok(class) = client.get_value(Some("DeviceClass"), None).await {
                if class.as_string() != Some("iPhone") { continue; }
            }
            if let Ok(value) = client.get_value(Some("DeviceName"), None).await {
                name = value.as_string().unwrap_or("iPhone").to_owned();
            }
        }
        result.push(DeviceInfo { name, udid: device.udid });
    }
    Ok(result)
}

#[tauri::command]
pub async fn select_phone(device: State<'_, DeviceInfoMutex>, udid: Option<String>) -> Result<(), AppError> {
    let _busy = BusyGuard::take()?;
    *device.lock().unwrap() = None;
    if let Some(id) = udid {
        let found = list_devices().await?.into_iter().find(|d| d.udid == id)
            .ok_or_else(|| problem("The selected iPhone is disconnected."))?;
        *device.lock().unwrap() = Some(found);
    }
    Ok(())
}

pub async fn get_provider(phone: &DeviceInfo) -> Result<UsbmuxdProvider, AppError> {
    let mut connection = usb().await?;
    let device = connection.get_devices().await.map_err(|_| problem("Could not refresh USB devices."))?
        .into_iter().find(|d| d.udid == phone.udid && matches!(d.connection_type, Connection::Usb))
        .ok_or_else(|| problem("Reconnect the selected iPhone over USB."))?;
    let address = UsbmuxdAddr::from_env_var().map_err(|_| problem("Invalid USB service address."))?;
    Ok(device.to_provider(address, "AccessHub"))
}
