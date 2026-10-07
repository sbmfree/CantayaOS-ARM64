//! Exclusive EL0 desktop: bounded copied blits, structured events, console mirror.
use crate::{
    executive::{ke::spinlock::IrqState, ps::process::EProcess},
    syscall::SavedRegs,
};
use alloc::sync::{Arc, Weak};
use cantaya_shared::desktop::{DesktopEvent, MAX_BLIT_PIXELS, RESET, TEXT};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

const INVALID_PARAMETER: u64 = 0xC000_000D;
const ACCESS_VIOLATION: u64 = 0xC000_0005;
const ACCESS_DENIED: u64 = 0xC000_0022;
const TIMEOUT: u64 = 0x102;
const NOT_SUPPORTED: u64 = 0xC000_00BB;
const EVENTS: usize = 128;
const OUTPUT: usize = 8192;
static ACTIVE: AtomicBool = AtomicBool::new(false);

struct State {
    owner: Option<Weak<EProcess>>,
    events: [DesktopEvent; EVENTS],
    event_head: usize,
    event_len: usize,
    output: [u8; OUTPUT],
    output_head: usize,
    output_len: usize,
    pixels: [u8; MAX_BLIT_PIXELS * 4],
}
static STATE: Mutex<State> = Mutex::new(State {
    owner: None,
    events: [DesktopEvent {
        kind: 0,
        code: 0,
        value: 0,
        text: 0,
    }; EVENTS],
    event_head: 0,
    event_len: 0,
    output: [0; OUTPUT],
    output_head: 0,
    output_len: 0,
    pixels: [0; MAX_BLIT_PIXELS * 4],
});

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let irq = IrqState::disable();
    let result = f(&mut STATE.lock());
    irq.restore();
    result
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn reset() {
    let irq = IrqState::disable();
    with_state(|s| {
        ACTIVE.store(false, Ordering::Relaxed);
        s.owner = None;
        s.event_head = 0;
        s.event_len = 0;
        s.output_head = 0;
        s.output_len = 0;
    });
    crate::windows::reset();
    irq.restore();
}

fn is_owner(s: &State, process: &Arc<EProcess>) -> bool {
    s.owner
        .as_ref()
        .and_then(Weak::upgrade)
        .is_some_and(|p| Arc::ptr_eq(&p, process))
}

/// Called with IRQs masked by the window syscall transaction.
pub fn owns_graphics(process: &Arc<EProcess>) -> bool {
    is_active() && crate::console::owns_input(process) && with_state(|s| is_owner(s, process))
}

pub fn process_exited(pid: u64) {
    let irq = IrqState::disable();
    let owner_exited = with_state(|s| {
        s.owner
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|p| p.pid.0 == pid)
    });
    if owner_exited {
        reset();
    }
    crate::windows::process_exited(pid);
    irq.restore();
}

pub fn has_pending() -> bool {
    is_active()
        && (with_state(|s| s.event_len != 0 || s.output_len != 0) || crate::windows::has_pending())
}

pub fn queue_event(event: DesktopEvent) {
    if !is_active() {
        return;
    }
    with_state(|s| {
        if s.event_len == EVENTS {
            // Cancel any held state before publishing new input after overflow.
            s.event_head = 0;
            s.event_len = 1;
            s.events[0] = DesktopEvent {
                kind: RESET,
                ..DesktopEvent::default()
            };
        }
        let tail = (s.event_head + s.event_len) % EVENTS;
        s.events[tail] = event;
        s.event_len += 1;
    });
}

pub fn capture(text: &str) {
    if !is_active() {
        return;
    }
    with_state(|s| {
        for byte in text.bytes() {
            if s.output_len == OUTPUT {
                s.output_head = (s.output_head + 1) % OUTPUT;
                s.output_len -= 1;
            }
            let tail = (s.output_head + s.output_len) % OUTPUT;
            s.output[tail] = byte;
            s.output_len += 1;
        }
    });
}

struct CaptureWriter;
impl core::fmt::Write for CaptureWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        capture(text);
        Ok(())
    }
}
pub fn write(args: core::fmt::Arguments) {
    use core::fmt::Write;
    if is_active() {
        let _ = CaptureWriter.write_fmt(args);
        crate::console::wake_ready_waiter();
    }
}

pub fn sys_query(regs: &mut SavedRegs) -> u64 {
    let Some(info) = crate::hal::framebuffer::display_info() else {
        return NOT_SUPPORTED;
    };
    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return ACCESS_DENIED;
    };
    let mut bytes = [0u8; 16];
    for (chunk, value) in
        bytes
            .chunks_exact_mut(4)
            .zip([info.width, info.height, info.version, info.max_blit_pixels])
    {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    let irq = IrqState::disable();
    let copied = process.with_user_address_space(|a| a.copy_to_user(regs.x[0], &bytes));
    irq.restore();
    if matches!(copied, Some(Ok(()))) {
        0
    } else {
        ACCESS_VIOLATION
    }
}

