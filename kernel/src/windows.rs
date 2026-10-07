//! Bounded, copied IPC between EL0 applications and the exclusive desktop.
//! Locks never span scheduling, and all raw waiters are cancelled before reaping.
use crate::{
    executive::{
        ke::spinlock::IrqState,
        ps::{process::EProcess, scheduler},
    },
    syscall::SavedRegs,
};
use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use cantaya_shared::desktop::*;
use spin::Mutex;

const INVALID_PARAMETER: u64 = 0xC000_000D;
const ACCESS_VIOLATION: u64 = 0xC000_0005;
const ACCESS_DENIED: u64 = 0xC000_0022;
const INVALID_HANDLE: u64 = 0xC000_0008;
const NO_MEMORY: u64 = 0xC000_0017;
const RETRY: u64 = 0xC000_022D;
const TIMEOUT: u64 = 0x102;
const EVENTS: usize = 32;

struct Window {
    info: WindowInfo,
    owner: Weak<EProcess>,
    pixels: Vec<u8>,
    events: VecDeque<DesktopEvent>,
    waiters: VecDeque<usize>,
}
struct State {
    windows: [Option<Window>; MAX_WINDOWS],
    next_id: u64,
    epoch: u64,
    acknowledged: u64,
    staging: [u8; MAX_BLIT_PIXELS * 4],
}
static STATE: Mutex<State> = Mutex::new(State {
    windows: [const { None }; MAX_WINDOWS],
    next_id: 1,
    epoch: 0,
    acknowledged: 0,
    staging: [0; MAX_BLIT_PIXELS * 4],
});
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let irq = IrqState::disable();
    let result = f(&mut STATE.lock());
    irq.restore();
    result
}
fn owned(window: &Window, process: &Arc<EProcess>) -> bool {
    window
        .owner
        .upgrade()
        .is_some_and(|owner| Arc::ptr_eq(&owner, process))
}
fn index(s: &State, id: u64) -> Result<usize, u64> {
    s.windows
        .iter()
        .position(|w| w.as_ref().is_some_and(|w| w.info.id == id))
        .ok_or(INVALID_HANDLE)
}
fn changed(s: &mut State) {
    s.epoch = s.epoch.checked_add(1).expect("window epoch exhausted");
}
fn wake(waiters: impl IntoIterator<Item = usize>) {
    let irq = IrqState::disable();
    for waiter in waiters {
        scheduler::wake_thread(waiter as *mut _);
    }
    irq.restore();
}
pub fn has_pending() -> bool {
    with_state(|s| s.epoch != s.acknowledged)
}
/// Called inside console ownership transitions. Do not reenter the console lock.
pub fn reset() {
    let irq = IrqState::disable();
    let waiters = with_state(|s| {
        let mut waiters = Vec::new();
        for window in &mut s.windows {
            if let Some(mut w) = window.take() {
                waiters.extend(w.waiters.drain(..));
            }
        }
        changed(s);
        s.acknowledged = s.epoch;
        waiters
    });
    wake(waiters);
    irq.restore();
}
pub fn process_exited(pid: u64) {
    let irq = IrqState::disable();
    let waiters = with_state(|s| {
        let mut waiters = Vec::new();
        for window in &mut s.windows {
            if window
                .as_ref()
                .is_some_and(|w| w.owner.upgrade().is_none_or(|p| p.pid.0 == pid))
            {
                if let Some(mut w) = window.take() {
                    waiters.extend(w.waiters.drain(..));
                }
            }
        }
        // Even a console-only app completion wakes the desktop to reap its
        // launch handle with a zero-tick completion poll.
        changed(s);
        waiters
    });
    wake(waiters);
    crate::console::wake_ready_waiter();
    irq.restore();
}
fn copy_out(process: &Arc<EProcess>, address: u64, bytes: &[u8]) -> u64 {
    if matches!(
        process.with_user_address_space(|a| a.copy_to_user(address, bytes)),
        Some(Ok(()))
    ) {
        0
    } else {
        ACCESS_VIOLATION
    }
}
fn copy_in(process: &Arc<EProcess>, address: u64, bytes: &mut [u8]) -> u64 {
    if matches!(
        process.with_user_address_space(|a| a.copy_from_user(address, bytes)),
        Some(Ok(()))
    ) {
        0
    } else {
        ACCESS_VIOLATION
    }
}
/// x0 writable ID, x1 ASCII title, x2 title length 1..31, x3/x4 content size.
pub fn sys_create(regs: &mut SavedRegs) -> u64 {
    let irq = IrqState::disable();
    let result = (|| {
        let Some(process) = scheduler::current_process() else {
            return ACCESS_DENIED;
        };
        if !crate::desktop::is_active() {
            return ACCESS_DENIED;
        }
        if !(1..=31).contains(&regs.x[2])
            || !(32..=MAX_WINDOW_WIDTH as u64).contains(&regs.x[3])
            || !(32..=MAX_WINDOW_HEIGHT as u64).contains(&regs.x[4])
        {
            return INVALID_PARAMETER;
        }
        if !matches!(
            process.with_user_address_space(|a| a.validate_user_writable_range(regs.x[0], 8)),
            Some(Ok(()))
        ) {
            return ACCESS_VIOLATION;
        }
        let mut title = [0u8; 32];
        let status = copy_in(&process, regs.x[1], &mut title[..regs.x[2] as usize]);
        if status != 0 {
            return status;
        }
        if !title[..regs.x[2] as usize]
            .iter()
            .all(|b| matches!(b, b' '..=b'~'))
        {
            return INVALID_PARAMETER;
        }
        with_state(|s| {
            let Some(slot) = s.windows.iter().position(Option::is_none) else {
                return NO_MEMORY;
            };
            if s.next_id == u64::MAX {
                return NO_MEMORY;
            }
            let mut pixels = Vec::new();
            let length = (regs.x[3] * regs.x[4] * 4) as usize;
            if pixels.try_reserve_exact(length).is_err() {
                return NO_MEMORY;
            }
            pixels.resize(length, 0);
            let mut events = VecDeque::new();
            if events.try_reserve_exact(EVENTS).is_err() {
                return NO_MEMORY;
            }
            let id = s.next_id;
            let status = copy_out(&process, regs.x[0], &id.to_le_bytes());
            if status != 0 {
                return status;
            }
            s.next_id += 1;
            s.windows[slot] = Some(Window {
                info: WindowInfo {
                    id,
                    revision: 1,
                    width: regs.x[3] as u32,
                    height: regs.x[4] as u32,
                    title,
                    ..WindowInfo::default()
                },
                owner: Arc::downgrade(&process),
                pixels,
                events,
                waiters: VecDeque::new(),
            });
            changed(s);
            0
        })
    })();
    if result == 0 {
        crate::console::wake_ready_waiter();
    }
    irq.restore();
    result
}
/// x0 owned ID, x1 source pixels, x2 linear pixel offset, x3 count 1..4096.
/// x4 optional expected revision (zero is the original unconditional ABI).
/// Validate/copy the complete source before changing any surface bytes.
pub fn sys_present(regs: &mut SavedRegs) -> u64 {
    let irq = IrqState::disable();
    let result = (|| {
        let Some(process) = scheduler::current_process() else {
            return ACCESS_DENIED;
        };
        with_state(|s| {
            let slot = match index(s, regs.x[0]) {
                Ok(i) => i,
                Err(e) => return e,
            };
            let w = s.windows[slot].as_ref().unwrap();
            if !owned(w, &process) {
                return ACCESS_DENIED;
            }
            // Optional revision guards clients against resizing during uploads.
            if regs.x[4] != 0 && regs.x[4] != w.info.revision {
                return RETRY;
            }
            let Some(end) = regs.x[2].checked_add(regs.x[3]) else {
                return INVALID_PARAMETER;
            };
            if !(1..=MAX_BLIT_PIXELS as u64).contains(&regs.x[3])
                || end > (w.pixels.len() / 4) as u64
            {
                return INVALID_PARAMETER;
            }
            let length = regs.x[3] as usize * 4;
            let status = copy_in(&process, regs.x[1], &mut s.staging[..length]);
            if status != 0 {
                return status;
            }
            let w = s.windows[slot].as_mut().unwrap();
            let offset = regs.x[2] as usize * 4;
            w.pixels[offset..offset + length].copy_from_slice(&s.staging[..length]);
            w.info.revision = w
                .info
                .revision
                .checked_add(1)
                .expect("surface revision exhausted");
            changed(s);
            0
        })
    })();
    if result == 0 {
        crate::console::wake_ready_waiter();
    }
    irq.restore();
    result
}
pub fn sys_read_event(regs: &mut SavedRegs) -> u64 {
    let Some(process) = scheduler::current_process() else {
        return ACCESS_DENIED;
    };
    with_state(|s| {
        let slot = match index(s, regs.x[0]) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let w = s.windows[slot].as_mut().unwrap();
        if !owned(w, &process) {
            return ACCESS_DENIED;
        }
        if !matches!(
            process.with_user_address_space(|a| a.validate_user_writable_range(regs.x[1], 16)),
            Some(Ok(()))
        ) {
            return ACCESS_VIOLATION;
        }
        let Some(event) = w.events.front() else {
            return TIMEOUT;
        };
        let mut bytes = [0u8; 16];
        for (chunk, value) in
            bytes
                .chunks_exact_mut(4)
                .zip([event.kind, event.code, event.value as u32, event.text])
        {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        let status = copy_out(&process, regs.x[1], &bytes);
        if status == 0 {
            w.events.pop_front();
        }
        status
    })
}
pub fn observe_or_register_waiter(process: &Arc<EProcess>, id: u64, waiter: usize) -> Option<i32> {
    with_state(|s| {
        let slot = match index(s, id) {
            Ok(i) => i,
            Err(e) => return Some(e as i32),
        };
        let w = s.windows[slot].as_mut().unwrap();
        if !owned(w, process) {
            return Some(ACCESS_DENIED as i32);
        }
        if !w.events.is_empty() {
            return Some(0);
        }
        if w.waiters.len() == 16 {
            return Some(NO_MEMORY as i32);
        }
        if w.waiters.try_reserve(1).is_err() {
            return Some(NO_MEMORY as i32);
        }
        assert!(!w.waiters.contains(&waiter), "duplicate window waiter");
        w.waiters.push_back(waiter);
        None
    })
}
pub fn cancel_waiter(id: u64, waiter: usize) {
    with_state(|s| {
        if let Ok(slot) = index(s, id) {
            s.windows[slot]
                .as_mut()
                .unwrap()
                .waiters
                .retain(|w| *w != waiter);
        }
    });
}
pub fn sys_wait_event(regs: &mut SavedRegs) -> u64 {
    let Some(process) = scheduler::current_process() else {
        return ACCESS_DENIED;
    };
    match scheduler::wait_for_window_event(process, regs.x[0]) {
        Ok(status) => status as u32 as u64,
        Err(_) => INVALID_PARAMETER,
    }
}
/// The application or desktop can destroy a surface and wake blocked readers.
pub fn sys_close(regs: &mut SavedRegs) -> u64 {
    let irq = IrqState::disable();
    let result = (|| {
        let Some(process) = scheduler::current_process() else {
            return ACCESS_DENIED;
        };
        let desktop = crate::desktop::owns_graphics(&process);
        let (status, waiters) = with_state(|s| {
            let slot = match index(s, regs.x[0]) {
                Ok(i) => i,
                Err(e) => return (e, VecDeque::new()),
            };
            if !desktop && !owned(s.windows[slot].as_ref().unwrap(), &process) {
                return (ACCESS_DENIED, VecDeque::new());
            }
            let window = s.windows[slot].take().unwrap();
            changed(s);
            (0, window.waiters)
        });
        wake(waiters);
        status
    })();
    if result == 0 {
        crate::console::wake_ready_waiter();
    }
    irq.restore();
    result
}
fn write_info(process: &Arc<EProcess>, address: u64, mut info: WindowInfo, epoch: u64) -> u64 {
    info.epoch = epoch;
    let mut bytes = [0u8; 64];
    bytes[0..8].copy_from_slice(&info.id.to_le_bytes());
    bytes[8..16].copy_from_slice(&info.revision.to_le_bytes());
    bytes[16..24].copy_from_slice(&info.epoch.to_le_bytes());
    bytes[24..28].copy_from_slice(&info.width.to_le_bytes());
    bytes[28..32].copy_from_slice(&info.height.to_le_bytes());
    bytes[32..64].copy_from_slice(&info.title);
    copy_out(process, address, &bytes)
}
/// x0 owned ID, x1 writable WindowInfo. Does not consume queued events.
pub fn sys_query(regs: &mut SavedRegs) -> u64 {
    let Some(process) = scheduler::current_process() else {
        return ACCESS_DENIED;
    };
    with_state(|s| {
        let slot = match index(s, regs.x[0]) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let w = s.windows[slot].as_ref().unwrap();
        if !owned(w, &process) {
            return ACCESS_DENIED;
        }
        write_info(&process, regs.x[1], w.info, s.epoch)
    })
}
/// x0 ID, x1/x2 new content size. Desktop-only, bounded and atomic on failure.
pub fn sys_resize(regs: &mut SavedRegs) -> u64 {
    let irq = IrqState::disable();
    let result = (|| {
        let Some(process) = scheduler::current_process() else {
            return ACCESS_DENIED;
        };
        if !crate::desktop::owns_graphics(&process) {
            return ACCESS_DENIED;
        }
        if !(32..=MAX_WINDOW_WIDTH as u64).contains(&regs.x[1])
            || !(32..=MAX_WINDOW_HEIGHT as u64).contains(&regs.x[2])
        {
            return INVALID_PARAMETER;
        }
        let (status, waiters) = with_state(|s| {
            let slot = match index(s, regs.x[0]) {
                Ok(i) => i,
                Err(e) => return (e, VecDeque::new()),
            };
            let w = s.windows[slot].as_mut().unwrap();
            let width = regs.x[1] as usize;
            let height = regs.x[2] as usize;
            if w.info.width as usize == width && w.info.height as usize == height {
                return (0, VecDeque::new());
            }
            let mut pixels = Vec::new();
            let length = width * height * 4;
            if pixels.try_reserve_exact(length).is_err() {
                return (NO_MEMORY, VecDeque::new());
            }
            pixels.resize(length, 0);
            let old_width = w.info.width as usize;
            let overlap = width.min(old_width) * 4;
            for row in 0..height.min(w.info.height as usize) {
                pixels[row * width * 4..row * width * 4 + overlap]
                    .copy_from_slice(&w.pixels[row * old_width * 4..row * old_width * 4 + overlap]);
            }
            w.pixels = pixels;
            w.info.width = width as u32;
            w.info.height = height as u32;
            w.info.revision = w
                .info
                .revision
                .checked_add(1)
                .expect("surface revision exhausted");
            // Old pointer coordinates and held gestures no longer apply.
            w.events.clear();
            w.events.push_back(DesktopEvent {
                kind: RESET,
                ..DesktopEvent::default()
            });
            w.events.push_back(DesktopEvent {
                kind: RESIZE,
                value: width as i32,
                text: height as u32,
                ..DesktopEvent::default()
            });
            let waiters = core::mem::take(&mut w.waiters);
            changed(s);
            (0, waiters)
        });
        wake(waiters);
        status
    })();
    if result == 0 {
        crate::console::wake_ready_waiter();
    }
    irq.restore();
    result
}

/// x0 slot 0..3, x1 writable 64-byte WindowInfo. Desktop only.
pub fn sys_enumerate(regs: &mut SavedRegs) -> u64 {
    let irq = IrqState::disable();
    let result = (|| {
        let Some(process) = scheduler::current_process() else {
            return ACCESS_DENIED;
        };
        if !crate::desktop::owns_graphics(&process) {
            return ACCESS_DENIED;
        }
        if regs.x[0] >= MAX_WINDOWS as u64 {
            return INVALID_PARAMETER;
        }
        with_state(|s| {
            let info = s.windows[regs.x[0] as usize]
                .as_ref()
                .map_or(WindowInfo::default(), |w| w.info);
            write_info(&process, regs.x[1], info, s.epoch)
        })
    })();
    irq.restore();
    result
}
/// x0 ID, x1 output, x2 pixel offset, x3 count, x4 expected revision.
pub fn sys_copy(regs: &mut SavedRegs) -> u64 {
    let irq = IrqState::disable();
    let result = (|| {
        let Some(process) = scheduler::current_process() else {
            return ACCESS_DENIED;
        };
        if !crate::desktop::owns_graphics(&process) {
            return ACCESS_DENIED;
        }
        with_state(|s| {
            let slot = match index(s, regs.x[0]) {
                Ok(i) => i,
                Err(e) => return e,
            };
            let w = s.windows[slot].as_ref().unwrap();
            let Some(end) = regs.x[2].checked_add(regs.x[3]) else {
                return INVALID_PARAMETER;
            };
            if !(1..=MAX_BLIT_PIXELS as u64).contains(&regs.x[3])
                || end > (w.pixels.len() / 4) as u64
            {
                return INVALID_PARAMETER;
            }
            if regs.x[4] != w.info.revision {
                return RETRY;
            }
            copy_out(
                &process,
                regs.x[1],
                &w.pixels[regs.x[2] as usize * 4..end as usize * 4],
            )
        })
    })();
    irq.restore();
    result
}
/// Desktop routes validated keys and content-local pointer coordinates.
pub fn sys_send_event(regs: &mut SavedRegs) -> u64 {
    let irq = IrqState::disable();
    let result = (|| {
        let Some(process) = scheduler::current_process() else {
            return ACCESS_DENIED;
        };
        if !crate::desktop::owns_graphics(&process) {
            return ACCESS_DENIED;
        }
        let mut bytes = [0u8; 16];
        let status = copy_in(&process, regs.x[1], &mut bytes);
        if status != 0 {
            return status;
        }
        let field = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let event = DesktopEvent {
            kind: field(0),
            code: field(4),
            value: field(8) as i32,
            text: field(12),
        };
        let (status, waiters) = with_state(|s| {
            let slot = match index(s, regs.x[0]) {
                Ok(i) => i,
                Err(e) => return (e, VecDeque::new()),
            };
            let w = s.windows[slot].as_mut().unwrap();
            let valid = match event.kind {
                KEY => event.code <= 767 && (0..=2).contains(&event.value) && event.text <= 127,
                POINTER => {
                    event.code <= 7
                        && event.value >= 0
                        && (event.value as u32) < w.info.width
                        && event.text < w.info.height
                }
                RESET => event.code == 0 && event.value == 0 && event.text == 0,
                FOCUS => event.code == 0 && (0..=1).contains(&event.value) && event.text == 0,
                _ => false,
            };
            if !valid {
                return (INVALID_PARAMETER, VecDeque::new());
            }
            if w.events.len() == EVENTS {
                w.events.clear();
                w.events.push_back(DesktopEvent {
                    kind: RESET,
                    ..DesktopEvent::default()
                });
            }
            w.events.push_back(event);
            (0, core::mem::take(&mut w.waiters))
        });
        wake(waiters);
        status
    })();
    irq.restore();
    result
}
pub fn sys_acknowledge(regs: &mut SavedRegs) -> u64 {
    let irq = IrqState::disable();
    let result = (|| {
        let Some(process) = scheduler::current_process() else {
            return ACCESS_DENIED;
        };
        if !crate::desktop::owns_graphics(&process) {
            return ACCESS_DENIED;
        }
        with_state(|s| {
            if regs.x[0] > s.epoch {
                return INVALID_PARAMETER;
            }
            s.acknowledged = s.acknowledged.max(regs.x[0]);
            0
        })
    })();
    irq.restore();
    result
}
