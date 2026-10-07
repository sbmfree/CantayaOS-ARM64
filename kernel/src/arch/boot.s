// AArch64 kernel entry assembly
// Placed first in the link order via linker.ld `.text.boot` section.
//
// IMPORTANT: All .macro definitions must come BEFORE first use (LLVM single-pass).

// ─────────────────────────────────────────────────────────────────────────────
// Macros (defined first — LLVM assembler requires definition before use)
// ─────────────────────────────────────────────────────────────────────────────

// Save general-purpose and full FP/SIMD state. SavedRegs is the prefix.
.macro SAVE_CONTEXT
    sub  sp,  sp, #800
    stp  x0,  x1,  [sp, #0]
    stp  x2,  x3,  [sp, #16]
    stp  x4,  x5,  [sp, #32]
    stp  x6,  x7,  [sp, #48]
    stp  x8,  x9,  [sp, #64]
    stp  x10, x11, [sp, #80]
    stp  x12, x13, [sp, #96]
    stp  x14, x15, [sp, #112]
    stp  x16, x17, [sp, #128]
    stp  x18, x19, [sp, #144]
    stp  x20, x21, [sp, #160]
    stp  x22, x23, [sp, #176]
    stp  x24, x25, [sp, #192]
    stp  x26, x27, [sp, #208]
    stp  x28, x29, [sp, #224]
    mrs  x0,  ELR_EL1
    mrs  x1,  SPSR_EL1
    stp  x30, x0,  [sp, #240]
    str  x1,       [sp, #256]
    stp  q0, q1, [sp, #272]
    stp  q2, q3, [sp, #304]
    stp  q4, q5, [sp, #336]
    stp  q6, q7, [sp, #368]
    stp  q8, q9, [sp, #400]
    stp  q10, q11, [sp, #432]
    stp  q12, q13, [sp, #464]
    stp  q14, q15, [sp, #496]
    stp  q16, q17, [sp, #528]
    stp  q18, q19, [sp, #560]
    stp  q20, q21, [sp, #592]
    stp  q22, q23, [sp, #624]
    stp  q24, q25, [sp, #656]
    stp  q26, q27, [sp, #688]
    stp  q28, q29, [sp, #720]
    stp  q30, q31, [sp, #752]
    mrs  x0, FPCR
    mrs  x1, FPSR
    str  x0, [sp, #784]
    str  x1, [sp, #792]
    msr  FPCR, xzr
    msr  FPSR, xzr
.endm

.macro RESTORE_CONTEXT
    ldr  x0, [sp, #784]
    ldr  x1, [sp, #792]
    msr  FPCR, x0
    msr  FPSR, x1
    ldp  q0, q1, [sp, #272]
    ldp  q2, q3, [sp, #304]
    ldp  q4, q5, [sp, #336]
    ldp  q6, q7, [sp, #368]
    ldp  q8, q9, [sp, #400]
    ldp  q10, q11, [sp, #432]
    ldp  q12, q13, [sp, #464]
    ldp  q14, q15, [sp, #496]
    ldp  q16, q17, [sp, #528]
    ldp  q18, q19, [sp, #560]
    ldp  q20, q21, [sp, #592]
    ldp  q22, q23, [sp, #624]
    ldp  q24, q25, [sp, #656]
    ldp  q26, q27, [sp, #688]
    ldp  q28, q29, [sp, #720]
    ldp  q30, q31, [sp, #752]
    ldr  x1,       [sp, #256]
    ldp  x30, x0,  [sp, #240]
    msr  SPSR_EL1, x1
    msr  ELR_EL1,  x0
    ldp  x28, x29, [sp, #224]
    ldp  x26, x27, [sp, #208]
    ldp  x24, x25, [sp, #192]
    ldp  x22, x23, [sp, #176]
    ldp  x20, x21, [sp, #160]
    ldp  x18, x19, [sp, #144]
    ldp  x16, x17, [sp, #128]
    ldp  x14, x15, [sp, #112]
    ldp  x12, x13, [sp, #96]
    ldp  x10, x11, [sp, #80]
    ldp  x8,  x9,  [sp, #64]
    ldp  x6,  x7,  [sp, #48]
    ldp  x4,  x5,  [sp, #32]
    ldp  x2,  x3,  [sp, #16]
    ldp  x0,  x1,  [sp, #0]
    add  sp,  sp, #800
.endm

.macro EL1_EXCEPTION_HANDLER num
    SAVE_CONTEXT
    mov  x0, #\num
    mov  x1, sp
    bl   el1_exception_handler
    RESTORE_CONTEXT
    eret
.endm

.macro EL1_IRQHANDLER
    SAVE_CONTEXT
    mov  x0, sp
    bl   el1_irq_handler
    RESTORE_CONTEXT
    eret
.endm

// SVC from EL0 — inline expanded (no nested macro) to avoid LLVM issues
.macro EL0_SYNC_HANDLER
    SAVE_CONTEXT
    mrs  x0, ESR_EL1
    lsr  x0, x0, #26
    cmp  x0, #0x15
    b.eq 1f
    mov  x0, sp
    bl   el0_exception_handler
    b    2f
1:
    ldr  x0, [sp, #64]
    mov  x1, sp
    bl   syscall_dispatch
2:
    RESTORE_CONTEXT
    eret
.endm

.macro EL0_IRQ_HANDLER
    SAVE_CONTEXT
    mov  x0, sp
    bl   el0_irq_handler
    RESTORE_CONTEXT
    eret
.endm

// ─────────────────────────────────────────────────────────────────────────────
// Kernel entry (_start)
// ─────────────────────────────────────────────────────────────────────────────

    .section ".text.boot", "ax"
    .global _start
    .type   _start, @function

_start:
    msr DAIFSet, #0xf
    msr SPSel, #1
    mov  x19, x0

    // Enable FP/SIMD explicitly rather than inheriting the UEFI setting.
    mrs  x9, CPACR_EL1
    orr  x9, x9, #0x300000
    msr  CPACR_EL1, x9
    isb

    // Zero BSS (use adrp+add for ±4 GiB range; adr only has ±1 MiB)
    adrp x1, __bss_start
    add  x1, x1, :lo12:__bss_start
    adrp x2, __bss_end
    add  x2, x2, :lo12:__bss_end
1:  cmp  x1, x2
    b.ge 2f
    stp  xzr, xzr, [x1], #16
    b    1b
2:
    // Set exception vector table
    adrp x1, exception_vectors
    add  x1, x1, :lo12:exception_vectors
    msr  VBAR_EL1, x1
    isb

    mov  x0, x19
    bl   kernel_main

.Lhang:
    wfe
    b    .Lhang

    .size _start, . - _start


// ─────────────────────────────────────────────────────────────────────────────
// Exception vector table — 2 KiB aligned (AArch64 spec §D1.10.2)
// ─────────────────────────────────────────────────────────────────────────────

    .section ".text", "ax"
    .balign 2048
    .global exception_vectors
exception_vectors:

// ── Current EL / SP_EL0 ───────────────────────────────────────────────────
    .balign 128
vec_el1sp0_sync:    b .Lel1sp0_sync
    .balign 128
vec_el1sp0_irq:     b .Lel1sp0_irq
    .balign 128
vec_el1sp0_fiq:     b .Lel1sp0_fiq
    .balign 128
vec_el1sp0_serror:  b .Lel1sp0_serror

// ── Current EL / SP_EL1 ───────────────────────────────────────────────────
    .balign 128
vec_el1sp1_sync:    b .Lel1sp1_sync
    .balign 128
vec_el1sp1_irq:     b .Lel1sp1_irq
    .balign 128
vec_el1sp1_fiq:     b .Lel1sp1_fiq
    .balign 128
vec_el1sp1_serror:  b .Lel1sp1_serror

// ── Lower EL (AArch64) ────────────────────────────────────────────────────
    .balign 128
vec_el0_sync:       b .Lel0_sync
    .balign 128
vec_el0_irq:        b .Lel0_irq
    .balign 128
vec_el0_fiq:        b .Lel0_fiq
    .balign 128
vec_el0_serror:     b .Lel0_serror

// ── Lower EL (AArch32) — not supported ────────────────────────────────────
    .balign 128
    b .Lel0_aarch32_sync
    .balign 128
    b .Lel0_aarch32_irq
    .balign 128
    b .Lel0_aarch32_fiq
    .balign 128
    b .Lel0_aarch32_serror

// Each vector entry is exactly one branch instruction.  The architectural
// vector slots are only 128 bytes wide, whereas a complete save/restore
// sequence is larger; keeping handlers outside the table avoids entries
// overflowing into the next slot.
    .balign 2048
.Lel1sp0_sync:       EL1_EXCEPTION_HANDLER 0
.Lel1sp0_irq:        EL1_IRQHANDLER
.Lel1sp0_fiq:        EL1_EXCEPTION_HANDLER 2
.Lel1sp0_serror:     EL1_EXCEPTION_HANDLER 3

.Lel1sp1_sync:       EL1_EXCEPTION_HANDLER 4
.Lel1sp1_irq:        EL1_IRQHANDLER
.Lel1sp1_fiq:        EL1_EXCEPTION_HANDLER 6
.Lel1sp1_serror:     EL1_EXCEPTION_HANDLER 7

.Lel0_sync:          EL0_SYNC_HANDLER
.Lel0_irq:           EL0_IRQ_HANDLER
.Lel0_fiq:           EL1_EXCEPTION_HANDLER 10
.Lel0_serror:        EL1_EXCEPTION_HANDLER 11

.Lel0_aarch32_sync:  EL1_EXCEPTION_HANDLER 12
.Lel0_aarch32_irq:   EL1_IRQHANDLER
.Lel0_aarch32_fiq:   EL1_EXCEPTION_HANDLER 14
.Lel0_aarch32_serror: EL1_EXCEPTION_HANDLER 15


// ─────────────────────────────────────────────────────────────────────────────
// Context switch
// ─────────────────────────────────────────────────────────────────────────────
// void arch_context_switch(ThreadContext *from, const ThreadContext *to);

    .global arch_context_switch
    .type   arch_context_switch, @function
arch_context_switch:
    stp  x19, x20, [x0, #0]
    stp  x21, x22, [x0, #16]
    stp  x23, x24, [x0, #32]
    stp  x25, x26, [x0, #48]
    stp  x27, x28, [x0, #64]
    stp  x29, x30, [x0, #80]
    mov  x9,  sp
    str  x9,       [x0, #96]
    mrs  x9,  ELR_EL1
    str  x9,       [x0, #104]
    mrs  x9,  SPSR_EL1
    str  x9,       [x0, #112]
    mrs  x9,  DAIF              // save interrupt mask state
    str  x9,       [x0, #120]
    mrs  x9,  SP_EL0            // save per-thread EL0 stack state
    str  x9,       [x0, #128]
    stp  q8, q9, [x0, #144]
    stp  q10, q11, [x0, #176]
    stp  q12, q13, [x0, #208]
    stp  q14, q15, [x0, #240]
    mrs  x9, FPCR
    str  x9, [x0, #272]
    mrs  x9, FPSR
    str  x9, [x0, #280]

    ldp  x19, x20, [x1, #0]
    ldp  x21, x22, [x1, #16]
    ldp  x23, x24, [x1, #32]
    ldp  x25, x26, [x1, #48]
    ldp  x27, x28, [x1, #64]
    ldp  x29, x30, [x1, #80]
    ldr  x9,       [x1, #96]
    mov  sp,  x9
    ldr  x9,       [x1, #104]
    msr  ELR_EL1, x9
    ldr  x9,       [x1, #112]
    msr  SPSR_EL1, x9
    ldr  x9,       [x1, #128]
    msr  SP_EL0, x9
    ldp  q8, q9, [x1, #144]
    ldp  q10, q11, [x1, #176]
    ldp  q12, q13, [x1, #208]
    ldp  q14, q15, [x1, #240]
    ldr  x9, [x1, #272]
    msr  FPCR, x9
    ldr  x9, [x1, #280]
    msr  FPSR, x9
    ldr  x9,       [x1, #120]  // restore interrupt mask state last
    msr  DAIF, x9

    ret
    .size arch_context_switch, . - arch_context_switch


// ─────────────────────────────────────────────────────────────────────────────
// Save the active context and initially enter EL0
// ─────────────────────────────────────────────────────────────────────────────
// void arch_context_save_and_enter_user(ThreadContext *from, u64 ttbr0_root,
//     u64 entry, u64 user_stack_top, u64 kernel_stack_top, u64 user_argument);

    .global arch_context_save_and_enter_user
    .type   arch_context_save_and_enter_user, @function
arch_context_save_and_enter_user:
    stp  x19, x20, [x0, #0]
    stp  x21, x22, [x0, #16]
    stp  x23, x24, [x0, #32]
    stp  x25, x26, [x0, #48]
    stp  x27, x28, [x0, #64]
    stp  x29, x30, [x0, #80]
    mov  x9,  sp
    str  x9,       [x0, #96]
    mrs  x9,  ELR_EL1
    str  x9,       [x0, #104]
    mrs  x9,  SPSR_EL1
    str  x9,       [x0, #112]
    mrs  x9,  DAIF
    str  x9,       [x0, #120]
    mrs  x9,  SP_EL0
    str  x9,       [x0, #128]
    stp  q8, q9, [x0, #144]
    stp  q10, q11, [x0, #176]
    stp  q12, q13, [x0, #208]
    stp  q14, q15, [x0, #240]
    mrs  x9, FPCR
    str  x9, [x0, #272]
    mrs  x9, FPSR
    str  x9, [x0, #280]

    mov  x6, x5
    msr  DAIFSet, #0xf
    msr  SPSel, #1
    msr  TTBR0_EL1, x1
    dsb  ishst
    tlbi vmalle1
    dsb  ish
    isb

    mov  sp, x4
    msr  SP_EL0, x3
    msr  ELR_EL1, x2
    mov  x5, xzr
    msr  SPSR_EL1, x5
    mov  x0, x3
    mov  x1, x6
    msr  FPCR, xzr
    msr  FPSR, xzr
    movi v0.16b, #0
    movi v1.16b, #0
    movi v2.16b, #0
    movi v3.16b, #0
    movi v4.16b, #0
    movi v5.16b, #0
    movi v6.16b, #0
    movi v7.16b, #0
    movi v8.16b, #0
    movi v9.16b, #0
    movi v10.16b, #0
    movi v11.16b, #0
    movi v12.16b, #0
    movi v13.16b, #0
    movi v14.16b, #0
    movi v15.16b, #0
    movi v16.16b, #0
    movi v17.16b, #0
    movi v18.16b, #0
    movi v19.16b, #0
    movi v20.16b, #0
    movi v21.16b, #0
    movi v22.16b, #0
    movi v23.16b, #0
    movi v24.16b, #0
    movi v25.16b, #0
    movi v26.16b, #0
    movi v27.16b, #0
    movi v28.16b, #0
    movi v29.16b, #0
    movi v30.16b, #0
    movi v31.16b, #0
    eret
    .size arch_context_save_and_enter_user, . - arch_context_save_and_enter_user
