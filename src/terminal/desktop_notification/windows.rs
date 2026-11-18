//! Windows Notification Center activates unpackaged apps through their COM
//! class. The installer owns that identity; copies without it retain WinRT
//! banner activation where Windows delivers the per-object event.

use std::ffi::c_void;
use std::sync::mpsc::{self, SyncSender};
#[cfg(not(test))]
use std::sync::{LazyLock, mpsc::Receiver};

use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_POINTER};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoAllowSetForegroundWindow, CoInitializeEx,
    CoRegisterClassObject, CoRevokeClassObject, CoUninitialize, CreateItemMoniker,
    GetRunningObjectTable, IClassFactory, IClassFactory_Impl, IMoniker, IRunningObjectTable,
    REGCLS_MULTIPLEUSE, ROTFLAGS_REGISTRATIONKEEPSALIVE,
};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use windows::core::{BOOL, GUID, HSTRING, IUnknown, Interface, PCWSTR, Ref, Result, implement, w};

use crate::core::aumid::AUMID;
use crate::ui::tray::TrayAction;

// Keep in sync with windows-installer.iss. Distinct from the install AppId.
pub(crate) const ACTIVATOR: GUID = GUID::from_u128(0x60ef7707_8f84_4911_9443_a7c8053aa5cf);

#[cfg(not(test))]
struct Toast {
    title: String,
    body: String,
    leaf_id: u64,
}

#[cfg(not(test))]
pub(super) fn enqueue(title: &str, body: &str, leaf_id: u64) -> bool {
    if crate::core::aumid::toast_app_id().is_none() {
        return false;
    }
    static TX: LazyLock<Option<SyncSender<Toast>>> = LazyLock::new(|| {
        let (tx, rx) = mpsc::sync_channel(32);
        std::thread::Builder::new()
            .name("lyrel-toasts".into())
            .spawn(move || worker(rx))
            .map(|_| tx)
            .map_err(|e| log::warn!("could not start Windows toast worker: {e}"))
            .ok()
    });
    let Some(tx) = TX.as_ref() else {
        return false;
    };
    tx.try_send(Toast {
        title: title.into(),
        body: body.into(),
        leaf_id,
    })
    .is_ok()
}

#[derive(Clone)]
enum Destination {
    Live {
        instance: uuid::Uuid,
        actions: smol::channel::Sender<TrayAction>,
    },
    Shell(Option<SyncSender<()>>),
}

#[implement(INotificationActivationCallback)]
struct Callback {
    destination: Destination,
}

