.section ".text.start", "ax"
.global _start
.type _start, @function

_start:
    mov x19, x1
    movn x0, #0
    adr x1, message
    adr x2, message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 1f

    cbz x19, 2f
    mov x20, x19
3:
    ldrb w21, [x20], #1
    cbnz w21, 3b
    sub x22, x20, x19
    sub x22, x22, #1
    sub sp, sp, #96
    mov x23, sp
    adr x24, argument_label
    mov x25, #10
4:
    ldrb w26, [x24], #1
    strb w26, [x23], #1
    subs x25, x25, #1
    b.ne 4b
    mov x24, x19
    mov x25, x22
5:
    ldrb w26, [x24], #1
    strb w26, [x23], #1
    subs x25, x25, #1
    b.ne 5b
    mov w26, #10
    strb w26, [x23]
    movn x0, #0
    mov x1, sp
    add x2, x22, #11
    mov x8, #0x8
    svc #0
    cbnz x0, 1f
    add sp, sp, #96

2:
    movn x0, #0
    mov x1, #0x2a
    mov x8, #0x29
    svc #0

1:
    brk #0

.size _start, . - _start

.section ".rodata", "a"
message:
    .ascii "Hello from CantayaOS!\n"
message_end:
argument_label:
    .ascii "Argument: "
