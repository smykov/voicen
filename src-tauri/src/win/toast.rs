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
//! holding `<` or `&` cannot change the toast; no launch argument) and hands it to the
//! [`ToastSink`] port. The shipped sink is
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
//!
//! Buttons (T-213; spec 001 FR-027, spec 007 FR-030; decisions #274 Q5, #279, #298, #312,
//! #313). This module is the one place a toast button is built: an offer's
//! `ToastAction` becomes one `<action content=… arguments=… activationType="foreground"/>`,
//! its arguments only `ToastAction::to_toast_arg`'s output and its label `action.retry` /
//! `action.open` in the same `ui_language`, and only once the activator is registered in
//! this process ([`ShellNotifier::attach`]: posted to the toast thread, which registers an
//! `IClassFactory` for [`TOAST_ACTIVATOR_CLSID`] through `CoRegisterClassObject`
//! (`CLSCTX_LOCAL_SERVER`, `REGCLS_MULTIPLEUSE`) under its MTA guard). Before attach, or
//! after a failed registration (one `toast_failed` line with the HRESULT, never retried),
//! toasts show without buttons: never a dead button. A click, from the popup or from
//! Action Center, reaches the app only through the activator's `Activate`, on a COM
//! thread: it copies the argument, stamps the instant, parses it only with
//! `ToastArg::parse` and hands it to the [`ToastRoute`] without taking the session lock or
//! panicking, and answers `S_OK` whatever the argument. Retry(id) runs
//! `DictationSession::retry(at, Some(id))` on a blocking task (core runs nothing for an id
//! that is not the pending one: ids rise across processes, I11); Open(call) runs
//! `call::open_calls(app, Some(CallRef { root, id }))` with the root the toast thread
//! recorded when it showed that call's Open toast, and nothing for a call this process did
//! not offer (an Open clicked after a restart only starts the app, #312). The installer
//! points the Start-menu shortcut's `ToastActivatorCLSID` and the HKCU `LocalServer32` key
//! at this exe (`installer-hooks.nsh`, `!define VOICEN_TOAST_CLSID`, equal to the const).

use std::collections::HashMap;
use std::ffi::c_void;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use tauri::{AppHandle, Manager, Runtime};
use voicen_core::call::storage::wire_root;
use voicen_core::call::CallId;
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::dictation::DictationSession;
use voicen_core::i18n::{self, MessageId};
use voicen_core::platform::{Notifier, PendingId, ToastAction, ToastArg};
use voicen_core::settings::service::SettingsService;
use windows::core::{
    h, implement, Error as WinError, IUnknown, Interface, Ref, BOOL, GUID, HRESULT, HSTRING, PCWSTR,
};
use windows::Data::Xml::Dom::{XmlDocument, XmlElement};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, ERROR_CANCELLED, E_POINTER};
use windows::Win32::System::Com::{
    CoRegisterClassObject, IClassFactory, IClassFactory_Impl, CLSCTX_LOCAL_SERVER,
    REGCLS_MULTIPLEUSE,
};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager,
};

use crate::call::{self, CallRef};

use super::{os_code, ComGuard};

/// The app's AppUserModelID: `identifier` of `tauri.conf.json`, set on the installed
/// Start-menu shortcut (`SetLnkAppUserModelId` in the NSIS installer).
pub const APP_USER_MODEL_ID: &str = "dev.voicen.app";

/// The toast activator's class id (T-213 I7): registered by [`register_class_object`],
/// and the one `!define VOICEN_TOAST_CLSID` of `src-tauri/windows/installer-hooks.nsh`
/// (the shortcut's `System.AppUserModel.ToastActivatorCLSID` and
/// `HKCU\Software\Classes\CLSID\{…}\LocalServer32`) is this value.
pub const TOAST_ACTIVATOR_CLSID: GUID = GUID::from_u128(0x163266b0_4b55_446a_bbbe_e31cac2c713a);

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

/// Registers the activator's class factory with COM (the shipped
/// [`register_class_object`]; tests inject a fake or a failing one).
pub type RegisterFn = fn(&IClassFactory) -> Result<(), WinError>;

/// The shipped [`RegisterFn`]: `CoRegisterClassObject(TOAST_ACTIVATOR_CLSID, factory,
/// CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)`, never revoked (the registration lives as
/// long as the toast thread's apartment, the process).
pub fn register_class_object(factory: &IClassFactory) -> Result<(), WinError> {
    // SAFETY: a plain COM call with a live class object; COM keeps its own reference.
    unsafe {
        CoRegisterClassObject(
            &TOAST_ACTIVATOR_CLSID,
            factory,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE,
        )
    }
    .map(drop)
}

