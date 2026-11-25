use idevice::IdeviceError;
use isideload::SideloadError;
use rootcause::Report;
use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};

#[derive(Debug, thiserror::Error, Clone, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum AppError {
    #[error("{0}")]
    MaxApps(String),
    #[error("{0}")]
    NotEnoughAppIds(String),
    #[error("{0}")]
    DeviceComs(String),
    #[error("{0}")]
    InstallProhibited(String),
    #[error("{0}")]
    Underage(String),
    #[error("{0}")]
    AccountLocked(String),
    #[error("{0}")]
    Developer(String),
    #[error("{0}")]
    Auth(String),
    #[error("Not logged in")]
    NotLoggedIn,
    #[error("No device selected")]
    NoDeviceSelected,
    #[error("{0}")]
    Anisette(String),
    #[error("{0}")]
    Keyring(String),
    #[error("{0}")]
    Misc(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("AppError", 2)?;
        state.serialize_field("type", self.as_ref())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}

const INSTALL_PROHIBITED_MESSAGE: &str = "The iPhone refused app installation (InstallProhibited). Check whether Screen Time or a device management policy prohibits app installation. If this iPhone is managed, contact its administrator.";

fn is_install_prohibited(error: &IdeviceError) -> bool {
    // idevice preserves an unrecognized device code, optionally followed by the
    // device's ErrorDescription in parentheses. Match that code, not arbitrary
    // report text that might only mention it while describing a different error.
    matches!(error, IdeviceError::UnknownErrorType(code)
        if code == "InstallProhibited"
            || (code.starts_with("InstallProhibited (") && code.ends_with(')')))
}

// from rootcause report
impl From<Report> for AppError {
    fn from(report: Report) -> Self {
        let report_str = report.to_string();

        for cause in report.iter_reports() {
            if let Some(error) = cause.downcast_current_context::<reqwest::Error>() {
                if error.url().and_then(|u| u.host_str()).is_some_and(|h| matches!(h, "ani.jkcoxson.com" | "ani.iloader.app")) {
                    return AppError::Anisette("The authentication helper is unavailable. Try the alternative under Authentication helper.".into());
                }
                if error.is_timeout() {
                    return AppError::Auth("Apple authentication timed out. Check your connection, then try again.".into());
                }
                if let Some(status) = error.status() {
                    if matches!(status.as_u16(), 502 | 503 | 504) {
                        return AppError::Auth(format!("Apple authentication service is temporarily unavailable (HTTP {}). Please wait a few minutes and try again.", status.as_u16()));
                    }
                    if status.as_u16() == 429 {
                        return AppError::Auth("Apple is limiting sign-in attempts (HTTP 429). Wait before trying again.".into());
                    }
                }
            }
            if cause.downcast_current_context::<IdeviceError>().is_some_and(is_install_prohibited) {
                return AppError::InstallProhibited(INSTALL_PROHIBITED_MESSAGE.into());
            }
            if cause.downcast_current_context::<keyring::Error>().is_some() {
                return AppError::Keyring(report_str);
            }
            if let Some(err) = cause.downcast_current_context::<SideloadError>() {
                match err {
                    &SideloadError::AuthWithMessage(code, _) => match code {
                        -20209 => return AppError::AccountLocked(report_str),
                        _ => {
                            return AppError::Auth(report_str);
                        }
                    },
                    &SideloadError::DeveloperError(code, _) => match code {
                        1102 => return AppError::Underage(report_str),
                        _ => {
                            return AppError::Developer(report_str);
                        }
                    },
                    SideloadError::IdeviceError(idev_err) => match idev_err {
                        error if is_install_prohibited(error) => {
                            return AppError::InstallProhibited(INSTALL_PROHIBITED_MESSAGE.into());
                        }
                        IdeviceError::Socket(_) => {
                            return AppError::DeviceComs(report_str);
                        }
                        IdeviceError::ApplicationVerificationFailed(e) => {
                            if e.contains("maximum number of installed apps") {
                                return AppError::MaxApps(report_str);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            let cause_str = cause.to_string();
            if cause_str.contains("Not enough available app IDs") {
                return AppError::NotEnoughAppIds(report_str);
            }
            if cause_str.contains("Failed to get anisette data for login")
                || cause_str.contains("Failed to get anisette client info")
                || cause_str.contains("Failed to get anisette headers")
            {
                return AppError::Anisette(report_str);
            }
        }

        AppError::Misc(report_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apple_http_failures_have_actionable_messages_without_internal_traces() {
        use std::{io::{Read, Write}, net::TcpListener};
        for code in [502, 503, 504, 429] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/test", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut byte = [0]; let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
                write!(stream, "HTTP/1.1 {code} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            });
            let error = reqwest::Client::new().get(url).send().await.unwrap().error_for_status().unwrap_err();
            let report = Report::new(error).context("Internal source at private/path.rs:123").into_dynamic();
            let converted = AppError::from(report);
            assert!(matches!(converted, AppError::Auth(_)));
            assert!(converted.to_string().contains(&code.to_string()));
            assert!(!converted.to_string().contains("private/path"));
            server.join().unwrap();
        }
    }

    #[test]
    fn wrapped_device_install_prohibition_has_clear_help_without_internal_trace() {
        for code in ["InstallProhibited", "InstallProhibited (App installation is restricted)"] {
            let report = Report::new(SideloadError::IdeviceError(IdeviceError::UnknownErrorType(code.into())))
                .context("Failed at C:\\internal\\sideloader.rs:316")
                .context("Installing signed app")
                .into_dynamic();
            let error = AppError::from(report);
            assert!(matches!(error, AppError::InstallProhibited(_)));
            assert_eq!(error.to_string(), INSTALL_PROHIBITED_MESSAGE);
            let serialized = serde_json::to_value(&error).unwrap();
            assert_eq!(serialized["type"], "install_prohibited");
            assert!(!serialized["message"].as_str().unwrap().contains("sideloader.rs"));
        }
        let direct = Report::new(IdeviceError::UnknownErrorType("InstallProhibited".into())).into_dynamic();
        assert!(matches!(AppError::from(direct), AppError::InstallProhibited(_)));
    }

    #[test]
    fn unrelated_errors_and_mentions_are_not_install_prohibitions() {
        for code in ["ApplicationVerificationFailed", "InstallProhibitedElsewhere", "OtherError (InstallProhibited)"] {
            let report = Report::new(SideloadError::IdeviceError(IdeviceError::UnknownErrorType(code.into())))
                .context("Installing signed app").into_dynamic();
            assert!(matches!(AppError::from(report), AppError::Misc(_)));
        }
        let mention = Report::new(std::io::Error::other("See InstallProhibited documentation for a different error")).into_dynamic();
        assert!(matches!(AppError::from(mention), AppError::Misc(_)));
        let authentication = Report::new(SideloadError::AuthWithMessage(-1, "InstallProhibited".into())).into_dynamic();
        assert!(matches!(AppError::from(authentication), AppError::Auth(_)));
    }
}
