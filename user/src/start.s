.section ".text.start", "ax"
.global _start
.type _start, @function

_start:
    mov x9, #0x5e
    cmp x1, x9
    b.eq process_exit_test_entry
    mov x9, #0x5f
    cmp x1, x9
    b.eq process_exit_test_entry

    // The kernel supplies this thread's initial user-stack top in x0. Keep a
    // cookie on that stack while timer IRQs force switches between processes.
    sub sp, sp, #16
    str x0, [sp]

    movz x9, #0x2d00
    movk x9, #0x0131, lsl #16
1:
    subs x9, x9, #1
    b.ne 1b

    ldr x10, [sp]
    cmp x10, x0
    b.ne 2f
    add sp, sp, #16

    movn x0, #0
    adr x1, message
    adr x2, message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0

    // Allocate and release one process-owned user page. The in/out base
    // address lives on the guarded EL0 stack and therefore exercises both
    // validated copy-in and copy-out.
    sub sp, sp, #16
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x11, [sp]
    cbz x11, 2f
    str x11, [sp, #8]

    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f

    // First-fit allocation must reuse the released region.
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x12, [sp]
    ldr x11, [sp, #8]
    cmp x12, x11
    b.ne 2f

    movn x0, #0
    mov x1, x12
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f

    // An explicitly requested base must map at that released address.
    ldr x11, [sp, #8]
    str x11, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x12, [sp]
    cmp x12, x11
    b.ne 2f

    movn x0, #0
    mov x1, x12
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f

    // Create a second EL0 thread, wait on its handle, close the handle, then
    // free the stack region after the thread has signaled completion.
    sub sp, sp, #32
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x11, [sp]
    cbz x11, 2f
    str x11, [sp, #8]

    add x12, x11, #0x1000
    mov x0, sp
    adr x1, worker_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f
    ldr x13, [sp]
    cbz x13, 2f

    mov x0, x13
    mov x1, #0x31
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #16]
    add x3, sp, #16
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #16]
    cmp x14, #0x31
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    ldr x11, [sp, #8]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f
    add sp, sp, #32

    // Create a sibling, terminate it by its typed thread handle while it is
    // still queued, then verify its requested final status. A repeated
    // termination of the signaled handle is a successful no-op.
    sub sp, sp, #32
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x11, [sp]
    cbz x11, 2f
    str x11, [sp, #8]

    add x12, x11, #0x1000
    mov x0, sp
    adr x1, external_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f
    ldr x13, [sp]
    cbz x13, 2f

    mov x14, #2
    str x14, [sp, #16]
    mov x14, #0x7e
    str x14, [sp, #24]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #16
    add x3, sp, #24
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f
    ldr x14, [sp, #24]
    mov x15, #0x7e
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x1, #0x52
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #16]
    add x3, sp, #16
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #16]
    mov x15, #0x52
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x1, #0x53
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #16]
    add x3, sp, #16
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #16]
    mov x15, #0x52
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    ldr x11, [sp, #8]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f
    add sp, sp, #32

    // Terminate a sibling while it is blocked indefinitely on another typed
    // thread handle. A later finite wait reaps that sibling before the target
    // is completed, proving the cancelled registration cannot wake stale state.
    sub sp, sp, #64
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x11, [sp]
    cbz x11, 2f
    str x11, [sp, #8]

    add x12, x11, #0x1000
    adr x0, blocked_thread_wait_target_handle
    adr x1, blocked_thread_wait_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f

    str xzr, [sp, #16]
    movn x0, #0
    add x1, sp, #16
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x11, [sp, #16]
    cbz x11, 2f
    str x11, [sp, #24]

    add x12, x11, #0x1000
    adr x0, blocked_thread_waiter_handle
    adr x1, blocked_thread_waiting_sibling_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f

    adr x9, blocked_thread_waiter_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #10
    str x14, [sp, #32]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #32
    mov x3, xzr
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f

    mov x0, x13
    mov x1, #0x56
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #40]
    add x3, sp, #40
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #40]
    mov x15, #0x56
    cmp x14, x15
    b.ne 2f

    adr x9, blocked_thread_wait_target_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #2
    str x14, [sp, #32]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #32
    mov x3, xzr
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f

    mov x0, x13
    mov x1, #0x57
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #40]
    add x3, sp, #40
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #40]
    mov x15, #0x57
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    adr x9, blocked_thread_waiter_handle
    ldr x0, [x9]
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    ldr x11, [sp, #8]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f
    ldr x11, [sp, #24]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f
    add sp, sp, #64

    movn x0, #0
    adr x1, blocked_thread_wait_message
    adr x2, blocked_thread_wait_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    movn x0, #0
    adr x1, terminate_thread_message
    adr x2, terminate_thread_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    movn x0, #0
    adr x1, finite_wait_message
    adr x2, finite_wait_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    movn x0, #0
    adr x1, handle_message
    adr x2, handle_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    movn x0, #0
    adr x1, vm_message
    adr x2, vm_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    // Query into the same user stack slot to validate a second copy-out path.
    mov x0, #0
    mov x1, sp
    mov x8, #0x36
    svc #0
    cbnz x0, 2f
    ldr x11, [sp]
    cbz x11, 2f
    add sp, sp, #16

    // A selector-zero child receives a controlled x1 mode value. It creates a
    // sibling blocked in a typed wait, terminates its own process through the
    // pseudo-handle, and the parent observes the requested final status.
    sub sp, sp, #16
    str xzr, [sp]
    mov x0, sp
    mov x1, xzr
    mov x2, #0x5e
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    ldr x13, [sp]
    cbz x13, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #8]
    add x3, sp, #8
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #8]
    mov x15, #0x61
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    add sp, sp, #16

    movn x0, #0
    adr x1, process_exit_wait_message
    adr x2, process_exit_wait_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    // Launch the same blocked-wait topology, allow setup to run, then use the
    // parent-owned process handle to terminate it and observe status 0x62.
    sub sp, sp, #16
    str xzr, [sp]
    mov x0, sp
    mov x1, xzr
    mov x2, #0x5f
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    ldr x13, [sp]
    cbz x13, 2f

    mov x14, #10
    str x14, [sp, #8]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #8
    mov x3, xzr
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f

    mov x0, x13
    mov x1, #0x62
    mov x8, #0x29
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #8]
    add x3, sp, #8
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #8]
    mov x15, #0x62
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    add sp, sp, #16

    movn x0, #0
    adr x1, external_process_wait_message
    adr x2, external_process_wait_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    // Spawn the fixed FAT-backed child image, verify its explicit completion
    // status, then close the handle.
    sub sp, sp, #16
    str xzr, [sp]
    mov x0, sp
    mov x1, #1
    mov x2, xzr
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    ldr x13, [sp]
    cbz x13, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #8]
    add x3, sp, #8
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #8]
    mov x15, #0x43
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    add sp, sp, #16

    movn x0, #0
    adr x1, child_message
    adr x2, child_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    movn x0, #0
    adr x1, process_message
    adr x2, process_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    movn x0, #0
    mov x1, #0x2a
    mov x8, #0x29
    svc #0

2:
    brk #0

3:
    wfe
    b 3b

worker_entry:
1:
    nop
    b 1b

process_exit_test_entry:
    mov x19, x1
    // Create a non-signaled target then a sibling that waits on its existing
    // typed handle. The finite wait below yields enough timer slices for the
    // sibling to register its infinite completion wait before self-termination.
    sub sp, sp, #32
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, process_exit_test_failed
    ldr x11, [sp]
    cbz x11, process_exit_test_failed
    add x12, x11, #0x1000
    adr x0, process_exit_wait_target_handle
    adr x1, process_exit_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, process_exit_test_failed

    str xzr, [sp, #8]
    movn x0, #0
    add x1, sp, #8
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, process_exit_test_failed
    ldr x11, [sp, #8]
    cbz x11, process_exit_test_failed
    add x12, x11, #0x1000
    adr x0, process_exit_waiter_handle
    adr x1, process_exit_waiting_sibling_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, process_exit_test_failed

    ldr x0, process_exit_waiter_handle
    cbz x0, process_exit_test_failed
    mov x9, #10
    str x9, [sp, #16]
    mov x1, xzr
    add x2, sp, #16
    mov x3, xzr
    mov x8, #0x4
    svc #0
    mov x9, #0x102
    cmp x0, x9
    b.ne process_exit_test_failed

    mov x9, #0x5e
    cmp x19, x9
    b.ne process_exit_external_target

    movn x0, #0
    mov x1, #0x61
    mov x8, #0x29
    svc #0
    brk #0

process_exit_external_target:
1:
    nop
    b 1b

process_exit_test_failed:
    brk #0

process_exit_target_entry:
1:
    nop
    b 1b

process_exit_waiting_sibling_entry:
    adr x9, process_exit_wait_target_handle
    ldr x0, [x9]
    cbz x0, process_exit_test_failed
    mov x1, xzr
    mov x2, xzr
    mov x3, xzr
    mov x8, #0x4
    svc #0
    brk #0

external_target_entry:
1:
    nop
    b 1b

blocked_thread_wait_target_entry:
1:
    nop
    b 1b

blocked_thread_waiting_sibling_entry:
    adr x9, blocked_thread_wait_target_handle
    ldr x0, [x9]
    cbz x0, blocked_thread_waiting_sibling_failed
    mov x1, xzr
    mov x2, xzr
    mov x3, xzr
    mov x8, #0x4
    svc #0

blocked_thread_waiting_sibling_failed:
    brk #0

.size _start, . - _start

.section ".rodata", "a"
message:
    .ascii "[user-init] EL0 context resume validated\n"
message_end:
vm_message:
    .ascii "[user-init] EL0 fixed VM reuse validated\n"
vm_message_end:
handle_message:
    .ascii "[user-init] EL0 thread handle wait validated\n"
handle_message_end:
terminate_thread_message:
    .ascii "[user-init] external thread handle termination validated\n"
terminate_thread_message_end:
finite_wait_message:
    .ascii "[user-init] finite typed wait timeout validated\n"
finite_wait_message_end:
blocked_thread_wait_message:
    .ascii "[user-init] blocked typed wait thread termination validated\n"
blocked_thread_wait_message_end:
process_exit_wait_message:
    .ascii "[user-init] process-wide blocked wait termination validated\n"
process_exit_wait_message_end:
external_process_wait_message:
    .ascii "[user-init] external process blocked wait termination validated\n"
external_process_wait_message_end:
process_message:
    .ascii "[user-init] EL0 process handle wait validated\n"
process_message_end:
child_message:
    .ascii "[user-init] FAT child status wait validated\n"
child_message_end:

.section ".data", "aw"
.align 3
process_exit_wait_target_handle:
    .quad 0
process_exit_waiter_handle:
    .quad 0
blocked_thread_wait_target_handle:
    .quad 0
blocked_thread_waiter_handle:
    .quad 0