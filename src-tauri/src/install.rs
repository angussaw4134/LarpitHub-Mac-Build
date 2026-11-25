use std::{collections::HashSet, fs::File, io::{Read, Write}, path::{Path, PathBuf}, sync::Mutex};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, Window};
use tauri_plugin_dialog::DialogExt;
use tempfile::TempDir;
use crate::{binding::{problem, require, selected}, device::DeviceInfoMutex, error::AppError,
    sideload::{SideloaderGuard, SideloaderMutex}, BusyGuard};

struct Package { _folder: TempDir, archive: PathBuf, app_dir: String }
#[derive(Default)]
pub struct PackageState(Mutex<Option<Package>>);
#[derive(Serialize)]
pub struct PackageInfo { name: String, bundle: String, version: String }

fn valid_component(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let device_name = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|prefix| stem.strip_prefix(prefix)
            .is_some_and(|n| ["1","2","3","4","5","6","7","8","9","¹","²","³"].contains(&n)));
    !name.is_empty() && !name.ends_with([' ', '.']) && !device_name &&
        !name.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
}

fn inspect(path: &Path, extract: Option<&Path>) -> Result<(String, PackageInfo), AppError> {
    let invalid = || problem("Choose an iPhone IPA with one app and a valid Info.plist.");
    let file = File::open(path).map_err(|_| invalid())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| invalid())?;
    let mut info = None;
    let mut seen = HashSet::new();
    let mut total = 0u64;
    if zip.len() > 150_000 { return Err(problem("Too many entries in the IPA.")); }
    for index in 0..zip.len() {
        let mut item = zip.by_index(index).map_err(|_| invalid())?;
        let name = item.name().trim_end_matches('/').to_owned();
        if !name.starts_with("Payload/") { continue; }
        let parts: Vec<_> = name.split('/').collect();
        if item.enclosed_name().is_none() || parts.iter().any(|s| !valid_component(s)) ||
            !seen.insert(name.to_lowercase()) {
            return Err(problem("Unsupported file path inside the IPA."));
        }
        if item.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(problem("This Windows loader does not support archives containing symbolic links."));
        }
        if !parts[1].ends_with(".app") { return Err(invalid()); }
        total = total.saturating_add(item.size());
        if total > 8 * 1024 * 1024 * 1024 { return Err(problem("Unpacked IPA exceeds 8 GiB.")); }
        let metadata = parts.last() == Some(&"Info.plist");
        let mut data = Vec::new();
        if metadata {
            if item.size() > 4 * 1024 * 1024 { return Err(invalid()); }
            (&mut item).take(4 * 1024 * 1024 + 1).read_to_end(&mut data).map_err(|_| invalid())?;
            if data.len() as u64 != item.size() { return Err(invalid()); }
            let value = plist::Value::from_reader(std::io::Cursor::new(&data)).map_err(|_| invalid())?;
            if let Some(dict) = value.as_dictionary() {
                let field = |k| dict.get(k).and_then(plist::Value::as_string);
                if let Some(exe) = field("CFBundleExecutable") {
                    if !valid_component(exe) { return Err(invalid()); }
                }
                if parts.len() == 3 {
                    let bundle = field("CFBundleIdentifier").ok_or_else(invalid)?;
                    let exe = field("CFBundleExecutable").ok_or_else(invalid)?;
                    if info.is_some() { return Err(invalid()); }
                    info = Some((parts[1].to_owned(), exe.to_owned(), PackageInfo {
                        name: field("CFBundleDisplayName").or_else(||field("CFBundleName")).unwrap_or(bundle).to_owned(),
                        bundle: bundle.to_owned(), version: field("CFBundleShortVersionString").unwrap_or("").to_owned(),
                    }));
                }
            }
        }
        if let Some(root) = extract {
            let dest = root.join(&name);
            let err = |_| problem("Cannot unpack this IPA. Check available disk space.");
            if item.is_dir() { std::fs::create_dir_all(dest).map_err(err)?; continue; }
            std::fs::create_dir_all(dest.parent().ok_or_else(invalid)?).map_err(err)?;
            let mut out = File::create_new(dest).map_err(err)?;
            if metadata { out.write_all(&data).map_err(err)?; }
            else {
                let size = item.size();
                if std::io::copy(&mut item.take(size.saturating_add(1)), &mut out).map_err(err)? != size {
                    return Err(invalid());
                }
            }
        }
    }
    let (app, exe, info) = info.ok_or_else(invalid)?;
    if !seen.contains(&format!("Payload/{app}/{exe}").to_lowercase()) ||
       seen.iter().any(|s| !s.starts_with(&format!("payload/{}/",app.to_lowercase())) && s != &format!("payload/{}",app.to_lowercase())) {
        return Err(invalid());
    }
    Ok((app, info))
}

fn package_path(slot: u8) -> Result<PathBuf, AppError> {
    let filename = match slot {
        1 => "Application.ipa",
        2 => "Application-2.ipa",
        _ => return Err(problem("Select app 1 or app 2.")),
    };
    let executable = std::env::current_exe().map_err(|_| problem("Cannot locate Larpit Hub."))?;
    #[cfg(target_os = "macos")]
    {
        let contents = executable.parent().and_then(|p| p.parent())
            .ok_or_else(|| problem("Cannot locate Larpit Hub resources."))?;
        return Ok(contents.join("Resources").join(filename));
    }
    #[cfg(not(target_os = "macos"))]
    Ok(executable.with_file_name(filename))
}

