.section ".text.start", "ax"
.global _start
.type _start, @function

_start:
    movn x0, #0
    adr x1, message
    adr x2, message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 1f

    movn x0, #0
    mov x1, #0x43
    mov x8, #0x29
    svc #0

1:
    brk #0

.size _start, . - _start

.section ".rodata", "a"
message:
    .ascii "[user-child] FAT image executed\n"
message_end: