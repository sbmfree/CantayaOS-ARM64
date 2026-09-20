/* CantayaOS init process linker script. */

ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    /* The first free L1 slot in the kernel-compatible TTBR0 root. */
    . = 0x0000000100000000;

    .text ALIGN(4096) : {
        KEEP(*(.text.start))
        *(.text .text.*)
    } :text

    .rodata ALIGN(4096) : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    } :rodata

    .data ALIGN(4096) : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    } :data

    .bss ALIGN(4096) (NOLOAD) : {
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note*)
    }
}