//! ARM GICv2 driver.
//!
//! QEMU `virt` exposes GICv2 at:
//!   Distributor (GICD): 0x0800_0000
//!   CPU interface (GICC): 0x0801_0000

const GICD_BASE: usize = 0x0800_0000;
const GICC_BASE: usize = 0x0801_0000;

// GICD registers
const GICD_CTLR: usize = 0x000;
const GICD_TYPER: usize = 0x004;
const GICD_ISENABLER: usize = 0x100; // enable set  (32 IRQs per word)
const GICD_ICENABLER: usize = 0x180; // enable clear
const GICD_IPRIORITYR: usize = 0x400; // priority (8-bit per IRQ)
const GICD_ITARGETSR: usize = 0x800; // target CPU (8-bit per IRQ)
const GICD_ICFGR: usize = 0xC00; // config (2 bits per IRQ)

// GICC registers
const GICC_CTLR: usize = 0x000;
const GICC_PMR: usize = 0x004; // priority mask
const GICC_IAR: usize = 0x00C; // interrupt acknowledge
const GICC_EOIR: usize = 0x010; // end of interrupt

#[inline]
fn gicd_base() -> usize {
    crate::arch::mmu::phys_to_direct_map(GICD_BASE as u64) as usize
}

#[inline]
fn gicc_base() -> usize {
    crate::arch::mmu::phys_to_direct_map(GICC_BASE as u64) as usize
}

#[inline]
unsafe fn gicd_write(offset: usize, val: u32) {
    unsafe { core::ptr::write_volatile((gicd_base() + offset) as *mut u32, val) }
}
#[inline]
unsafe fn gicd_read(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((gicd_base() + offset) as *const u32) }
}
#[inline]
unsafe fn gicc_write(offset: usize, val: u32) {
    unsafe { core::ptr::write_volatile((gicc_base() + offset) as *mut u32, val) }
}
#[inline]
unsafe fn gicc_read(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((gicc_base() + offset) as *const u32) }
}

/// IRQ handler callback table — indexed by IRQ number.
/// Supports up to 256 IRQs; QEMU virt rarely needs more.
const MAX_IRQS: usize = 256;
static mut IRQ_HANDLERS: [Option<fn()>; MAX_IRQS] = [None; MAX_IRQS];

/// Initialise GICv2 distributor and CPU interface.
pub fn init() {
    unsafe {
        // Determine how many IRQs the GIC supports
        let typer = gicd_read(GICD_TYPER);
        let itlines = (typer & 0x1F) as usize + 1;
        let num_irqs = itlines * 32;

        // Disable all IRQs, set all to priority 0xA0, route to CPU 0
        for i in 0..num_irqs / 32 {
            gicd_write(GICD_ICENABLER + i * 4, 0xFFFF_FFFF);
        }
        for i in 0..num_irqs {
            // Priority register (8 bit per IRQ, packed as 4 per word)
            let reg = GICD_IPRIORITYR + i;
            let byte_offset = reg & !3;
            let shift = (reg & 3) * 8;
            let mut val = gicd_read(byte_offset);
            val &= !(0xFF << shift);
            val |= 0xA0 << shift;
            gicd_write(byte_offset, val);

            // Target CPU 0
            let reg = GICD_ITARGETSR + i;
            let byte_offset = reg & !3;
            let shift = (reg & 3) * 8;
            let mut val = gicd_read(byte_offset);
            val &= !(0xFF << shift);
            val |= 0x01 << shift;
            gicd_write(byte_offset, val);
        }

        // Enable distributor
        gicd_write(GICD_CTLR, 1);

        // CPU interface: accept all priority levels, enable
        gicc_write(GICC_PMR, 0xFF);
        gicc_write(GICC_CTLR, 1);
    }
}

/// Register a handler for the given IRQ number.
pub fn register_handler(irq: usize, handler: fn()) {
    assert!(irq < MAX_IRQS, "IRQ {} out of range", irq);
    unsafe { IRQ_HANDLERS[irq] = Some(handler) };
    enable_irq(irq);
}

/// Enable an IRQ in the distributor.
pub fn enable_irq(irq: usize) {
    let word = irq / 32;
    let bit = 1 << (irq % 32);
    unsafe { gicd_write(GICD_ISENABLER + word * 4, bit) };
}

/// Disable an IRQ in the distributor.
pub fn disable_irq(irq: usize) {
    let word = irq / 32;
    let bit = 1 << (irq % 32);
    unsafe { gicd_write(GICD_ICENABLER + word * 4, bit) };
}

/// Called by the IRQ exception vector.  Acknowledges the interrupt, runs the
/// registered handler (if any), then sends EOI.
pub fn handle_irq() {
    unsafe {
        let iar = gicc_read(GICC_IAR);
        let irq = (iar & 0x3FF) as usize;

        if irq < 1022 {
            // Timer handlers may switch contexts, so complete the active IRQ
            // before dispatch rather than after a potentially deferred return.
            gicc_write(GICC_EOIR, iar);
            if let Some(handler) = IRQ_HANDLERS.get(irq).and_then(|h| *h) {
                handler();
            }
        }
        // irq 1022/1023 are spurious — ignore
    }
}
