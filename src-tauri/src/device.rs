use std::sync::Mutex;
use idevice::{IdeviceService, lockdown::LockdownClient, provider::UsbmuxdProvider,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdDevice}};
use serde::Serialize;
use tauri::State;
use tokio::time::{Duration, timeout};
use crate::{binding::problem, error::AppError, BusyGuard};

#[derive(Clone, Serialize)]
pub struct DeviceInfo { pub name: String, pub udid: String }

pub type DeviceInfoMutex = Mutex<Option<DeviceInfo>>;

const USB_TIMEOUT: Duration = Duration::from_secs(5);
const DETAILS_TIMEOUT: Duration = Duration::from_secs(2);

async fn wired_devices() -> Result<(UsbmuxdAddr, Vec<UsbmuxdDevice>), AppError> {
    let address = UsbmuxdAddr::from_env_var().map_err(|_| problem("Invalid USB service address."))?;
    let unavailable = || problem(if cfg!(target_os = "macos") {
        "The Mac's iPhone connection is unavailable. Check that your unlocked iPhone appears in Finder, then click Refresh."
    } else {
        "Open Apple Devices or iTunes on Windows, then reconnect your iPhone by USB."
    });
    let mut connection = timeout(USB_TIMEOUT, address.connect(0)).await
        .map_err(|_| unavailable())?.map_err(|_| unavailable())?;
    let scan_error = || problem("Could not read connected iPhones. Reconnect the unlocked iPhone and click Refresh.");
    let mut devices = timeout(USB_TIMEOUT, connection.get_devices()).await
        .map_err(|_| scan_error())?.map_err(|_| scan_error())?;
    devices.retain(|device| matches!(device.connection_type, Connection::Usb));
    Ok((address, devices))
}

#[tauri::command]
pub async fn list_devices() -> Result<Vec<DeviceInfo>, AppError> {
    let (address, devices) = wired_devices().await?;
    let mut result = Vec::new();
    for device in devices {
        let provider = device.to_provider(address.clone(), "AccessHub");
        let mut name = "USB device · unlock and tap Trust".to_owned();
        // A connected phone may not expose details until it is unlocked or trusted.
        if let Ok(Ok(Some((class, device_name)))) = timeout(DETAILS_TIMEOUT, async {
            let mut client = LockdownClient::connect(&provider).await?;
            let class = client.get_value(Some("DeviceClass"), None).await?;
            let Some(class) = class.as_string() else { return Ok(None); };
            if class != "iPhone" {
                return Ok(Some((class.to_owned(), None)));
            }
            let name = client.get_value(Some("DeviceName"), None).await.ok()
                .and_then(|value| value.as_string().map(str::to_owned));
            Ok::<_, idevice::IdeviceError>(Some((class.to_owned(), name)))
        }).await {
            if class != "iPhone" { continue; }
            name = device_name.unwrap_or_else(|| "iPhone".to_owned());
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
    let (address, devices) = wired_devices().await?;
    let device = devices.into_iter().find(|d| d.udid == phone.udid)
        .ok_or_else(|| problem("Reconnect the selected iPhone over USB."))?;
    Ok(device.to_provider(address, "AccessHub"))
}
