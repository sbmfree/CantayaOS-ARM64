//! AArch64 exception handlers called from `boot.s`.
//!
//! The assembly stubs save the full register context and call these Rust
//! functions. Faults are decoded into stable kernel types now, so EL0 process
//! support can evolve without replacing opaque panic strings later.

const STATUS_ACCESS_VIOLATION: i32 = 0xC000_0005u32 as i32;
const STATUS_STACK_OVERFLOW: i32 = 0xC000_00FDu32 as i32;
const STATUS_ILLEGAL_INSTRUCTION: i32 = 0xC000_001Du32 as i32;
const STATUS_DATATYPE_MISALIGNMENT: i32 = 0x8000_0002u32 as i32;
const STATUS_BREAKPOINT: i32 = 0x8000_0003u32 as i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionSource {
    El0,
    El1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultClass {
    InstructionAbort,
    DataAbort,
    Other(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortStatus {
    AddressSize(u8),
    Translation(u8),
    AccessFlag(u8),
    Permission(u8),
    Alignment,
    External(u8),
    Other(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultInfo {
    pub source: ExceptionSource,
    pub class: FaultClass,
    pub status: AbortStatus,
    pub esr: u64,
    pub elr: u64,
    pub far: u64,
    pub is_write: bool,
}

impl FaultInfo {
    pub fn is_abort(self) -> bool {
        matches!(
            self.class,
            FaultClass::InstructionAbort | FaultClass::DataAbort
        )
    }
}

/// Decode raw architectural fault registers into a kernel-level fault record.
pub fn decode_fault(source: ExceptionSource, esr: u64, elr: u64, far: u64) -> FaultInfo {
    let ec = ((esr >> 26) & 0x3F) as u8;
    let fsc = (esr & 0x3F) as u8;
    let class = match ec {
        0x20 | 0x21 => FaultClass::InstructionAbort,
        0x24 | 0x25 => FaultClass::DataAbort,
        other => FaultClass::Other(other),
    };
    let status = match fsc {
        0x00..=0x03 => AbortStatus::AddressSize(fsc & 0b11),
        0x04..=0x07 => AbortStatus::Translation(fsc & 0b11),
        0x09..=0x0B => AbortStatus::AccessFlag(fsc & 0b11),
        0x0D..=0x0F => AbortStatus::Permission(fsc & 0b11),
        0x10..=0x1F => AbortStatus::External(fsc),
        0x21 => AbortStatus::Alignment,
        other => AbortStatus::Other(other),
    };

    FaultInfo {
        source,
        class,
        status,
        esr,
        elr,
        far,
        is_write: matches!(class, FaultClass::DataAbort) && esr & (1 << 6) != 0,
    }
}

fn read_fault_info(source: ExceptionSource) -> FaultInfo {
    let esr: u64;
    let elr: u64;
    let far: u64;
    unsafe {
        core::arch::asm!("mrs {}, ESR_EL1", out(reg) esr);
        core::arch::asm!("mrs {}, ELR_EL1", out(reg) elr);
        core::arch::asm!("mrs {}, FAR_EL1", out(reg) far);
    }
    decode_fault(source, esr, elr, far)
}

fn el0_exception_status(fault: FaultInfo, stack_guard: bool) -> i32 {
    match fault.class {
        FaultClass::InstructionAbort | FaultClass::DataAbort => {
            if stack_guard {
                STATUS_STACK_OVERFLOW
            } else {
                STATUS_ACCESS_VIOLATION
            }
        }
        FaultClass::Other(0x22) | FaultClass::Other(0x26) => STATUS_DATATYPE_MISALIGNMENT,
        FaultClass::Other(0x3C) | FaultClass::Other(0x3D) => STATUS_BREAKPOINT,
        FaultClass::Other(_) => STATUS_ILLEGAL_INSTRUCTION,
    }
}

/// Called for all synchronous / FIQ / SError exceptions at EL1.
#[no_mangle]
pub extern "C" fn el1_exception_handler(kind: u64, _sp: *const u8) {
    let fault = read_fault_info(ExceptionSource::El1);
    panic!("EL1 exception kind={} fault={:?}", kind, fault);
}

/// Called for IRQ at EL1 (from the current EL SPx vector).
#[no_mangle]
pub extern "C" fn el1_irq_handler(_sp: *const u8) {
    crate::hal::gic::handle_irq();
}

/// Called for IRQ at EL1 which interrupted EL0. The vector-owned exception
/// frame retains all EL0 GPRs while the scheduler records SP_EL0 in the
/// thread context before switching away.
#[no_mangle]
pub extern "C" fn el0_irq_handler(_sp: *const u8) {
    static EL0_PREEMPTIONS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    if EL0_PREEMPTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) == 0 {
        log::info!("EL0 timer preemption captured");
    }
    crate::hal::gic::handle_irq();
}

/// Called for synchronous exceptions from EL0 that are not SVC.
///
/// EL0 faults are process-local: record a stable completion status for the
/// interrupted thread and let the scheduler switch to another runnable thread.
/// EL1 faults remain fatal because kernel recovery has no comparable boundary.
#[no_mangle]
pub extern "C" fn el0_exception_handler(_sp: *const u8) {
    let fault = read_fault_info(ExceptionSource::El0);
    let process = crate::executive::ps::scheduler::current_process()
        .expect("EL0 exception without a current process");
    let stack_guard = fault.is_abort()
        && process
            .with_user_address_space(|address_space| address_space.is_stack_guard_page(fault.far))
            .unwrap_or(false);
    let status = el0_exception_status(fault, stack_guard);
    log::error!(
        "EL0 controlled exception pid={} status={:#x} guard={} info={:?}",
        process.pid.0,
        status as u32,
        stack_guard,
        fault,
    );
    drop(process);
    crate::executive::ps::scheduler::terminate_current(status);
}