/// One parsed, checked toast click.
enum Activation {
    Retry { at: Instant, id: PendingId },
    Open { call: CallId, root: PathBuf },
}

/// Where a toast click goes ([`app_route`] in the app). Called on a COM thread: it must
/// not block and must not take the session lock.
pub struct ToastRoute(Box<dyn Fn(Activation) + Send + Sync>);

/// The app's route: Retry -> `DictationSession::retry(at, Some(id))` on a blocking task,
/// on the session managed as `Arc<DictationSession>` (nothing without one), the tray
/// Retry's shape; Open -> `call::open_calls(app, Some(CallRef { root: wire_root(root),
/// id }))`, which builds or focuses the Calls window and selects the call.
pub fn app_route<R: Runtime>(app: &AppHandle<R>) -> ToastRoute {
    let app = app.clone();
    ToastRoute(Box::new(move |activation| match activation {
        Activation::Retry { at, id } => {
            let Some(session) = app.try_state::<Arc<DictationSession>>() else {
                return;
            };
            let session = Arc::clone(&session);
            drop(tauri::async_runtime::spawn_blocking(move || {
                session.retry(at, Some(id))
            }));
        }
        Activation::Open { call, root } => call::open_calls(
            &app,
            Some(CallRef {
                root: wire_root(&root),
                id: call.to_string(),
            }),
        ),
    }))
}

/// What a registered activator shares with the toast thread: the route and the root of
/// every call whose Open toast this process showed.
struct Buttons {
    route: ToastRoute,
    offered: Mutex<HashMap<CallId, PathBuf>>,
}

impl Buttons {
    /// One click: the argument through the one codec; Open only for an offered call.
    fn activate(&self, at: Instant, arg: &str) {
        match ToastArg::parse(arg) {
            Some(ToastArg::Retry(id)) => (self.route.0)(Activation::Retry { at, id }),
            Some(ToastArg::Open(call)) => {
                let root = self
                    .offered
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&call)
                    .cloned();
                if let Some(root) = root {
                    (self.route.0)(Activation::Open { call, root });
                }
            }
            None => {}
        }
    }
}

/// The activator: the object a toast click reaches through COM.
#[implement(INotificationActivationCallback)]
struct Activator {
    buttons: Arc<Buttons>,
}

