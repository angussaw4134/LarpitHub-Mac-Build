use std::sync::Mutex;
use isideload::{sideload::sideloader::Sideloader, util::callbacks::{ExtensionsBehaviorCallbackBox, MaxCertsCallbackBox}};
use crate::error::AppError;
type AppleSigner = Sideloader<MaxCertsCallbackBox, ExtensionsBehaviorCallbackBox>;
pub struct SignedAccount { pub signer: AppleSigner, pub deadline:i64, pub account_id: String }
pub type SideloaderMutex = Mutex<Option<SignedAccount>>;
pub fn signed_in_account(state: &SideloaderMutex) -> Result<String, AppError> {
    state.lock().unwrap().as_ref().filter(|v|v.deadline > chrono::Utc::now().timestamp()).map(|value|value.account_id.clone()).ok_or(AppError::NotLoggedIn)
}
pub struct SideloaderGuard<'a> { state:&'a SideloaderMutex, active: Option<SignedAccount> }
impl<'a> SideloaderGuard<'a> {
    pub fn take(state: &'a SideloaderMutex) -> Result<Self, AppError> {
        let active = state.lock().unwrap().take().filter(|v|v.deadline > chrono::Utc::now().timestamp()).ok_or(AppError::NotLoggedIn)?;
        Ok(Self {state,active:Some(active)})
    }
    pub fn account_id(&self) -> &str { &self.active.as_ref().expect("Active account").account_id }
    pub fn get_mut(&mut self) -> &mut AppleSigner { &mut self.active.as_mut().expect("Active account").signer }
}

impl Drop for SideloaderGuard<'_> { fn drop(&mut self) { *self.state.lock().unwrap() = self.active.take(); } }
