//! Shared terminal output for the kernel prompt and bounded EL0 console writes.

pub fn write(args: core::fmt::Arguments<'_>) {
    crate::hal::uart::write_console(args);
    crate::hal::framebuffer::write_fmt(args);
}
