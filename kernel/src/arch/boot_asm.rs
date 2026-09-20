//! Include the AArch64 boot assembly via `global_asm!`.
//! This file is included from `arch/mod.rs` to pull the assembly into
//! the kernel ELF with the correct section attributes.

use core::arch::global_asm;

global_asm!(include_str!("boot.s"));
