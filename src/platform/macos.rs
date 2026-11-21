use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use super::{HotkeyEvent, HotkeySink};
use crate::hotkey::{self, KeyClass, ModKey, Mods, RawKey};

const KEY_V: u16 = 9;

/// A modifier key's keycode → the key and its device-dependent flag bit (set while it is down).
/// Caps Lock (57) is not a talk key.
fn mod_key(code: i64) -> Option<(ModKey, u64)> {
    Some(match code {
        59 => (ModKey::LeftControl, 0x1),
        62 => (ModKey::RightControl, 0x2000),
        58 => (ModKey::LeftOption, 0x20),
        61 => (ModKey::RightOption, 0x40),
        56 => (ModKey::LeftShift, 0x2),
        60 => (ModKey::RightShift, 0x4),
        55 => (ModKey::LeftCommand, 0x8),
        54 => (ModKey::RightCommand, 0x10),
        63 => (ModKey::Fn, 0x800000),
        _ => return None,
    })
}

fn raw_key(etype: CGEventType, event: &CGEvent) -> Option<(RawKey, KeyClass)> {
    let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
    match etype {
        CGEventType::FlagsChanged => {
            let (key, bit) = mod_key(code)?;
            Some((RawKey::Mod { key, down: event.get_flags().bits() & bit != 0 }, KeyClass::Other))
        }
        CGEventType::KeyDown => {
            let code = code as u16;
            let f = event.get_flags();
            let mods = Mods {
                control: f.contains(CGEventFlags::CGEventFlagControl),
                option: f.contains(CGEventFlags::CGEventFlagAlternate),
                shift: f.contains(CGEventFlags::CGEventFlagShift),
                command: f.contains(CGEventFlags::CGEventFlagCommand),
            };
            let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
            Some((RawKey::Down { code, mods, repeat }, key_class(code)))
        }
        CGEventType::KeyUp => Some((RawKey::Up { code: code as u16 }, key_class(code as u16))),
        _ => None,
    }
}

/// Listens on a background thread with a CGEventTap. Needs Accessibility (or Input
/// Monitoring); until granted it reports PermissionMissing and retries.
///
/// The tap is listen-only while the talk key is a modifier (it is never kept from apps), and
/// active (can drop keys) only for a chord or while a new key is being recorded. When that need
/// changes (a new binding from Settings), the tap is replaced by the other kind.
pub fn spawn_hotkey_listener(sink: HotkeySink) {
    let sink: Arc<HotkeySink> = Arc::new(sink);
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            let mut warned = false;
            let our_pid = i64::from(std::process::id());
            loop {
                // A tap can be created without Accessibility yet receive nothing, so wait for it.
                if !accessibility_trusted(false) {
                    if !warned {
                        sink(HotkeyEvent::PermissionMissing);
                        warned = true;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                let active = hotkey::lock().wants_swallow();
                let disabled = Arc::new(AtomicBool::new(false));
                let (s, d) = (sink.clone(), disabled.clone());
                // Every keystroke passes through this while the tap is active: one lock, no I/O.
                let tap = CGEventTap::new(
                    CGEventTapLocation::Session,
                    CGEventTapPlacement::HeadInsertEventTap,
                    if active { CGEventTapOptions::Default } else { CGEventTapOptions::ListenOnly },
                    vec![CGEventType::FlagsChanged, CGEventType::KeyDown, CGEventType::KeyUp],
                    move |_proxy, etype, event| {
                        if matches!(etype, CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput) {
                            d.store(true, Ordering::SeqCst);
                            return CallbackResult::Keep;
                        }
                        // Our own ⌘V and live typing.
                        if event.get_integer_value_field(EventField::EVENT_SOURCE_UNIX_PROCESS_ID) == our_pid {
                            return CallbackResult::Keep;
                        }
                        let Some((raw, class)) = raw_key(etype, event) else { return CallbackResult::Keep };
                        let verdict = hotkey::lock().feed(raw, Instant::now(), class);
                        if let Some(e) = verdict.event {
                            s(e);
                        }
                        if verdict.swallow { CallbackResult::Drop } else { CallbackResult::Keep }
                    },
                );
                let Ok(tap) = tap else {
                    if !warned {
                        sink(HotkeyEvent::PermissionMissing);
                        warned = true;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                };
                let source = tap.mach_port().create_runloop_source(0).expect("runloop source");
                let run_loop = CFRunLoop::get_current();
                run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });
                tap.enable();
                eprintln!("keyboard tap: {}", if active { "active" } else { "listen-only" });
                sink(HotkeyEvent::Ready);
                loop {
                    CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, Duration::from_millis(200), false);
                    if disabled.swap(false, Ordering::SeqCst) {
                        tap.enable();
                    }
                    if hotkey::lock().wants_swallow() != active {
                        break;
                    }
                }
                run_loop.remove_source(&source, unsafe { kCFRunLoopCommonModes });
            }
        })
        .expect("spawn hotkey thread");
}

