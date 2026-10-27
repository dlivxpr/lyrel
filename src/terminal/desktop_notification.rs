//! Desktop notification delivery and platform activation routing.

use gpui_kit::EntityId;
#[cfg(any(target_os = "macos", test))]
use std::sync::atomic::Ordering;

pub(crate) fn notify_desktop(title: Option<&str>, body: &str) {
    notify_desktop_for_pane(title, body, None);
}

/// Show a desktop notification.
///
/// Naming a `pane` makes the notification clickable where the platform supports
/// it, so activating it reveals that pane. It is an `EntityId` on purpose: the
/// tray dispatch identifies leaves by their gpui entity id, and taking a bare
/// `u64` here is what lets a caller hand over a `pane_id` — a different number,
/// assigned by the daemon — and get a notification that reveals nothing.
///
/// macOS always goes through `macos_notify`, clickable or not. Elsewhere, every
/// other case (no pane, unsupported platform, Windows toast queue full) falls
/// back to the plain `notify-rust` path below, which is why
/// `try_clickable_notification` reports whether it took the job.
pub(crate) fn notify_desktop_for_pane(title: Option<&str>, body: &str, pane: Option<EntityId>) {
    let summary = sanitize_notification_text(title.unwrap_or("lyrel"), NOTIFY_TITLE_MAX);
    let body = sanitize_notification_text(body, NOTIFY_BODY_MAX);

    #[cfg(target_os = "macos")]
    {
        macos_notify::deliver(summary, body, pane.map(|p| p.as_u64()));
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(pane) = pane
            && try_clickable_notification(&summary, &body, pane.as_u64())
        {
            return;
        }

        std::thread::spawn(move || {
            let mut notif = notify_rust::Notification::new();
            notif.summary(&summary).body(&body);
            // Without our own AUMID, the Windows backend falls back to
            // PowerShell's — icon and name included. Only set ours once the
            // shell has indexed a shortcut carrying it: for an AUMID it does
            // not know, `show()` reports success and drops the toast, so the
            // ugly fallback beats the branded one every time we are not sure.
            #[cfg(target_os = "windows")]
            if let Some(app_id) = crate::core::aumid::toast_app_id() {
                notif.app_id(app_id);
            }
            let _ = notif.show();
        });
    }
}

/// Longest title / body we hand to a notification backend.
///
/// The text comes straight off the terminal (OSC 9/777 payloads, window
/// titles, an agent's own status line), so it is attacker-shaped: arbitrarily
/// long, and free to contain control bytes. Every backend truncates in its own
/// ugly way, and on Windows a stray `ESC` makes the toast XML fail to parse and
/// the whole notification disappear — so clamp both here, once, for all paths.
const NOTIFY_TITLE_MAX: usize = 96;
const NOTIFY_BODY_MAX: usize = 512;

fn sanitize_notification_text(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut taken = 0usize;
    for ch in s.chars() {
        // XML 1.0 allows exactly tab / LF / CR out of the control range.
        if ch.is_control() && !matches!(ch, '\t' | '\n' | '\r') {
            continue;
        }
        if taken == max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
        taken += 1;
    }
    out
}

/// What a click on a macOS notification has to carry back: the pane to reveal.
///
/// It rides in the notification's `identifier`, which is the one field the
/// center hands back verbatim on activation without a dictionary round trip.
/// Shape: `lyrel-pane-<pid>-<leaf>-<seq>`.
///
/// The pid is what makes a stale identifier fail closed. Notifications outlive
/// the process that sent them, and the center hands a click on one of those to
/// whatever process now owns the bundle id — a relaunched lyrel, or a second
/// instance running alongside. A leaf id is a gpui entity id, which a fresh
/// process hands out again in the same order, so without the pid that click
/// would reveal an unrelated pane. The sequence number keeps two notifications
/// for the same pane apart — the center treats a repeated identifier as
/// "replace the earlier one".
#[cfg(any(target_os = "macos", test))]
const NOTIFICATION_ID_PREFIX: &str = "lyrel-pane-";

#[cfg(any(target_os = "macos", test))]
fn notification_identifier(leaf_id: u64) -> String {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{NOTIFICATION_ID_PREFIX}{pid}-{leaf_id}-{seq}")
}

#[cfg(any(target_os = "macos", test))]
fn leaf_id_in_identifier(identifier: &str) -> Option<u64> {
    let rest = identifier.strip_prefix(NOTIFICATION_ID_PREFIX)?;
    let (pid, rest) = rest.split_once('-')?;
    if pid.parse::<u32>().ok()? != std::process::id() {
        return None;
    }
    let (leaf_id, _seq) = rest.split_once('-')?;
    leaf_id.parse().ok()
}

/// macOS notifications, straight to `NSUserNotificationCenter`.
///
/// This used to go through `mac-notification-sys` with `wait_for_click`, so a
/// click could reveal the pane. The way that crate notices a click is to park
/// the sending thread and, for every notification outstanding, add a repeating
/// 0.5 s timer to the *main* run loop that calls `deliveredNotifications` — a
/// synchronous XPC round trip — to see whether the banner is still there. A
/// banner nobody clicks stays in Notification Center, so its timer never goes
/// away. Sampled with nine outstanding: a fifth of the UI thread inside that
/// XPC, every window juddering, and each agent turn adding one more timer.
///
/// Here the click arrives through the center's delegate, which is what the
/// API is for: no parked thread, no timer, nothing on the main thread until
/// the user actually clicks. The pane rides in the notification's identifier.
///
/// Nothing else may touch the center on this platform. `mac-notification-sys`
/// installs a delegate of its own the first time it sends, the last
/// `setDelegate:` wins, and `notify-rust` is that crate on macOS — so its
/// `show` is never called here, only its `set_application`, which does not
/// install one (that is what names a bare `cargo run` binary to the center).
#[cfg(target_os = "macos")]
#[allow(
    deprecated,
    reason = "UNUserNotificationCenter needs a signed, entitled bundle; NSUserNotification is what a bare binary can use"
)]
mod macos_notify {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::ProtocolObject;
    use objc2::{AnyThread, define_class, msg_send};
    use objc2_foundation::{
        NSObject, NSObjectProtocol, NSString, NSUserNotification, NSUserNotificationCenter,
        NSUserNotificationCenterDelegate,
    };

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; `Delegate` has no
        // ivars and no `Drop`.
        #[unsafe(super = NSObject)]
        #[name = "LyrelNotificationDelegate"]
        struct Delegate;

        // SAFETY: `NSObjectProtocol` has no safety requirements.
        unsafe impl NSObjectProtocol for Delegate {}

        // SAFETY: `NSUserNotificationCenterDelegate` has no safety requirements.
        unsafe impl NSUserNotificationCenterDelegate for Delegate {
            /// Runs on the main thread, when the user clicks the banner or the
            /// entry in Notification Center. A channel push, then the clicked
            /// entry is dropped from the center — a one-way message, unlike the
            /// `deliveredNotifications` round trip this module exists to avoid.
            #[unsafe(method(userNotificationCenter:didActivateNotification:))]
            fn did_activate(
                &self,
                center: &NSUserNotificationCenter,
                notification: &NSUserNotification,
            ) {
                if let Some(leaf_id) = notification
                    .identifier()
                    .and_then(|id| super::leaf_id_in_identifier(&id.to_string()))
                {
                    super::reveal_pane(leaf_id);
                }
                center.removeDeliveredNotification(notification);
            }
        }
    );

    fn install_delegate() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // Name the delivering application to the center before it is first
            // touched: the center drops requests from a process with no bundle
            // identity, which is what a bare `cargo run` binary is. This
            // swizzles `-[NSBundle bundleIdentifier]` for the main bundle to
            // `io.github.dlivxpr.lyrel` when LaunchServices knows that id (a bundled
            // lyrel.app, or a machine that has one installed); when it does not,
            // the swizzle's own default, `com.apple.Terminal`, is what the
            // center sees. It can only be called once per process, so there is
            // no second chance to pass a different name.
            let _ = notify_rust::set_application("io.github.dlivxpr.lyrel");
            // SAFETY: `NSObject`'s `init` takes nothing and returns the object.
            let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
            let center = NSUserNotificationCenter::defaultUserNotificationCenter();
            // SAFETY: `delegate` is a `Delegate`, which conforms to the protocol.
            unsafe { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
            // The center holds its delegate unretained. Ours lives as long as
            // the process, so it is simply never released.
            std::mem::forget(delegate);
        });
    }

    /// Hands the notification to the center and returns. `deliverNotification:`
    /// is an XPC message, so it goes out on a short-lived thread rather than
    /// from wherever the OSC sequence was parsed — never the UI thread.
    pub(super) fn deliver(title: String, body: String, leaf_id: Option<u64>) {
        std::thread::spawn(move || {
            autoreleasepool(|_| {
                install_delegate();
                let notification = NSUserNotification::new();
                notification.setTitle(Some(&NSString::from_str(&title)));
                notification.setInformativeText(Some(&NSString::from_str(&body)));
                if let Some(leaf_id) = leaf_id {
                    let id = super::notification_identifier(leaf_id);
                    notification.setIdentifier(Some(&NSString::from_str(&id)));
                }
                NSUserNotificationCenter::defaultUserNotificationCenter()
                    .deliverNotification(&notification);
            });
        });
    }
}

#[cfg(all(target_os = "windows", not(test)))]
fn try_clickable_notification(title: &str, body: &str, leaf_id: u64) -> bool {
    // An AUMID the shell has not indexed makes `Show` report success and drops
    // the toast on the floor, so without one the notify-rust path — which
    // deliberately keeps PowerShell's identity rather than lose the toast — is
    // the only one that shows anything at all.
    if crate::core::aumid::toast_app_id().is_none() {
        return false;
    }
    let Some(tx) = toast_thread() else {
        return false;
    };
    tx.try_send((title.to_string(), body.to_string(), leaf_id))
        .is_ok()
}

#[cfg(all(target_os = "windows", not(test)))]
type ToastRequest = (String, String, u64);

/// Toasts are built, shown and *kept* on one dedicated thread.
///
/// A `ToastNotification` has to stay alive for its `Activated` event to reach
/// us, and Windows parks toasts in Action Center long after they leave the
/// screen — a thread per toast sleeping out a fixed window would both leak
/// threads and stop working the moment the timer expired. Keeping them on a
/// single thread also means the WinRT objects never cross a thread boundary.
#[cfg(all(target_os = "windows", not(test)))]
fn toast_thread() -> Option<&'static std::sync::mpsc::SyncSender<ToastRequest>> {
    use std::sync::OnceLock;
    use std::sync::mpsc::sync_channel;

    static TX: OnceLock<Option<std::sync::mpsc::SyncSender<ToastRequest>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<ToastRequest>(32);
        std::thread::Builder::new()
            .name("lyrel-toasts".into())
            .spawn(move || toast_loop(rx))
            .map_err(|e| log::warn!("failed to start the toast thread: {e}"))
            .ok()
            .map(|_| tx)
    })
    .as_ref()
}

#[cfg(all(target_os = "windows", not(test)))]
fn toast_loop(rx: std::sync::mpsc::Receiver<ToastRequest>) {
    use std::collections::VecDeque;
    use windows::UI::Notifications::ToastNotification;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    // MTA lets the Activated callback run on a thread-pool thread without a
    // message loop of our own.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    // How many past toasts stay clickable. Dropping the oldest is what bounds
    // the memory here; there is no useful deadline to expire them on.
    const MAX_LIVE_TOASTS: usize = 32;
    let mut live: VecDeque<ToastNotification> = VecDeque::new();

    while let Ok((title, body, leaf_id)) = rx.recv() {
        if let Some(toast) = show_windows_toast(&title, &body, leaf_id) {
            if live.len() == MAX_LIVE_TOASTS {
                live.pop_front();
            }
            live.push_back(toast);
        }
    }
}

#[cfg(all(target_os = "windows", not(test)))]
fn show_windows_toast(
    title: &str,
    body: &str,
    leaf_id: u64,
) -> Option<windows::UI::Notifications::ToastNotification> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::{IInspectable, Ref};

    let app_id = crate::core::aumid::toast_app_id()?;

    let xml = format!(
        r#"<toast>
  <visual>
    <binding template="ToastText02">
      <text id="1">{}</text>
      <text id="2">{}</text>
    </binding>
  </visual>
</toast>"#,
        xml_escape(title),
        xml_escape(body)
    );

    let doc = match XmlDocument::new() {
        Ok(d) => {
            if let Err(e) = d.LoadXml(&xml.into()) {
                log::warn!("failed to load toast xml: {e}");
                return None;
            }
            d
        }
        Err(e) => {
            log::warn!("failed to create toast xml document: {e}");
            return None;
        }
    };

    let toast = match ToastNotification::CreateToastNotification(&doc) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("failed to create toast notification: {e}");
            return None;
        }
    };

    if let Err(e) = toast.Activated(&TypedEventHandler::new(
        move |_sender: Ref<ToastNotification>, _args: Ref<IInspectable>| {
            reveal_pane(leaf_id);
            Ok(())
        },
    )) {
        log::warn!("failed to register toast activated handler: {e}");
    }

    let notifier = match ToastNotificationManager::CreateToastNotifierWithId(&app_id.into()) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("failed to create toast notifier: {e}");
            return None;
        }
    };

    if let Err(e) = notifier.Show(&toast) {
        log::warn!("failed to show toast: {e}");
        return None;
    }

    Some(toast)
}

/// Ask the tray dispatch loop to bring `leaf_id` to the front. Runs on whatever
/// thread the platform hands the activation to, so it only touches the channel.
#[cfg(any(target_os = "macos", all(target_os = "windows", not(test))))]
fn reveal_pane(leaf_id: u64) {
    if let Some(tx) = crate::ui::tray::sender() {
        let _ = tx.try_send(crate::ui::tray::TrayAction::RevealPane { leaf_id });
    }
}

#[cfg(not(any(target_os = "macos", all(target_os = "windows", not(test)))))]
fn try_clickable_notification(_title: &str, _body: &str, _leaf_id: u64) -> bool {
    // Linux notifications go through notify-rust; click-to-reveal would need a
    // D-Bus action listener of its own.
    false
}

#[cfg(target_os = "windows")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod notification_tests {
    use super::*;

    #[test]
    fn a_notification_identifier_carries_its_pane_back_out() {
        let id = notification_identifier(42);
        assert_eq!(leaf_id_in_identifier(&id), Some(42));
    }

    #[test]
    fn two_notifications_for_one_pane_do_not_replace_each_other() {
        // The center replaces a delivered notification whose identifier repeats.
        assert_ne!(notification_identifier(7), notification_identifier(7));
    }

    #[test]
    fn a_foreign_identifier_reveals_nothing() {
        let pid = std::process::id();
        for id in [
            String::new(),
            "lyrel-pane-".into(),
            "lyrel-pane-x-1".into(),
            "lyrel-pane-42".into(),
            "other-42-1".into(),
            format!("lyrel-pane-{pid}"),
            format!("lyrel-pane-{pid}-42"),
            format!("lyrel-pane-{pid}-x-1"),
        ] {
            assert_eq!(leaf_id_in_identifier(&id), None, "{id:?}");
        }
    }

    #[test]
    fn a_notification_from_another_process_reveals_nothing() {
        // Notifications outlive the process that sent them, and gpui hands out
        // the same entity ids again in a fresh process. A stale click must not
        // land on whatever pane holds that id now.
        let other_pid = std::process::id().wrapping_add(1);
        let stale = format!("{NOTIFICATION_ID_PREFIX}{other_pid}-42-0");
        assert_eq!(leaf_id_in_identifier(&stale), None);
    }

    #[test]
    fn sanitizing_drops_control_bytes_but_keeps_line_breaks() {
        let raw = "build \x1b[31mfailed\x07\nsee log\ttail";
        assert_eq!(
            sanitize_notification_text(raw, NOTIFY_BODY_MAX),
            "build [31mfailed\nsee log\ttail"
        );
    }

    #[test]
    fn sanitizing_clamps_by_chars_not_bytes() {
        let out = sanitize_notification_text(&"账".repeat(200), 8);
        assert_eq!(out, format!("{}…", "账".repeat(8)));
        assert_eq!(sanitize_notification_text("short", 8), "short");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn xml_escaping_covers_every_entity_once() {
        assert_eq!(
            xml_escape(r#"a & b < c > "d" 'e'"#),
            "a &amp; b &lt; c &gt; &quot;d&quot; &apos;e&apos;"
        );
    }
}