/// x0 pixels, x1 x, x2 y, x3 width, x4 height, x5 source stride in bytes.
/// Every source row is copied before the first framebuffer write.
pub fn sys_present(regs: &mut SavedRegs) -> u64 {
    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return ACCESS_DENIED;
    };
    if !crate::console::owns_input(&process) {
        return ACCESS_DENIED;
    }
    let Some(info) = crate::hal::framebuffer::display_info() else {
        return NOT_SUPPORTED;
    };
    let [source, x, y, width, height, stride] = regs.x[..6].try_into().unwrap();
    if width == 0
        || height == 0
        || width > u64::from(info.width)
        || height > u64::from(info.height)
        || x > u64::from(info.width) - width
        || y > u64::from(info.height) - height
        || width * height > MAX_BLIT_PIXELS as u64
        || stride < width * 4
        || stride > 4096
        || stride % 4 != 0
        || source
            .checked_add((height - 1) * stride + width * 4)
            .is_none()
    {
        return INVALID_PARAMETER;
    }
    with_state(|s| {
        // Recheck after entering the IRQ-masked transaction: a sibling may
        // have released the input claim before we acquired this state.
        if !crate::console::owns_input(&process) {
            return ACCESS_DENIED;
        }
        let copied = process.with_user_address_space(|a| {
            for row in 0..height {
                let start = (row * width * 4) as usize;
                a.copy_from_user(
                    source + row * stride,
                    &mut s.pixels[start..start + width as usize * 4],
                )?;
            }
            Ok::<(), crate::arch::mmu::PageMapError>(())
        });
        if !matches!(copied, Some(Ok(()))) {
            return ACCESS_VIOLATION;
        }
        if !is_active() {
            s.owner = Some(Arc::downgrade(&process));
            ACTIVE.store(true, Ordering::Relaxed);
        }
        if !is_owner(s, &process) {
            return ACCESS_DENIED;
        }
        crate::hal::framebuffer::blit(
            x as u32,
            y as u32,
            width as u32,
            height as u32,
            &s.pixels[..(width * height * 4) as usize],
        );
        0
    })
}

/// x0 writable event. Invalid output leaves both event and serial queues intact.
pub fn sys_read_event(regs: &mut SavedRegs) -> u64 {
    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return ACCESS_DENIED;
    };
    if !crate::console::owns_input(&process) {
        return ACCESS_DENIED;
    }
    with_state(|s| {
        if !is_owner(s, &process) {
            return ACCESS_DENIED;
        }
        let valid =
            process.with_user_address_space(|a| a.validate_user_writable_range(regs.x[0], 16));
        if !matches!(valid, Some(Ok(()))) {
            return ACCESS_VIOLATION;
        }
        let queued = s.event_len != 0;
        let event = if queued {
            s.events[s.event_head]
        } else if let Some(byte) = crate::hal::uart::try_read_byte() {
            DesktopEvent {
                kind: TEXT,
                text: u32::from(byte),
                ..DesktopEvent::default()
            }
        } else {
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
        let copied = process.with_user_address_space(|a| a.copy_to_user(regs.x[0], &bytes));
        if !matches!(copied, Some(Ok(()))) {
            return ACCESS_VIOLATION;
        }
        if queued {
            s.event_head = (s.event_head + 1) % EVENTS;
            s.event_len -= 1;
        }
        0
    })
}

/// x0 buffer (1..1024 bytes), x1 capacity, x2 writable count. Empty = TIMEOUT.
pub fn sys_read_output(regs: &mut SavedRegs) -> u64 {
    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return ACCESS_DENIED;
    };
    if !crate::console::owns_input(&process) {
        return ACCESS_DENIED;
    }
    if !(1..=1024).contains(&regs.x[1]) {
        return INVALID_PARAMETER;
    }
    let Some(buffer_end) = regs.x[0].checked_add(regs.x[1]) else {
        return INVALID_PARAMETER;
    };
    let Some(count_end) = regs.x[2].checked_add(8) else {
        return INVALID_PARAMETER;
    };
    if regs.x[0] < count_end && regs.x[2] < buffer_end {
        return INVALID_PARAMETER;
    }
    with_state(|s| {
        if !is_owner(s, &process) {
            return ACCESS_DENIED;
        }
        let copied = process.with_user_address_space(|a| {
            a.validate_user_writable_range(regs.x[0], regs.x[1] as usize)?;
            a.validate_user_writable_range(regs.x[2], 8)?;
            if s.output_len == 0 {
                return Ok(false);
            }
            let count = s.output_len.min(regs.x[1] as usize);
            // The scratch buffer is shared with blits; IRQs are masked here.
            for i in 0..count {
                s.pixels[i] = s.output[(s.output_head + i) % OUTPUT];
            }
            a.copy_to_user(regs.x[0], &s.pixels[..count])?;
            a.copy_to_user(regs.x[2], &(count as u64).to_le_bytes())?;
            s.output_head = (s.output_head + count) % OUTPUT;
            s.output_len -= count;
            Ok::<bool, crate::arch::mmu::PageMapError>(true)
        });
        match copied {
            Some(Ok(true)) => 0,
            Some(Ok(false)) => TIMEOUT,
            _ => ACCESS_VIOLATION,
        }
    })
}
