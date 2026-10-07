.section ".text", "ax"
.global el0_simd_probe
.type el0_simd_probe, @function
el0_simd_probe:
    sub sp, sp, #752
    stp x19, x20, [sp, #0]
    stp x21, x22, [sp, #16]
    stp x23, x30, [sp, #32]
    stp q8, q9, [sp, #48]
    stp q10, q11, [sp, #80]
    stp q12, q13, [sp, #112]
    stp q14, q15, [sp, #144]
    mrs x9, FPCR
    mrs x10, FPSR
    stp x9, x10, [sp, #176]
    and x19, x0, #0xff
    mov x0, #1
    add x1, sp, #192
    mov x8, #0x36
    svc #0
    cbnz x0, .Lsimd_fail
    ldr x21, [sp, #192]
    mov x22, #0x400000
    mov x23, #0x8000000
    msr FPCR, x22
    msr FPSR, x23
    dup v0.16b, w19
    dup v1.16b, w19
    dup v2.16b, w19
    dup v3.16b, w19
    dup v4.16b, w19
    dup v5.16b, w19
    dup v6.16b, w19
    dup v7.16b, w19
    dup v8.16b, w19
    dup v9.16b, w19
    dup v10.16b, w19
    dup v11.16b, w19
    dup v12.16b, w19
    dup v13.16b, w19
    dup v14.16b, w19
    dup v15.16b, w19
    dup v16.16b, w19
    dup v17.16b, w19
    dup v18.16b, w19
    dup v19.16b, w19
    dup v20.16b, w19
    dup v21.16b, w19
    dup v22.16b, w19
    dup v23.16b, w19
    dup v24.16b, w19
    dup v25.16b, w19
    dup v26.16b, w19
    dup v27.16b, w19
    dup v28.16b, w19
    dup v29.16b, w19
    dup v30.16b, w19
    dup v31.16b, w19
.Lsimd_loop:
    mov x20, #64
.Lsimd_copy_loop:
    add x0, sp, #208
    mov x8, #0x40
    svc #0
    cbnz x0, .Lsimd_fail
    stp q0, q1, [sp, #240]
    stp q2, q3, [sp, #272]
    stp q4, q5, [sp, #304]
    stp q6, q7, [sp, #336]
    stp q8, q9, [sp, #368]
    stp q10, q11, [sp, #400]
    stp q12, q13, [sp, #432]
    stp q14, q15, [sp, #464]
    stp q16, q17, [sp, #496]
    stp q18, q19, [sp, #528]
    stp q20, q21, [sp, #560]
    stp q22, q23, [sp, #592]
    stp q24, q25, [sp, #624]
    stp q26, q27, [sp, #656]
    stp q28, q29, [sp, #688]
    stp q30, q31, [sp, #720]
    add x9, sp, #240
    mov x10, #512
.Lsimd_compare:
    ldrb w11, [x9], #1
    cmp w11, w19
    b.ne .Lsimd_fail
    subs x10, x10, #1
    b.ne .Lsimd_compare
    mrs x9, FPCR
    cmp x9, x22
    b.ne .Lsimd_fail
    mrs x9, FPSR
    cmp x9, x23
    b.ne .Lsimd_fail
    subs x20, x20, #1
    b.ne .Lsimd_copy_loop
    mov x0, #1
    add x1, sp, #192
    mov x8, #0x36
    svc #0
    cbnz x0, .Lsimd_fail
    ldr x9, [sp, #192]
    sub x9, x9, x21
    cmp x9, #5
    b.lo .Lsimd_loop
    mov x0, #0
    b .Lsimd_restore
.Lsimd_fail:
    mov x0, #1
.Lsimd_restore:
    ldp x9, x10, [sp, #176]
    msr FPCR, x9
    msr FPSR, x10
    ldp q8, q9, [sp, #48]
    ldp q10, q11, [sp, #80]
    ldp q12, q13, [sp, #112]
    ldp q14, q15, [sp, #144]
    ldp x19, x20, [sp, #0]
    ldp x21, x22, [sp, #16]
    ldp x23, x30, [sp, #32]
    add sp, sp, #752
    ret
.size el0_simd_probe, . - el0_simd_probe