pub fn send_paste() {
    let Ok(src) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else { return };
    for down in [true, false] {
        if let Ok(ev) = CGEvent::new_keyboard_event(src.clone(), KEY_V, down) {
            ev.set_flags(CGEventFlags::CGEventFlagCommand);
            ev.post(CGEventTapLocation::HID);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

/// Whether this process may watch keys and send ⌘V. With `prompt`, macOS shows its permission dialog.
pub fn accessibility_trusted(prompt: bool) -> bool {
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let opts = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::from(prompt))]);
    unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef()) }
}

/// The app in front (its process id), to tell whether the user switched apps while text was
/// being typed. Call it on the main thread (NSWorkspace updates it there).
pub fn focused_window() -> u64 {
    objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication().map_or(0, |a| a.processIdentifier() as u64)
}

/// Windows only (an app run as administrator ignores PoodPim's ⌘V there); never on macOS.
pub fn foreground_is_elevated() -> bool {
    false
}

/// The talk keys offered as one-click choices in Settings.
pub const TALK_PRESETS: &[(ModKey, &str)] =
    &[(ModKey::RightOption, "⌥ ขวา"), (ModKey::RightCommand, "⌘ ขวา"), (ModKey::RightControl, "⌃ ขวา"), (ModKey::Fn, "fn 🌐")];

pub fn mod_label(k: ModKey) -> &'static str {
    match k {
        ModKey::LeftControl => "Left ⌃",
        ModKey::RightControl => "Right ⌃",
        ModKey::LeftOption => "Left ⌥",
        ModKey::RightOption => "Right ⌥",
        ModKey::LeftShift => "Left ⇧",
        ModKey::RightShift => "Right ⇧",
        ModKey::LeftCommand => "Left ⌘",
        ModKey::RightCommand => "Right ⌘",
        ModKey::Fn => "fn",
    }
}

/// F-keys by virtual keycode (they are not in order).
const F_KEYS: [u16; 20] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90];

pub fn key_class(code: u16) -> KeyClass {
    match code {
        53 => KeyClass::Escape,
        // Volume, Help, the JIS 英数 / かな keys, and anything past the keyboard's usual range
        // (media and dictation keys): none of them type a character.
        _ if F_KEYS.contains(&code) => KeyClass::Function,
        72 | 73 | 74 | 102 | 104 | 114 => KeyClass::Function,
        128.. => KeyClass::Function,
        _ => KeyClass::Other,
    }
}

/// A key's name for labels (US layout), e.g. "Space", "F5", "K".
pub fn key_name(code: u16) -> String {
    if let Some(i) = F_KEYS.iter().position(|&c| c == code) {
        return format!("F{}", i + 1);
    }
    /// Keycodes 0–47 on a US keyboard (36 is Return, named below).
    const CHARS: [&str; 48] = [
        "A", "S", "D", "F", "H", "G", "Z", "X", "C", "V", "§", "B", "Q", "W", "E", "R", "Y", "T", "1", "2", "3", "4", "6", "5",
        "=", "9", "7", "-", "8", "0", "]", "O", "U", "[", "I", "P", "", "L", "J", "'", "K", ";", "\\", ",", "/", "N", "M", ".",
    ];
    let named = match code {
        36 => "Return",
        48 => "Tab",
        49 => "Space",
        50 => "`",
        51 => "Delete",
        53 => "Esc",
        65 => "Keypad .",
        67 => "Keypad *",
        69 => "Keypad +",
        71 => "Clear",
        75 => "Keypad /",
        76 => "Enter",
        78 => "Keypad -",
        81 => "Keypad =",
        82..=89 => return format!("Keypad {}", code - 82),
        91 => "Keypad 8",
        92 => "Keypad 9",
        102 => "英数",
        104 => "かな",
        114 => "Help",
        115 => "Home",
        116 => "Page Up",
        117 => "⌦",
        119 => "End",
        121 => "Page Down",
        123 => "←",
        124 => "→",
        125 => "↓",
        126 => "↑",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    match CHARS.get(code as usize) {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => format!("Key {code}"),
    }
}

/// Where the app's log goes when it isn't started from a terminal.
pub fn log_dir() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join("Library/Logs/PoodPim"))
}

/// Sends this process's standard error to `file`.
pub fn redirect_stderr(file: std::fs::File) {
    use std::os::fd::IntoRawFd;
    unsafe extern "C" {
        fn dup2(from: i32, to: i32) -> i32;
    }
    let fd = file.into_raw_fd();
    unsafe { dup2(fd, 2) };
}

/// Shown when PoodPim is opened while it is already running.
pub const ALREADY_RUNNING: &str = "PoodPim เปิดอยู่แล้ว\n\n\
ไอคอนไมโครโฟน 🎙 อยู่ที่แถบเมนูด้านบนขวาของจอ ถ้าไม่เห็น อาจถูกรอยบากของจอบังไว้ \
ลองกด ⌘ ค้างแล้วลากไอคอนอื่นออกจากแถบเมนู หรือดูที่ System Settings › Menu Bar\n\n\
กดปุ่มพูดค้างไว้เพื่อพูดได้เลย";

/// Starts the app at login with a LaunchAgent pointing at this executable (rewritten on every
/// start, so it follows the app if it is moved). Only for the bundled app, not dev builds.
pub fn set_autostart(enable: bool) -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home folder"))?;
    let plist = home.join("Library/LaunchAgents/local.poodpim.app.plist");
    if !enable {
        if plist.exists() {
            std::fs::remove_file(&plist)?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy();
    if !exe.contains(".app/Contents/MacOS/") {
        return Ok(());
    }
    let exe = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>local.poodpim.app</string>
  <key>ProgramArguments</key><array><string>{exe}</string></array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#
    );
    if std::fs::read_to_string(&plist).ok().as_deref() != Some(xml.as_str()) {
        std::fs::create_dir_all(plist.parent().unwrap())?;
        std::fs::write(&plist, xml)?;
    }
    Ok(())
}

/// Opens a text file in TextEdit.
pub fn open_in_editor(path: &std::path::Path) {
    let _ = std::process::Command::new("open").arg("-t").arg(path).spawn();
}

/// Opens a web page in the default browser.
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Shows a folder in Finder.
pub fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// Opens System Settings › Privacy & Security › Accessibility.
pub fn open_permission_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

/// Asks for the Lambda URL and API key with two native dialogs (osascript).
/// Returns None if cancelled. An empty key answer keeps `current_key`.
pub fn ask_server_settings(current_url: &str, current_key: &str) -> Option<(String, String)> {
    let url = osascript_prompt(super::URL_PROMPT, current_url, false)?;
    let hint = if current_key.is_empty() { super::KEY_PROMPT } else { super::KEY_KEEP_PROMPT };
    let key = osascript_prompt(hint, "", true)?;
    let key = if key.trim().is_empty() { current_key.to_string() } else { key };
    Some((url.trim().to_string(), key.trim().to_string()))
}

/// Asks for the API key only (typed hidden). None if cancelled or left empty.
pub fn ask_api_key(message: &str) -> Option<String> {
    let key = osascript_prompt(message, "", true)?.trim().to_string();
    (!key.is_empty()).then_some(key)
}

/// A message with an OK button. Returns when it is closed.
pub fn alert(message: &str) {
    let dialog = "display dialog (item 1 of argv) with title \"PoodPim\" buttons {\"OK\"} default button 1";
    let _ = std::process::Command::new("osascript")
        .args(["-e", "on run argv", "-e", "activate", "-e", dialog, "-e", "end run", message])
        .output();
}

/// A two-button dialog. True if the user picked `ok`.
pub fn confirm(message: &str, ok: &str, cancel: &str) -> bool {
    let dialog = "display dialog (item 1 of argv) with title \"PoodPim\" buttons {(item 3 of argv), (item 2 of argv)} default button 2 cancel button 1";
    let Ok(out) = std::process::Command::new("osascript")
        .args(["-e", "on run argv", "-e", "activate", "-e", dialog, "-e", "return button returned of result", "-e", "end run", message, ok, cancel])
        .output()
    else {
        return false;
    };
    out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == ok
}

/// A one-line text prompt. None if cancelled or left empty.
pub fn ask_text(message: &str, default: &str) -> Option<String> {
    let t = osascript_prompt(message, default, false)?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn osascript_prompt(message: &str, default: &str, hidden: bool) -> Option<String> {
    // Values go in as argv so nothing typed is ever interpreted as AppleScript.
    let dialog = if hidden {
        "display dialog (item 1 of argv) default answer (item 2 of argv) with title \"PoodPim\" with hidden answer"
    } else {
        "display dialog (item 1 of argv) default answer (item 2 of argv) with title \"PoodPim\""
    };
    let out = std::process::Command::new("osascript")
        .args(["-e", "on run argv", "-e", "activate", "-e", dialog, "-e", "return text returned of result", "-e", "end run", message, default])
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // cancelled
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
}
