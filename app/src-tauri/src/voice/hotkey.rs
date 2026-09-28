//! Ctrl+Win hold detection for push-to-talk.
//!
//! The global-shortcut plugin only registers a modifier plus a normal key,
//! and only reports presses, so a modifiers-only hold needs a low-level
//! keyboard hook. The hook never swallows a keystroke: every key still
//! reaches Windows and the focused app. It only watches, and reports three
//! things to the controller over a channel:
//!
//! - `ChordDown`: Ctrl and Win are both down and nothing else was pressed.
//! - `Interrupt`: another key arrived while the chord was held, meaning the
//!   user is doing a real shortcut (Ctrl+Win+D, Ctrl+Win+Left, ...).
//! - `ChordUp`: Ctrl or Win was released.
//!
//! The hook callback runs on the thread that installed it, inside that
//! thread's message loop, and Windows drops hooks that take too long, so
//! it does nothing but flip a few atomics and send on a channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    ChordDown,
    Interrupt,
    ChordUp,
}

const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// Unassigned virtual-key code, used as the "mask key" below.
const VK_MASK: u16 = 0xE8;

static SENDER: OnceLock<Sender<HookEvent>> = OnceLock::new();
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static WIN_DOWN: AtomicBool = AtomicBool::new(false);
static CHORD: AtomicBool = AtomicBool::new(false);
/// Set when some other key is pressed while Ctrl or Win is held, and cleared
/// once both are released: Ctrl+C followed by Win must not start listening.
static TAINTED: AtomicBool = AtomicBool::new(false);

/// Tests drive the real hook with SendInput, which marks events injected.
#[cfg(test)]
static ACCEPT_INJECTED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static RAW_LOG: std::sync::Mutex<Vec<(u32, bool, u32)>> = std::sync::Mutex::new(Vec::new());

