/* CantayaOS kernel linker script — AArch64
 *
 * Virtual layout:
 *   0xFFFF_8000_0000_0000  — kernel text + rodata + data + bss
 *
 * Physical layout is determined by the bootloader ELF segment loads;
 * the MMU maps physical→virtual after paging is enabled.
 *
 * The kernel ELF entry symbol is `_start`.
 */

ENTRY(_start)

SECTIONS
{
    /*
     * Load at 0x4000_0000 (1 GiB physical / virtual).
     * The MMU init code will later add an upper-half alias.
     * Using a physical address here lets UEFI allocate the pages
     * and lets pre-MMU code use ADR/ADRP normally.
     */
    . = 0x40000000;

    .text ALIGN(4096) : {
        KEEP(*(.text.boot))   /* boot.s entry stub must come first */
        *(.text .text.*)
    }

    .rodata ALIGN(4096) : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    .data ALIGN(4096) : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    /* Thread-local storage (not used yet, but reserve the section) */
    .tdata ALIGN(4096) : { *(.tdata .tdata.*) }
    .tbss  ALIGN(4096) : { *(.tbss .tbss.*)  }

    .bss ALIGN(4096) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* Keep the kernel stack section symbol for BootInfo */
    .stack (NOLOAD) : ALIGN(4096) {
        __stack_bottom = .;
        . += 0x10000;   /* 64 KiB default kernel boot stack */
        __stack_top = .;
    }

    /* Discard unwanted sections */
    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note*)
    }
}
