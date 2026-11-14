//! The toast notifier (T-210; spec 001 US2, FR-027 / FR-028; contracts/messages.md
//! "Where"): [`ShellNotifier`], core's `Notifier` port over Windows toasts.
//!
//! `notify` only filters and posts: an offer whose id is on the toast surface
//! (`i18n::is_toast_surface`, every message but the overlay-only
//! `notice.logs_unwritable`) goes over an unbounded channel to the one thread named
//! `toast`, and `notify` returns without waiting (the session calls it from the hotkey,
//! worker, timer and logs-folder threads; `Show` is a COM call). That thread runs under
//! the shared COM MTA guard ([`super::ComGuard`]), renders the message with `i18n::text`
//! in the `ui_language` of the settings snapshot taken at that toast, builds the toast
//! XML through the WinRT DOM (`XmlDocument`: the text is a text node, so a device name
//! holding `<` or `&` cannot change the toast; no action, no launch argument: the Retry
//! button is T-213's) and hands it to the [`ToastSink`] port. The shipped sink is
//! [`WinRtSink`] (`ToastNotificationManager::CreateToastNotifierWithId` with the app's
//! AUMID, which the installed Start-menu shortcut carries); tests inject a fake.
//!
//! Failures change nothing else (the overlay and the tray are set by core regardless):
//! a toast that cannot be built or shown writes one `warning kind=toast_failed
//! os_code=N` line (the Win32 code or HRESULT, never the OS text or the XML) and the next
//! offer is still shown; a toast thread that cannot start, or whose COM init fails,
//! writes one such line and the offers are dropped. With the app's notifications off
//! `Show` does not fail, so [`WinRtSink`] first passes `ToastNotifier::Setting()`
//! through [`setting_gate`]: a toast refused by the setting writes the same one line
//! per refused toast, `os_code=1223` ([`REFUSED_OS_CODE`], `ERROR_CANCELLED`).

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::i18n::{self, MessageId};
use voicen_core::platform::{Notifier, PendingId};
use voicen_core::settings::service::SettingsService;
use windows::core::{h, Error as WinError, HRESULT, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager,
};

use super::{os_code, ComGuard};

/// The app's AppUserModelID: `identifier` of `tauri.conf.json`, set on the installed
/// Start-menu shortcut (`SetLnkAppUserModelId` in the NSIS installer).
pub const APP_USER_MODEL_ID: &str = "dev.voicen.app";

/// The `os_code` logged for a toast refused by the notification setting:
/// `ERROR_CANCELLED`, the code of [`setting_gate`]'s error.
pub const REFUSED_OS_CODE: i32 = 1223;