impl INotificationActivationCallback_Impl for Callback_Impl {
    fn Activate(
        &self,
        app_id: &PCWSTR,
        invoked: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> Result<()> {
        if let Some((instance, leaf_id)) = activation_target(*app_id, *invoked) {
            match &self.destination {
                Destination::Live {
                    instance: own,
                    actions,
                } if *own == instance => {
                    let _ = actions.try_send(TrayAction::RevealPane { leaf_id });
                }
                Destination::Shell(_) => relay_activation(instance, *app_id, *invoked),
                Destination::Live { .. } => {}
            }
        }
        if let Destination::Shell(Some(done)) = &self.destination {
            let _ = done.try_send(());
        }
        Ok(())
    }
}

fn activation_target(app_id: PCWSTR, invoked: PCWSTR) -> Option<(uuid::Uuid, u64)> {
    if app_id.is_null() || invoked.is_null() {
        return None;
    }
    // SAFETY: The callback's COM contract supplies NUL-terminated strings.
    if !unsafe { app_id.as_wide() }
        .iter()
        .copied()
        .eq(AUMID.encode_utf16())
    {
        return None;
    }
    let invoked = unsafe { invoked.to_string() }.ok()?;
    let (tag, pane) = invoked.split_once(':')?;
    if tag.len() != 32 {
        return None;
    }
    Some((uuid::Uuid::parse_str(tag).ok()?, pane.parse().ok()?))
}

// The shell knows the install-wide CLSID, not the process that issued a toast.
// Look up a live callback only: stale notices cannot launch a server or GUI.
fn relay_activation(instance: uuid::Uuid, app_id: PCWSTR, invoked: PCWSTR) {
    let Ok(moniker) = instance_moniker(instance) else {
        return;
    };
    // SAFETY: Called from COM's MTA with validated callback arguments.
    unsafe {
        let Ok(owner) = GetRunningObjectTable(0).and_then(|rot| rot.GetObject(&moniker)) else {
            return;
        };
        if let Ok(callback) = owner.cast::<INotificationActivationCallback>() {
            // Pass on foreground permission when the shell granted it. A
            // denied grant must not prevent selecting the correct pane.
            let _ = CoAllowSetForegroundWindow(&callback, None);
            let _ = callback.Activate(app_id, invoked, &[]);
        }
    }
}

fn instance_moniker(instance: uuid::Uuid) -> Result<IMoniker> {
    let name = HSTRING::from(format!("{AUMID}.toast.{}", instance.simple()));
    // SAFETY: Called in an initialized COM apartment with valid strings.
    unsafe { CreateItemMoniker(w!("!"), &name) }
}

struct RunningPaneCallback {
    rot: IRunningObjectTable,
    cookie: u32,
}

impl RunningPaneCallback {
    fn new(instance: uuid::Uuid, actions: smol::channel::Sender<TrayAction>) -> Result<Self> {
        let callback: INotificationActivationCallback = Callback {
            destination: Destination::Live { instance, actions },
        }
        .into();
        let moniker = instance_moniker(instance)?;
        // SAFETY: The MTA outlives this registration. The default ROT scope
        // limits lookup to clients in the registering window station.
        unsafe {
            let rot = GetRunningObjectTable(0)?;
            let cookie = rot.Register(ROTFLAGS_REGISTRATIONKEEPSALIVE, &callback, &moniker)?;
            Ok(Self { rot, cookie })
        }
    }
}

impl Drop for RunningPaneCallback {
    fn drop(&mut self) {
        // SAFETY: The cookie came from this table in the still-live MTA.
        let _ = unsafe { self.rot.Revoke(self.cookie) };
    }
}

#[implement(IClassFactory)]
struct Factory {
    destination: Destination,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> Result<()> {
        if out.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: COM provides an output slot and requested interface ID.
        unsafe { out.write(std::ptr::null_mut()) };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let object: INotificationActivationCallback = Callback {
            destination: self.destination.clone(),
        }
        .into();
        unsafe { object.query(iid, out).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        // SAFETY: Called on the dedicated Windows notifications thread or the
        // COM-only process's main thread, before doing any other COM work.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: Balances this thread's successful CoInitializeEx.
        unsafe { CoUninitialize() };
    }
}

struct RegisteredClass(u32);
impl RegisteredClass {
    fn new(done: Option<SyncSender<()>>) -> Result<Self> {
        let factory: IClassFactory = Factory {
            destination: Destination::Shell(done),
        }
        .into();
        // SAFETY: The MTA is initialized and COM retains the factory until
        // CoRevokeClassObject. The registration is dropped before the MTA.
        unsafe {
            CoRegisterClassObject(
                &ACTIVATOR,
                &factory,
                CLSCTX_LOCAL_SERVER,
                REGCLS_MULTIPLEUSE,
            )
            .map(Self)
        }
    }
}
impl Drop for RegisteredClass {
    fn drop(&mut self) {
        // SAFETY: Cookie was allocated by CoRegisterClassObject in this MTA.
        let _ = unsafe { CoRevokeClassObject(self.0) };
    }
}

/// Answer shell activation without starting a GUI or connecting to a daemon.
/// If the issuing process is no longer alive, nothing is revealed.
pub(crate) fn run_embedding() {
    let Ok(_apartment) = Apartment::new() else {
        return;
    };
    let (tx, rx) = mpsc::sync_channel(1);
    if let Ok(_class) = RegisteredClass::new(Some(tx)) {
        let _ = rx.recv_timeout(std::time::Duration::from_secs(5));
    }
}

#[cfg(not(test))]
fn worker(rx: Receiver<Toast>) {
    use std::collections::VecDeque;
    use windows::UI::Notifications::ToastNotification;

    let Ok(_apartment) = Apartment::new() else {
        for toast in rx {
            super::show_plain_notification(toast.title, toast.body);
        }
        return;
    };
    // Entity IDs can be recycled by another GUI. The token only names a pane
    // in the GUI that issued the toast, even if COM activates a different one.
    let instance = uuid::Uuid::new_v4();
    let actions = crate::ui::tray::sender();
    let clickable = (|| {
        crate::core::aumid::clickable_app_id()?;
        let actions = actions.clone()?;
        match RunningPaneCallback::new(instance, actions)
            .and_then(|owner| RegisteredClass::new(None).map(|class| (owner, class)))
        {
            Ok(registration) => Some(registration),
            Err(e) => {
                log::warn!("could not register Windows toast activator: {e}");
                None
            }
        }
    })();
    // Only the non-COM path needs to keep WinRT toast objects alive.
    const MAX_LIVE_BANNERS: usize = 32;
    let mut banners = VecDeque::<ToastNotification>::new();
    for toast in rx {
        match show(
            &toast,
            clickable.as_ref().map(|_| instance),
            actions.as_ref(),
        ) {
            Ok(notification) => {
                if clickable.is_none() {
                    if banners.len() == MAX_LIVE_BANNERS {
                        banners.pop_front();
                    }
                    banners.push_back(notification);
                }
            }
            Err(e) => {
                log::warn!("could not show Windows pane notification: {e}");
                super::show_plain_notification(toast.title, toast.body);
            }
        }
    }
}

#[cfg(not(test))]
fn show(
    toast: &Toast,
    instance: Option<uuid::Uuid>,
    actions: Option<&smol::channel::Sender<TrayAction>>,
) -> Result<windows::UI::Notifications::ToastNotification> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    let title = xml_escape(&toast.title);
    let body = xml_escape(&toast.body);
    let leaf_id = toast.leaf_id;
    let xml = match instance {
        Some(instance) => format!(
            r#"<toast launch="{}:{leaf_id}"><visual><binding template="ToastGeneric"><text>{title}</text><text>{body}</text></binding></visual></toast>"#,
            instance.simple(),
        ),
        None => format!(
            r#"<toast><visual><binding template="ToastText02"><text id="1">{title}</text><text id="2">{body}</text></binding></visual></toast>"#,
        ),
    };
    let doc = XmlDocument::new()?;
    doc.LoadXml(&xml.into())?;
    let notification = ToastNotification::CreateToastNotification(&doc)?;
    if instance.is_none()
        && let Some(actions) = actions.cloned()
    {
        notification.Activated(&TypedEventHandler::new(move |_, _| {
            let _ = actions.try_send(TrayAction::RevealPane { leaf_id });
            Ok(())
        }))?;
    }
    ToastNotificationManager::CreateToastNotifierWithId(&AUMID.into())?.Show(&notification)?;
    Ok(notification)
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_current_toast_from_our_app_can_reveal_a_pane() {
        use windows::core::HSTRING;

        let (actions, received) = smol::channel::bounded(1);
        let factory: IClassFactory = Factory {
            destination: Destination::Live {
                instance: uuid::Uuid::from_u128(1),
                actions,
            },
        }
        .into();
        // SAFETY: The factory and callback are used in-process with valid strings.
        let callback: INotificationActivationCallback =
            unsafe { factory.CreateInstance(None) }.unwrap();
        let valid = "00000000000000000000000000000001:42";
        for (app, args) in [
            ("unrelated.app", valid),
            (AUMID, "00000000000000000000000000000002:42"),
            (AUMID, "00000000000000000000000000000001:42:43"),
            (AUMID, "42"),
        ] {
            unsafe { callback.Activate(&HSTRING::from(app), &HSTRING::from(args), &[]) }.unwrap();
            assert!(received.is_empty());
        }
        unsafe { callback.Activate(&HSTRING::from(AUMID), &HSTRING::from(valid), &[]) }.unwrap();
        assert_eq!(
            received.try_recv(),
            Ok(TrayAction::RevealPane { leaf_id: 42 })
        );
    }

