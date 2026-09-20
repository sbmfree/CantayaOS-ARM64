#![no_std]
#![no_main]

use core::arch::global_asm;

global_asm!(include_str!("start.s"));

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