/// Lets a toast through only when the app's notification setting is `Enabled`; every
/// other value (off for the app or the user, group policy, manifest) is refused with
/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`, logged as `os_code=1223`.
pub fn setting_gate(setting: NotificationSetting) -> Result<(), WinError> {
    if setting == NotificationSetting::Enabled {
        Ok(())
    } else {
        Err(WinError::from_hresult(HRESULT::from_win32(
            ERROR_CANCELLED.0,
        )))
    }
}

/// The toast thread's name.
const THREAD_NAME: &str = "toast";

/// Shows one toast from its XML. Called only on the toast thread, one call at a time;
/// it may block (a COM call). `Err` carries the failed call's error.
pub trait ToastSink: Send + Sync {
    fn show(&self, xml: &str) -> Result<(), WinError>;
}

/// Starts a named thread running `body` (`std::thread::Builder` in the app; tests
/// refuse to check the failure branch).
pub type SpawnFn = fn(&str, Box<dyn FnOnce() + Send>) -> io::Result<()>;

/// One offer for the toast thread.
type Offer = (MessageId, Vec<(&'static str, String)>);

/// The shell's `Notifier`: posts each toast-surface offer to the one toast thread.
pub struct ShellNotifier {
    /// `None` when the toast thread could not be started: the offers are dropped.
    offers: Option<Sender<Offer>>,
}

impl ShellNotifier {
    /// The notifier over `sink`, its toast thread started by `std::thread::Builder`.
    pub fn new(
        settings: Arc<SettingsService>,
        log: Arc<Log>,
        sink: Arc<dyn ToastSink>,
    ) -> ShellNotifier {
        ShellNotifier::with_spawn(settings, log, sink, spawn_thread)
    }

    /// The notifier over `sink`, its toast thread started by `spawn`. A failed spawn
    /// writes one `toast_failed` line here and the notifier drops every offer.
    pub fn with_spawn(
        settings: Arc<SettingsService>,
        log: Arc<Log>,
        sink: Arc<dyn ToastSink>,
        spawn: SpawnFn,
    ) -> ShellNotifier {
        let (sender, receiver) = mpsc::channel();
        let thread_log = Arc::clone(&log);
        let body = Box::new(move || run(&receiver, &settings, &thread_log, sink.as_ref()));
        match spawn(THREAD_NAME, body) {
            Ok(()) => ShellNotifier {
                offers: Some(sender),
            },
            Err(err) => {
                failed(&log, err.raw_os_error());
                ShellNotifier { offers: None }
            }
        }
    }
}

impl Notifier for ShellNotifier {
    fn notify(&self, id: MessageId, params: &[(&'static str, String)], _retry: Option<PendingId>) {
        if !i18n::is_toast_surface(id) {
            return;
        }
        if let Some(offers) = &self.offers {
            // Unbounded: never waits. A send fails only once the thread has ended
            // (its COM init failed, already logged): the offer is dropped.
            let _ = offers.send((id, params.to_vec()));
        }
    }
}

/// The app's spawner: a named `std` thread, detached.
fn spawn_thread(name: &str, body: Box<dyn FnOnce() + Send>) -> io::Result<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(body)
        .map(drop)
}

/// The toast thread: COM, then one toast per offer until the notifier is dropped.
fn run(offers: &Receiver<Offer>, settings: &SettingsService, log: &Log, sink: &dyn ToastSink) {
    let _com = match ComGuard::init() {
        Ok(com) => com,
        Err(hr) => {
            failed(log, Some(os_code(&WinError::from_hresult(hr))));
            return;
        }
    };
    while let Ok((id, params)) = offers.recv() {
        let lang = settings.snapshot().ui_language;
        let args: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let text = i18n::text(lang, id, &args);
        if let Err(err) = toast_xml(&text).and_then(|xml| sink.show(&xml)) {
            failed(log, Some(os_code(&err)));
        }
    }
}

/// The toast XML of one text line, built through the DOM: `<toast><visual><binding
/// template="ToastGeneric"><text>…</text></binding></visual></toast>`; `text` is a text
/// node, escaped by the serializer.
fn toast_xml(text: &str) -> Result<String, WinError> {
    let doc = XmlDocument::new()?;
    let toast = doc.CreateElement(h!("toast"))?;
    let visual = doc.CreateElement(h!("visual"))?;
    let binding = doc.CreateElement(h!("binding"))?;
    binding.SetAttribute(h!("template"), h!("ToastGeneric"))?;
    let line = doc.CreateElement(h!("text"))?;
    line.AppendChild(&doc.CreateTextNode(&HSTRING::from(text))?)?;
    binding.AppendChild(&line)?;
    visual.AppendChild(&binding)?;
    toast.AppendChild(&visual)?;
    doc.AppendChild(&toast)?;
    Ok(doc.GetXml()?.to_string_lossy())
}

/// The one failure line: the kind and the code, nothing else.
fn failed(log: &Log, os_code: Option<i32>) {
    log.write(LogEvent::Warning {
        kind: WarningKind::ToastFailed,
        os_code,
    });
}

/// The shipped sink: the XML loaded into a toast and shown by the toast notifier of
/// [`APP_USER_MODEL_ID`], after the notifier's `Setting()` passed [`setting_gate`].
/// Without the installed shortcut (a dev build, a `/NS` install, a test exe)
/// `CreateToastNotifierWithId`, `Setting` or `Show` fails, and a setting other than
/// `Enabled` is refused: the failure branch.
pub struct WinRtSink;

impl ToastSink for WinRtSink {
    fn show(&self, xml: &str) -> Result<(), WinError> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID))?;
        setting_gate(notifier.Setting()?)?;
        notifier.Show(&toast)
    }
}