#[tauri::command]
pub fn has_second_package() -> Result<bool, AppError> {
    Ok(package_path(2)?.is_file())
}

#[tauri::command]
pub fn has_bundled_demo() -> Result<bool, AppError> {
    Ok(cfg!(target_os = "macos") && package_path(1)?.is_file())
}

async fn pick_ipa(handle: &AppHandle) -> Result<Option<PathBuf>, AppError> {
    let mut dialog = handle.dialog().file()
        .set_title("Choose an iPhone app (.ipa)")
        .add_filter("iPhone application", &["ipa"]);
    if let Ok(downloads) = handle.path().download_dir() {
        if downloads.is_dir() {
            dialog = dialog.set_directory(downloads);
        }
    }
    let (send, receive) = tokio::sync::oneshot::channel();
    dialog.pick_file(move |choice| { let _ = send.send(choice); });
    let chosen = receive.await.map_err(|_| problem("The file picker closed unexpectedly."))?;
    chosen.map(|path| path.into_path().map_err(|_| problem("Select an IPA stored on this computer."))).transpose()
}

#[tauri::command]
pub async fn choose_package(slot: u8, handle: AppHandle, device: State<'_, DeviceInfoMutex>, session: State<'_, SideloaderMutex>, packages: State<'_, PackageState>) -> Result<Option<PackageInfo>, AppError> {
    let _busy = BusyGuard::take()?;
    let account = crate::sideload::signed_in_account(&session)?;
    require(&selected(&device)?, &account).await?;
    let source = match slot {
        0 => pick_ipa(&handle).await?,
        1 | 2 => {
            let bundled = package_path(slot)?;
            if bundled.is_file() {
                Some(bundled)
            } else if slot == 1 && !cfg!(target_os = "macos") {
                pick_ipa(&handle).await?
            } else {
                return Err(problem("The bundled IPA is missing. Choose an IPA from Downloads."));
            }
        }
        _ => return Err(problem("Choose a valid iPhone app.")),
    };
    let Some(source) = source else { return Ok(None); };
    let folder = tempfile::tempdir().map_err(|_| problem("Cannot create working folder."))?;
    let archive = folder.path().join("source.ipa");
    tokio::fs::copy(source, &archive).await.map_err(|_| problem("Cannot read the selected IPA."))?;
    let (app_dir, info) = inspect(&archive, None)?;
    *packages.0.lock().unwrap() = Some(Package { _folder: folder, archive, app_dir });
    Ok(Some(info))
}

#[tauri::command]
pub async fn install_package(window: Window, device: State<'_, DeviceInfoMutex>, packages: State<'_, PackageState>, session: State<'_, SideloaderMutex>) -> Result<(), AppError> {
    let _busy = BusyGuard::take()?;
    let phone = selected(&device)?;
    let mut session = SideloaderGuard::take(&session)?;
    let work = tempfile::tempdir().map_err(|_| problem("Cannot create working folder."))?;
    let path = {
        let guard = packages.0.lock().unwrap();
        let package = guard.as_ref().ok_or_else(|| problem("Choose an IPA first."))?;
        let _ = window.emit("install-stage", "Unpacking the IPA…");
        inspect(&package.archive, Some(work.path()))?;
        work.path().join("Payload").join(&package.app_dir)
    };
    let provider = require(&phone, session.account_id()).await?;
    let _ = window.emit("install-stage", "Signing and installing on the approved iPhone…");
    session.get_mut().install_app(&provider, path, false, None::<fn(f32) -> std::future::Ready<()>>, None).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn bundled_christmas_demo_passes_the_installers_ipa_check() {
        let archive = Path::new(env!("CARGO_MANIFEST_DIR")).join("mac-apps/WinterStudioDemo.ipa");
        let output = tempfile::tempdir().unwrap();
        let (app, metadata) = inspect(&archive, Some(output.path())).unwrap();
        assert_eq!(metadata.name, "3377 Christmas Demo");
        assert_eq!(metadata.bundle, "dev.accesshub.winterstudio.offline");
        assert!(output.path().join("Payload").join(app).join("Info.plist").is_file());
    }

    #[test]
    fn valid_ipa_metadata_and_extraction() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("example.ipa");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("Payload/Example.app/Info.plist", opts).unwrap();
        zip.write_all(br#"<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>app.example.demo</string><key>CFBundleExecutable</key><string>Example</string><key>CFBundleDisplayName</key><string>Example</string></dict></plist>"#).unwrap();
        zip.start_file("Payload/Example.app/Example", opts).unwrap();
        zip.write_all(b"fixture").unwrap();
        zip.finish().unwrap();
        let (name, metadata) = inspect(&path, None).unwrap();
        assert_eq!(name, "Example.app");
        assert_eq!(metadata.bundle, "app.example.demo");
        let unpack = tempfile::tempdir().unwrap();
        inspect(&path, Some(unpack.path())).unwrap();
        assert_eq!(std::fs::read(unpack.path().join("Payload/Example.app/Example")).unwrap(), b"fixture");
    }

    #[test]
    fn windows_path_components_are_validated() {
        for invalid in ["..", ".", "CON", "aux.txt", "file:stream", "file.", "a/b", "lpt9", "bad\\path"] {
            assert!(!valid_component(invalid));
        }
        assert!(valid_component("App resources.bundle"));
    }
}