/// Pure state transition, split out from the hook callback so it can be
/// tested without installing a real hook. Returns the event to report, if
/// any.
pub(crate) fn on_key(vk: u32, down: bool) -> Option<HookEvent> {
    let is_ctrl = vk == VK_LCONTROL || vk == VK_RCONTROL;
    let is_win = vk == VK_LWIN || vk == VK_RWIN;

    if down {
        if is_ctrl {
            CTRL_DOWN.store(true, Ordering::SeqCst);
        } else if is_win {
            WIN_DOWN.store(true, Ordering::SeqCst);
        } else {
            if CTRL_DOWN.load(Ordering::SeqCst) || WIN_DOWN.load(Ordering::SeqCst) {
                TAINTED.store(true, Ordering::SeqCst);
            }
            if CHORD.load(Ordering::SeqCst) {
                return Some(HookEvent::Interrupt);
            }
            return None;
        }
        // Holding a key auto-repeats its keydown; CHORD makes that a no-op.
        if CTRL_DOWN.load(Ordering::SeqCst)
            && WIN_DOWN.load(Ordering::SeqCst)
            && !TAINTED.load(Ordering::SeqCst)
            && !CHORD.swap(true, Ordering::SeqCst)
        {
            return Some(HookEvent::ChordDown);
        }
        None
    } else {
        if is_ctrl {
            CTRL_DOWN.store(false, Ordering::SeqCst);
        } else if is_win {
            WIN_DOWN.store(false, Ordering::SeqCst);
        } else {
            return None;
        }
        if !CTRL_DOWN.load(Ordering::SeqCst) && !WIN_DOWN.load(Ordering::SeqCst) {
            TAINTED.store(false, Ordering::SeqCst);
        }
        if CHORD.swap(false, Ordering::SeqCst) {
            return Some(HookEvent::ChordUp);
        }
        None
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let injected = (info.flags & LLKHF_INJECTED).0 != 0;
        #[cfg(test)]
        let injected = injected && !(ACCEPT_INJECTED.load(Ordering::SeqCst) && info.vkCode != VK_MASK as u32);
        #[cfg(test)]
        RAW_LOG.lock().unwrap().push((info.vkCode, wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN, info.flags.0));
        // Ignore synthetic input, including Pointr's own mask key and every
        // keystroke Pointr types on the user's behalf.
        if !injected {
            let msg = wparam.0 as u32;
            let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
            if down || up {
                if let Some(event) = on_key(info.vkCode, down) {
                    if let Some(tx) = SENDER.get() {
                        let _ = tx.send(event);
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Installs the hook on a dedicated thread (it needs a message loop) and
/// returns immediately. Safe to call once; later calls are ignored.
pub fn install(tx: Sender<HookEvent>) {
    if SENDER.set(tx).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("pointr-ptt-hook".into())
        .spawn(|| unsafe {
            // Windows skips a low-level hook that doesn't answer within a
            // few hundred ms, and the event is simply lost. While the
            // speech model is transcribing, every core is busy; top
            // priority keeps this thread answering in time.
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
            let module: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0);
            if let Err(e) = hook {
                eprintln!("[voice] failed to install keyboard hook: {}", e);
                return;
            }
            let mut msg = MSG::default();
            // Blocks forever; the loop exists only so Windows can deliver
            // hook callbacks to this thread.
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        })
        .expect("failed to spawn keyboard hook thread");
}

fn physically_down(vk: u32) -> bool {
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

/// Whether Ctrl and Win are both physically held right now, per Windows'
/// own key state rather than the hook's event stream.
pub fn chord_physically_held() -> bool {
    (physically_down(VK_LCONTROL) || physically_down(VK_RCONTROL))
        && (physically_down(VK_LWIN) || physically_down(VK_RWIN))
}

/// Resets the hook's view of the modifiers to the real key state, after a
/// key-up the hook never received. Without this a missed Win release would
/// leave WIN_DOWN stuck, and the next press would behave oddly.
pub fn resync() {
    let ctrl = physically_down(VK_LCONTROL) || physically_down(VK_RCONTROL);
    let win = physically_down(VK_LWIN) || physically_down(VK_RWIN);
    CTRL_DOWN.store(ctrl, Ordering::SeqCst);
    WIN_DOWN.store(win, Ordering::SeqCst);
    CHORD.store(false, Ordering::SeqCst);
    if !ctrl && !win {
        TAINTED.store(false, Ordering::SeqCst);
    }
}

/// Taps an unassigned key while Win is held. Windows opens the Start menu
/// when Win is released with nothing pressed in between; this "something"
/// stops that, so releasing push-to-talk doesn't pop Start open. Same
/// technique AutoHotkey uses for its menu mask key.
pub fn mask_start_menu() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_MASK),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The chord state is global (the hook callback has no context
    // pointer), so these run in one test to avoid parallel interference.
    #[test]
    fn chord_transitions() {
        let reset = || {
            for vk in [VK_LCONTROL, VK_RCONTROL, VK_LWIN, VK_RWIN] {
                on_key(vk, false);
            }
        };

        // Either order starts it, once; key repeat doesn't re-fire.
        reset();
        assert_eq!(on_key(VK_LCONTROL, true), None);
        assert_eq!(on_key(VK_LWIN, true), Some(HookEvent::ChordDown));
        assert_eq!(on_key(VK_LWIN, true), None);
        assert_eq!(on_key(VK_LCONTROL, true), None);
        assert_eq!(on_key(VK_LWIN, false), Some(HookEvent::ChordUp));
        assert_eq!(on_key(VK_LCONTROL, false), None);

        reset();
        assert_eq!(on_key(VK_RWIN, true), None);
        assert_eq!(on_key(VK_RCONTROL, true), Some(HookEvent::ChordDown));
        assert_eq!(on_key(VK_RCONTROL, false), Some(HookEvent::ChordUp));
        on_key(VK_RWIN, false);

        // A real shortcut (Ctrl+Win+D) interrupts.
        reset();
        on_key(VK_LCONTROL, true);
        assert_eq!(on_key(VK_LWIN, true), Some(HookEvent::ChordDown));
        assert_eq!(on_key(0x44, true), Some(HookEvent::Interrupt));
        on_key(0x44, false);
        assert_eq!(on_key(VK_LWIN, false), Some(HookEvent::ChordUp));
        on_key(VK_LCONTROL, false);

        // Ctrl+C, then Win while Ctrl is still held: no chord.
        reset();
        on_key(VK_LCONTROL, true);
        assert_eq!(on_key(0x43, true), None);
        on_key(0x43, false);
        assert_eq!(on_key(VK_LWIN, true), None);
        on_key(VK_LWIN, false);
        on_key(VK_LCONTROL, false);

        // ...and a clean press afterwards works again.
        assert_eq!(on_key(VK_LCONTROL, true), None);
        assert_eq!(on_key(VK_LWIN, true), Some(HookEvent::ChordDown));
        reset();

        // Plain typing never reports anything.
        for vk in [0x41, 0x42, 0x20] {
            assert_eq!(on_key(vk, true), None);
            assert_eq!(on_key(vk, false), None);
        }
    }

    /// Drives the real installed hook with synthetic key presses, in both
    /// release orders. Presses real Ctrl/Win on this machine (the mask key
    /// keeps Start closed), so ignored by default:
    ///   cargo test hook_release_order -- --ignored --nocapture --test-threads=1
    #[test]
    #[ignore]
    fn hook_release_order() {
        use std::sync::mpsc;
        use std::time::Duration;
        use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY;

        fn send(vk: u32, up: bool) {
            let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
            if vk == VK_LWIN || vk == VK_RWIN || vk == VK_RCONTROL {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            let input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk as u16), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
            };
            unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
            std::thread::sleep(Duration::from_millis(60));
        }
        fn drain(rx: &mpsc::Receiver<HookEvent>) -> Vec<HookEvent> {
            std::thread::sleep(Duration::from_millis(150));
            rx.try_iter().collect()
        }

        ACCEPT_INJECTED.store(true, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        install(tx);
        std::thread::sleep(Duration::from_millis(300));

        for (name, first_up, second_up) in [
            ("release Ctrl first", VK_LCONTROL, VK_LWIN),
            ("release Win first", VK_LWIN, VK_LCONTROL),
        ] {
            RAW_LOG.lock().unwrap().clear();
            send(VK_LCONTROL, false);
            send(VK_LWIN, false);
            mask_start_menu();
            std::thread::sleep(Duration::from_millis(300));
            send(first_up, true);
            send(second_up, true);
            let events = drain(&rx);
            let raw = RAW_LOG.lock().unwrap().clone();
            println!("{}: events {:?}
  raw (vk, down, flags): {:?}", name, events, raw);
            assert_eq!(events, vec![HookEvent::ChordDown, HookEvent::ChordUp], "{}", name);
        }

        // The fallback's view of the keys, independent of the hook.
        send(VK_LCONTROL, false);
        send(VK_LWIN, false);
        mask_start_menu();
        assert!(chord_physically_held(), "both held");
        send(VK_LWIN, true);
        assert!(!chord_physically_held(), "Win released first");
        resync();
        assert!(CTRL_DOWN.load(Ordering::SeqCst) && !WIN_DOWN.load(Ordering::SeqCst) && !CHORD.load(Ordering::SeqCst));
        send(VK_LCONTROL, true);
        resync();
        assert!(!CTRL_DOWN.load(Ordering::SeqCst) && !WIN_DOWN.load(Ordering::SeqCst));
        drain(&rx);
    }
}
