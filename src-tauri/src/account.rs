use futures::FutureExt;
use isideload::{
    anisette::{AnisetteClientInfo, remote_v3::RemoteV3AnisetteProvider},
    auth::apple_account::{AppleAccount, TwoFactorCallbackParams, TwoFactorCallbackResponse},
    dev::{certificates::DevelopmentCertificate, developer_session::DeveloperSession},
    sideload::{SideloaderBuilder, builder::{ExtensionsBehavior, ExtensionsBehaviorSelection, MaxCertsBehavior}},
    util::callbacks::MaxCertsCallbackBox,
};
use rootcause::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use std::{sync::{Arc, Mutex}, time::Duration};
use tauri::{AppHandle, Emitter, Listener, State, Window};
use crate::{BusyGuard, error::AppError, secure_storage::create_sideloading_storage, sideload::SideloaderMutex};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Prompt<T: Serialize + Clone> { request_id: String, params: T }

async fn prompt<T, R>(window: Window, event: &str, params: T) -> Result<R, Report>
where T: Serialize + Clone, R: DeserializeOwned + Send + 'static {
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<R>();
    let sender = Arc::new(Mutex::new(Some(tx)));
    // Listen before notifying the UI, and always remove the listener on timeout/error.
    let handler = window.listen(format!("{event}-{request_id}"), move |event| {
        if let Ok(value) = serde_json::from_str::<R>(event.payload()) {
            if let Some(tx) = sender.lock().unwrap().take() { let _ = tx.send(value); }
        }
    });
    let sent = window.emit(event, Prompt { request_id, params });
    let result = match sent {
        Ok(()) => match tokio::time::timeout(Duration::from_secs(180), rx).await {
            Ok(Ok(value)) => Ok(value),
            _ => Err(report!("The confirmation timed out. Please try signing in again.")),
        },
        Err(_) => Err(report!("Could not open the confirmation prompt.")),
    };
    window.unlisten(handler);
    result
}

fn anisette_url(server: &str) -> Result<&'static str, AppError> {
    match server {
        "primary" => Ok("https://ani.jkcoxson.com"),
        "alternative" => Ok("https://ani.iloader.app"),
        _ => Err(AppError::Anisette("Unknown authentication provider.".into())),
    }
}

fn account_builder(handle: &AppHandle,email: &str,server: &str) -> Result<isideload::auth::builder::AppleAccountBuilder, AppError> {
    Ok(AppleAccount::builder(email)
        // Respect Apple's rate-limit response instead of immediately retrying ten times.
        .err_429_retries(None)
        .anisette_provider(RemoteV3AnisetteProvider::default()?
            .set_serial_number("0".into())
            .set_storage(create_sideloading_storage(handle)?)
            // iLoader v2.4.2 (7364ae6): restore older client info for Apple HTTP 503.
            .set_client_info_override(Some(AnisetteClientInfo {
                client_info: "<MacBookPro13,3> <Mac OS X;10.12.6;16G29> <com.apple.AuthKit/1 (com.apple.akd/1.0)>".into(),
                user_agent: "akd/1.0 CFNetwork/811.5.4 Darwin/16.7.0".into(),
            }))
            .set_url(anisette_url(server)?)))
}

#[derive(serde::Deserialize, Serialize)]
struct SavedSession {
    email: String, server: String, adsid: String, token: String, deadline: i64,
}
fn session_entry() -> Result<keyring::Entry,AppError> {
    keyring::Entry::new("app.highzone.ipa-loader","verified-session-v1").map_err(|e| AppError::Keyring(e.to_string()))
}
fn forget() -> Result<(), AppError> {
    match session_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Keyring(e.to_string())),
    }
}
#[tauri::command]
pub async fn restore_apple(handle: AppHandle, window: Window, device: State<'_, crate::device::DeviceInfoMutex>, session: State<'_, SideloaderMutex>) -> Result<bool,AppError> {
    let _busy = BusyGuard::take()?;
    let raw = match session_entry()?.get_secret() {
        Ok(raw) => raw, Err(keyring::Error::NoEntry) => return Ok(false),
        Err(e) => return Err(AppError::Keyring(e.to_string())),
    };
    let saved: SavedSession = serde_json::from_slice(&raw).map_err(|_| AppError::Auth("Saved session is unreadable. Sign in again.".into()))?;
    if chrono::Utc::now().timestamp() >= saved.deadline { forget()?; return Ok(false); }
    let id = crate::binding::hash_id(&saved.adsid);
    let phone = crate::binding::selected(&device)?;
    crate::binding::require(&phone,&id).await?;
    let account = account_builder(&handle,&saved.email,&saved.server)?.build().await?;
    let developer = DeveloperSession::new(isideload::auth::apple_account::AppToken { token:saved.token, duration:0, expiry:0 }, saved.adsid, account.grandslam_client, account.anisette_generator);
    activate(&handle,window,&session,developer,saved.email,id,saved.deadline)?;
    Ok(true)
}

#[tauri::command]
pub async fn login_apple(handle: AppHandle, window: Window,
    device: State<'_, crate::device::DeviceInfoMutex>, session: State<'_, SideloaderMutex>, email: String, password: String, server: String,
) -> Result<(), AppError> {
    let _busy = BusyGuard::take()?;
    let phone = crate::binding::selected(&device)?;
    crate::binding::preflight(&phone).await?;
    *session.lock().unwrap() = None;
    let email = email.trim().to_lowercase();
    if email.is_empty() || password.is_empty() { return Err(AppError::Auth("Enter your Apple Account and password.".into())); }
    // Credentials are not saved, logged or sent to the AccessHub service.
    let tfa_window = window.clone();
    let two_factor = move |params: TwoFactorCallbackParams| {
        let window = tfa_window.clone();
        async move {
            let response = prompt::<_, TwoFactorCallbackResponse>(window, "accesshub-2fa", params).await?;
            Ok(response)
        }.boxed()
    };
    let mut account = account_builder(&handle, &email, &server)?
        .login(&password, Box::new(two_factor)).await?;
    drop(password);
    let token = account.get_app_token("xcode.auth").await?;
    let stable_id = account.spd.as_ref().and_then(|d| d.get("adsid")).and_then(plist::Value::as_string).filter(|s| !s.is_empty()).ok_or_else(|| crate::binding::problem("Apple did not return a verified account identifier."))?;
    let account_id = crate::binding::hash_id(stable_id);
    let deadline = chrono::Utc::now().timestamp() + 48 * 3600;
    let saved = SavedSession {email:email.clone(), server, adsid:stable_id.to_owned(),token:token.token.clone(),deadline};
    let developer = DeveloperSession::new(token, saved.adsid.clone(), account.grandslam_client, account.anisette_generator);
    crate::binding::enroll(&phone, &account_id).await?;
    let bytes = serde_json::to_vec(&saved).map_err(|_| AppError::Auth("Could not save session.".into()))?;
    session_entry()?.set_secret(&bytes).map_err(|e|AppError::Keyring(e.to_string()))?;
    activate(&handle,window,&session,developer,email,account_id,deadline)
}
fn activate(handle:&AppHandle,window:Window,session:&SideloaderMutex,developer:DeveloperSession,email:String,account_id:String,deadline:i64) -> Result<(),AppError> {
    let max_certs: MaxCertsCallbackBox = Box::new(move |certs: Vec<DevelopmentCertificate>| {
        let window = window.clone();
        // Replacing certificates can affect other apps; explicit user selection is required.
        let choices: Vec<_> = certs.into_iter().filter_map(|c| c.serial_number.map(|serial|
            serde_json::json!({"serial": serial, "name": c.machine_name.or(c.name).unwrap_or("Development certificate".into())})
        )).collect();
        async move {
            let allowed: Vec<String> = choices.iter().filter_map(|v| v["serial"].as_str().map(str::to_owned)).collect();
            let answer = prompt::<_, Option<Vec<String>>>(window, "accesshub-certificates", choices).await?;
            if let Some(ref selected) = answer {
                if selected.is_empty() || selected.iter().any(|id| !allowed.contains(id)) {
                    bail!("Select a certificate offered by Apple, or cancel.");
                }
            }
            Ok(answer)
        }.boxed()
    });
    let sideloader = SideloaderBuilder::new(developer, email)
        .machine_name("Larpit Hub".into())
        .storage(create_sideloading_storage(&handle)?)
        .extensions_behavior(ExtensionsBehaviorSelection::Fixed(ExtensionsBehavior::ReuseMain))
        .max_certs_behavior(MaxCertsBehavior::Prompt(max_certs)).build();
    *session.lock().unwrap() = Some(crate::sideload::SignedAccount { signer: sideloader, account_id, deadline });
    Ok(())
}

#[tauri::command]
pub fn logout_apple(session: State<'_, SideloaderMutex>) -> Result<(), AppError> {
    let _busy = BusyGuard::take()?;
    *session.lock().unwrap() = None;
    forget()
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn arbitrary_auth_servers_are_rejected() {
        assert!(anisette_url("http://example.com").is_err());
        assert!(anisette_url("primary").unwrap().starts_with("https://"));
    }
}