impl INotificationActivationCallback_Impl for Activator_Impl {
    /// Copies the argument (valid only during the call), stamps the instant and hands
    /// it on; never blocks on the session, never unwinds into COM, always `S_OK`.
    fn Activate(
        &self,
        _appusermodelid: &PCWSTR,
        invokedargs: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows::core::Result<()> {
        let at = Instant::now();
        let arg = if invokedargs.is_null() {
            String::new()
        } else {
            // SAFETY: a non-null argument from COM is a NUL-terminated UTF-16 string,
            // valid for the duration of the call; it is copied here.
            String::from_utf16_lossy(unsafe { invokedargs.as_wide() })
        };
        let _ = catch_unwind(AssertUnwindSafe(|| self.buttons.activate(at, &arg)));
        Ok(())
    }
}

/// The activator's class factory, the object registered with COM.
#[implement(IClassFactory)]
struct ActivatorFactory {
    buttons: Arc<Buttons>,
}

impl IClassFactory_Impl for ActivatorFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if ppvobject.is_null() || riid.is_null() {
            return Err(WinError::from_hresult(E_POINTER));
        }
        // SAFETY: checked non-null; COM's out pointer is cleared before any answer.
        unsafe { *ppvobject = std::ptr::null_mut() };
        if !punkouter.is_null() {
            return Err(WinError::from_hresult(CLASS_E_NOAGGREGATION));
        }
        let activator: INotificationActivationCallback = Activator {
            buttons: Arc::clone(&self.buttons),
        }
        .into();
        // SAFETY: `riid` and `ppvobject` are COM's valid in/out pointers (checked above).
        unsafe { activator.query(riid, ppvobject) }.ok()
    }

    fn LockServer(&self, _flock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

/// One message for the toast thread.
enum Message {
    /// A toast-surface offer and its button, if any.
    Offer(MessageId, Vec<(&'static str, String)>, Option<ToastAction>),
    /// Register the activator with `RegisterFn`, routing clicks to the route.
    Attach(ToastRoute, RegisterFn),
}

/// The shell's `Notifier`: posts each toast-surface offer to the one toast thread.
pub struct ShellNotifier {
    /// `None` when the toast thread could not be started: the offers are dropped.
    offers: Option<Sender<Message>>,
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

    /// Registers the toast activator through [`register_class_object`] and turns the
    /// buttons on for the offers after it: [`ShellNotifier::attach_with`].
    pub fn attach(&self, route: ToastRoute) {
        self.attach_with(route, register_class_object);
    }

    /// Posts the registration to the toast thread and returns at once. The thread hands
    /// the activator's class factory to `register` once per notifier (a later attach is
    /// ignored); offers after it carry a button only when `register` returned `Ok`. A
    /// failed registration writes one `toast_failed` line with its code.
    pub fn attach_with(&self, route: ToastRoute, register: RegisterFn) {
        if let Some(offers) = &self.offers {
            let _ = offers.send(Message::Attach(route, register));
        }
    }
}

impl Notifier for ShellNotifier {
    fn notify(
        &self,
        id: MessageId,
        params: &[(&'static str, String)],
        action: Option<ToastAction>,
    ) {
        if !i18n::is_toast_surface(id) {
            return;
        }
        if let Some(offers) = &self.offers {
            // Unbounded: never waits. A send fails only once the thread has ended
            // (its COM init failed, already logged): the offer is dropped.
            let _ = offers.send(Message::Offer(id, params.to_vec(), action));
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

/// The toast thread: COM, then one toast per offer until the notifier is dropped; an
/// attach registers the activator here, so its class object lives in this thread's MTA.
fn run(offers: &Receiver<Message>, settings: &SettingsService, log: &Log, sink: &dyn ToastSink) {
    let _com = match ComGuard::init() {
        Ok(com) => com,
        Err(hr) => {
            failed(log, Some(os_code(&WinError::from_hresult(hr))));
            return;
        }
    };
    // `Some` once the activator is registered (with its class object, kept for the
    // thread's life): only then is a button built.
    let mut registered: Option<(Arc<Buttons>, IClassFactory)> = None;
    let mut attached = false;
    while let Ok(message) = offers.recv() {
        match message {
            Message::Attach(route, register) => {
                if attached {
                    continue;
                }
                attached = true;
                let shared = Arc::new(Buttons {
                    route,
                    offered: Mutex::new(HashMap::new()),
                });
                let factory: IClassFactory = ActivatorFactory {
                    buttons: Arc::clone(&shared),
                }
                .into();
                match register(&factory) {
                    Ok(()) => registered = Some((shared, factory)),
                    Err(err) => failed(log, Some(os_code(&err))),
                }
            }
            Message::Offer(id, params, action) => {
                let lang = settings.snapshot().ui_language;
                let args: Vec<(&str, &str)> =
                    params.iter().map(|(k, v)| (*k, v.as_str())).collect();
                let text = i18n::text(lang, id, &args);
                let button = registered
                    .as_ref()
                    .zip(action)
                    .map(|((shared, _), action)| button(shared, lang, action));
                if let Err(err) = toast_xml(&text, button).and_then(|xml| sink.show(&xml)) {
                    failed(log, Some(os_code(&err)));
                }
            }
        }
    }
}

/// One button: its label in `lang` and its argument (the core codec's output). An Open
/// button records its call's root for the activator.
fn button(shared: &Buttons, lang: i18n::UiLanguage, action: ToastAction) -> (String, String) {
    let arg = action.to_toast_arg();
    let label = match action {
        ToastAction::Retry(_) => i18n::ACTION_RETRY,
        ToastAction::Open { call, root } => {
            shared
                .offered
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(call, root);
            i18n::ACTION_OPEN
        }
    };
    (i18n::text(lang, label, &[]), arg)
}

/// The toast XML of one text line and at most one button, built through the DOM:
/// `<toast><visual><binding template="ToastGeneric"><text>…</text></binding></visual>
/// [<actions><action content=… arguments=… activationType="foreground"/></actions>]
/// </toast>`; `text` is a text node and the button's label and argument are attribute
/// values, escaped by the serializer.
fn toast_xml(text: &str, button: Option<(String, String)>) -> Result<String, WinError> {
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
    if let Some((label, arg)) = button {
        let actions = doc.CreateElement(h!("actions"))?;
        let action: XmlElement = doc.CreateElement(h!("action"))?;
        action.SetAttribute(h!("content"), &HSTRING::from(label))?;
        action.SetAttribute(h!("arguments"), &HSTRING::from(arg))?;
        action.SetAttribute(h!("activationType"), h!("foreground"))?;
        actions.AppendChild(&action)?;
        toast.AppendChild(&actions)?;
    }
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