    #[test]
    fn a_shell_callback_in_another_instance_reaches_the_toast_owner() {
        let _apartment = Apartment::new().unwrap();
        let owner = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let (owner_actions, owner_received) = smol::channel::bounded(1);
        let (other_actions, other_received) = smol::channel::bounded(1);
        let owner_registration = RunningPaneCallback::new(owner, owner_actions).unwrap();
        let _other_registration = RunningPaneCallback::new(other, other_actions).unwrap();
        let factory: IClassFactory = Factory {
            destination: Destination::Shell(None),
        }
        .into();
        let payload = HSTRING::from(format!("{}:42", owner.simple()));
        let app = HSTRING::from(AUMID);

        // SAFETY: COM is initialized and the arguments live through each call.
        unsafe {
            let callback: INotificationActivationCallback = factory.CreateInstance(None).unwrap();
            callback.Activate(&app, &payload, &[]).unwrap();
            assert_eq!(
                owner_received.try_recv(),
                Ok(TrayAction::RevealPane { leaf_id: 42 })
            );
            assert!(other_received.is_empty());
            drop(owner_registration);
            callback.Activate(&app, &payload, &[]).unwrap();
            assert!(owner_received.is_empty());
            assert!(other_received.is_empty());
        }
    }

    #[test]
    fn terminal_text_cannot_supply_toast_xml() {
        assert_eq!(
            xml_escape(r#"a & b < c > "d" 'e'"#),
            "a &amp; b &lt; c &gt; &quot;d&quot; &apos;e&apos;"
        );
    }
}
