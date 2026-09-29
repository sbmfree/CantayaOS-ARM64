//! PL011 UART driver.
//!
//! QEMU `virt` machine maps the UART at 0x0900_0000.
//! We drive it in polled mode (no FIFO interrupts needed for early boot).

use core::fmt::{self, Write};
use spin::Mutex;

const UART_BASE: usize = 0x0900_0000;

// PL011 register offsets
const DR: usize = 0x000; // Data register
const FR: usize = 0x018; // Flag register
const IBRD: usize = 0x024; // Integer baud rate divisor
const FBRD: usize = 0x028; // Fractional baud rate divisor
const LCR_H: usize = 0x02C; // Line control
const CR: usize = 0x030; // Control register
const IMSC: usize = 0x038; // Interrupt mask

const FR_TXFF: u32 = 1 << 5; // TX FIFO full
const FR_RXFE: u32 = 1 << 4; // RX FIFO empty
const CR_UARTEN: u32 = 1 << 0;
const CR_TXE: u32 = 1 << 8;
const CR_RXE: u32 = 1 << 9;
const LCR_WLEN8: u32 = 0b11 << 5;
const LCR_FEN: u32 = 1 << 4; // FIFO enable

#[inline]
fn uart_base() -> usize {
    if crate::arch::mmu::is_kernel_direct_map_ready() {
        crate::arch::mmu::phys_to_direct_map(UART_BASE as u64) as usize
    } else {
        UART_BASE
    }
}

#[inline]
unsafe fn mmio_write(offset: usize, val: u32) {
    unsafe { core::ptr::write_volatile((uart_base() + offset) as *mut u32, val) }
}

#[inline]
unsafe fn mmio_read(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((uart_base() + offset) as *const u32) }
}

/// Initialise the PL011 UART with 115200 8N1 at a 24 MHz UART clock.
///
/// # Safety
/// Must be called once, before any use of [`write_byte`].
pub unsafe fn init() {
    unsafe {
        // Disable UART
        mmio_write(CR, 0);
        // Baud: 115200  with UARTCLK=24MHz → BAUDDIV = 24e6 / (16 × 115200) ≈ 13.02
        // IBRD=13, FBRD=round(0.0208 × 64)=1
        mmio_write(IBRD, 13);
        mmio_write(FBRD, 1);
        // 8-bit, FIFO enabled
        mmio_write(LCR_H, LCR_WLEN8 | LCR_FEN);
        // Mask all interrupts
        mmio_write(IMSC, 0);
        // Enable UART, TX, RX
        mmio_write(CR, CR_UARTEN | CR_TXE | CR_RXE);
    }
}

/// Write a single byte (polls until TX FIFO has space).
#[inline]
pub fn write_byte(b: u8) {
    unsafe {
        while mmio_read(FR) & FR_TXFF != 0 {}
        mmio_write(DR, b as u32);
    }
}

/// Read a byte if available, or `None`.
#[inline]
pub fn try_read_byte() -> Option<u8> {
    unsafe {
        if mmio_read(FR) & FR_RXFE != 0 {
            None
        } else {
            Some(mmio_read(DR) as u8)
        }
    }
}

/// Check receive readiness without consuming the next PL011 byte.
#[inline]
pub fn has_input() -> bool {
    unsafe { mmio_read(FR) & FR_RXFE == 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// `core::fmt::Write` implementation
// ─────────────────────────────────────────────────────────────────────────────

pub struct Uart;

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                write_byte(b'\r');
            }
            write_byte(b);
        }
        Ok(())
    }
}

static UART_WRITER: Mutex<Uart> = Mutex::new(Uart);

/// Write terminal text without letting an IRQ logger interrupt the UART lock.
pub fn write_console(args: fmt::Arguments<'_>) {
    let daif_saved: u64;
    unsafe {
        core::arch::asm!(
            "mrs {0}, DAIF",
            "msr DAIFSet, #0xf",
            out(reg) daif_saved,
        );
    }

    {
        let mut writer = UART_WRITER.lock();
        let _ = writer.write_fmt(args);
    }

    unsafe { core::arch::asm!("msr DAIF, {0}", in(reg) daif_saved) };
}

/// `log` crate backend — writes to PL011 UART and the framebuffer console.
struct UartLogger;

impl log::Log for UartLogger {
    fn enabled(&self, _meta: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        use core::fmt::Write;

        let level = match record.level() {
            log::Level::Error => "ERROR",
            log::Level::Warn => " WARN",
            log::Level::Info => " INFO",
            log::Level::Debug => "DEBUG",
            log::Level::Trace => "TRACE",
        };

        // Disable IRQs while logging to prevent deadlock if the IRQ handler
        // also calls log! (which would try to re-acquire the UART lock).
        let daif_saved: u64;
        unsafe {
            core::arch::asm!(
                "mrs {0}, DAIF",
                "msr DAIFSet, #0xf",
                out(reg) daif_saved,
            );
        }

        // UART output
        {
            let mut w = UART_WRITER.lock();
            let _ = writeln!(w, "[{level}] {}", record.args());
        }

        // Framebuffer output (no-op until framebuffer::init() is called)
        crate::hal::framebuffer::write_fmt(format_args!("[{level}] {}\n", record.args()));

        // Restore interrupt state
        unsafe {
            core::arch::asm!("msr DAIF, {0}", in(reg) daif_saved);
        }
    }

    fn flush(&self) {}
}

static LOGGER: UartLogger = UartLogger;

/// Register the UART as the `log` crate backend.
pub fn init_logger() {
    log::set_logger(&LOGGER).expect("logger already set");
    log::set_max_level(log::LevelFilter::Trace);
}
