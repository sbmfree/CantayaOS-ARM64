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
    mov x9, #0x60
    cmp x1, x9
    b.eq process_wait_target_entry
    mov x9, #0x75
    cmp x1, x9
    b.eq handle_collision_owner_entry
    mov x9, #0x76
    cmp x1, x9
    b.eq handle_collision_child_entry
    mov x9, #0x77
    cmp x1, x9
    b.eq unexpected_creation_entry
    // Initial processes start with x1=0. Any other controlled value here is
    // an opaque parent-owned handle to probe from this empty child table.
    cbnz x1, handle_isolation_probe_entry

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

    // Terminate a sibling while it is blocked in a finite typed wait. Keep the
    // original target live past the sibling's former deadline after reaping so
    // a stale timeout or completion registration cannot wake freed state.
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
    adr x0, finite_blocked_thread_wait_target_handle
    adr x1, finite_blocked_thread_wait_target_entry
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
    adr x0, finite_blocked_thread_waiter_handle
    adr x1, finite_blocked_thread_waiting_sibling_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f

    adr x9, finite_blocked_thread_waiter_handle
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
    mov x1, #0x58
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
    mov x15, #0x58
    cmp x14, x15
    b.ne 2f

    adr x9, finite_blocked_thread_wait_target_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #60
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
    mov x1, #0x59
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
    mov x15, #0x59
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    adr x9, finite_blocked_thread_waiter_handle
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
    adr x1, finite_blocked_thread_wait_message
    adr x2, finite_blocked_thread_wait_message_end
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

    // Terminate a sibling waiting indefinitely on a child process completion.
    // A short finite wait on the live child reaps the sibling before the child
    // completes, so a stale process-completion registration is observable.
    sub sp, sp, #32
    adr x0, process_wait_target_handle
    mov x1, xzr
    mov x2, #0x60
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    adr x9, process_wait_target_handle
    ldr x13, [x9]
    cbz x13, 2f

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
    adr x0, process_waiter_handle
    adr x1, process_wait_infinite_sibling_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f

    adr x9, process_waiter_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #10
    str x14, [sp, #16]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #16
    mov x3, xzr
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f

    mov x0, x13
    mov x1, #0x5a
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #24]
    add x3, sp, #24
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #24]
    mov x15, #0x5a
    cmp x14, x15
    b.ne 2f

    adr x9, process_wait_target_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #2
    str x14, [sp, #16]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #16
    mov x3, xzr
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f

    mov x0, x13
    mov x1, #0x5b
    mov x8, #0x29
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #24]
    add x3, sp, #24
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #24]
    mov x15, #0x5b
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    adr x9, process_waiter_handle
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
    add sp, sp, #32

    movn x0, #0
    adr x1, process_wait_message
    adr x2, process_wait_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    // Repeat with a finite process-completion wait, then keep the child live
    // beyond the killed sibling's former deadline before terminating it.
    sub sp, sp, #32
    adr x0, process_wait_target_handle
    mov x1, xzr
    mov x2, #0x60
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    adr x9, process_wait_target_handle
    ldr x13, [x9]
    cbz x13, 2f

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
    adr x0, process_waiter_handle
    adr x1, process_wait_finite_sibling_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f

    adr x9, process_waiter_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #10
    str x14, [sp, #16]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #16
    mov x3, xzr
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f

    mov x0, x13
    mov x1, #0x5c
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #24]
    add x3, sp, #24
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #24]
    mov x15, #0x5c
    cmp x14, x15
    b.ne 2f

    adr x9, process_wait_target_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #60
    str x14, [sp, #16]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #16
    mov x3, xzr
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f

    mov x0, x13
    mov x1, #0x5d
    mov x8, #0x29
    svc #0
    cbnz x0, 2f

    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #24]
    add x3, sp, #24
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #24]
    mov x15, #0x5d
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    adr x9, process_waiter_handle
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
    add sp, sp, #32

    movn x0, #0
    adr x1, finite_process_wait_message
    adr x2, finite_process_wait_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    // Run two bounded mixed-lifecycle rounds. Each keeps finite waiters
    // registered on a live thread and process at the same time, externally
    // terminates both, reaps their raw records, passes their old deadlines,
    // then completes the original targets. Closing every handle and freeing
    // every stack before repeating also exercises resource reuse.
    mov x19, #2
mixed_lifecycle_round:
    sub sp, sp, #64
    adr x0, mixed_process_target_handle
    mov x1, xzr
    mov x2, #0x60
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    adr x9, mixed_process_target_handle
    ldr x13, [x9]
    cbz x13, 2f

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
    adr x0, mixed_thread_target_handle
    adr x1, mixed_thread_wait_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f

    // The second round reuses the first round's process and thread slots.
    // Old values must remain invalid for every existing lifecycle operation.
    cmp x19, #1
    b.ne mixed_stale_reuse_done
    adr x9, mixed_old_process_handle
    ldr x21, [x9]
    adr x9, mixed_process_target_handle
    ldr x22, [x9]
    cmp w21, w22
    b.ne 2f
    cmp x21, x22
    b.eq 2f
    adr x9, mixed_old_thread_handle
    ldr x23, [x9]
    adr x9, mixed_thread_target_handle
    ldr x24, [x9]
    cmp w23, w24
    b.ne 2f
    cmp x23, x24
    b.eq 2f

    movz x20, #0x8
    movk x20, #0xc000, lsl #16
    mov x0, x21
    mov x1, xzr
    mov x2, xzr
    mov x3, xzr
    mov x8, #0x4
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x21
    mov x1, #0x67
    mov x8, #0x29
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x21
    mov x8, #0xf
    svc #0
    cmp x0, x20
    b.ne 2f

    mov x0, x23
    mov x1, xzr
    mov x2, xzr
    mov x3, xzr
    mov x8, #0x4
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x23
    mov x1, #0x68
    mov x8, #0x30
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x23
    mov x8, #0xf
    svc #0
    cmp x0, x20
    b.ne 2f

    movn x0, #0
    adr x1, stale_handle_message
    adr x2, stale_handle_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f
mixed_stale_reuse_done:

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
    adr x0, mixed_thread_waiter_handle
    adr x1, mixed_thread_waiting_sibling_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f

    str xzr, [sp, #32]
    movn x0, #0
    add x1, sp, #32
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x11, [sp, #32]
    cbz x11, 2f
    str x11, [sp, #40]

    add x12, x11, #0x1000
    adr x0, mixed_process_waiter_handle
    adr x1, mixed_process_waiting_sibling_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f

    // Bound setup by waiting on both still-live waiters. The first interval
    // gives every mixed participant time to register; the second confirms the
    // process waiter remains blocked before either external termination.
    adr x9, mixed_thread_waiter_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #30
    str x14, [sp, #48]
    mov x14, #0x7e
    str x14, [sp, #56]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #48
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f
    ldr x14, [sp, #56]
    mov x15, #0x7e
    cmp x14, x15
    b.ne 2f

    adr x9, mixed_process_waiter_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #5
    str x14, [sp, #48]
    mov x14, #0x7e
    str x14, [sp, #56]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #48
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f
    ldr x14, [sp, #56]
    mov x15, #0x7e
    cmp x14, x15
    b.ne 2f

    adr x9, mixed_thread_waiter_handle
    ldr x13, [x9]
    mov x0, x13
    mov x1, #0x63
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    adr x9, mixed_process_waiter_handle
    ldr x13, [x9]
    mov x0, x13
    mov x1, #0x64
    mov x8, #0x30
    svc #0
    cbnz x0, 2f

    adr x9, mixed_thread_waiter_handle
    ldr x13, [x9]
    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #56]
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #56]
    mov x15, #0x63
    cmp x14, x15
    b.ne 2f

    adr x9, mixed_process_waiter_handle
    ldr x13, [x9]
    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #56]
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #56]
    mov x15, #0x64
    cmp x14, x15
    b.ne 2f

    // Scheduling this finite wait reaps both terminated waiters. Its 120-tick
    // interval also passes their 100-tick deadlines while both targets live.
    adr x9, mixed_thread_target_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x14, #120
    str x14, [sp, #48]
    mov x14, #0x7e
    str x14, [sp, #56]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #48
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    mov x14, #0x102
    cmp x0, x14
    b.ne 2f
    ldr x14, [sp, #56]
    mov x15, #0x7e
    cmp x14, x15
    b.ne 2f

    mov x0, x13
    mov x1, #0x65
    mov x8, #0x30
    svc #0
    cbnz x0, 2f
    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #56]
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #56]
    mov x15, #0x65
    cmp x14, x15
    b.ne 2f

    adr x9, mixed_process_target_handle
    ldr x13, [x9]
    cbz x13, 2f
    mov x0, x13
    mov x1, #0x66
    mov x8, #0x29
    svc #0
    cbnz x0, 2f
    mov x0, x13
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #56]
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #56]
    mov x15, #0x66
    cmp x14, x15
    b.ne 2f

    // In the second round, reuse each completed target's slot for the
    // opposite object type while the replacement remains live. A stale close
    // must not silently close that replacement.
    cmp x19, #1
    b.ne mixed_cross_type_reuse_done
    adr x9, mixed_process_target_handle
    ldr x21, [x9]
    mov x0, x21
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    adr x9, mixed_cross_thread_stack_base
    str xzr, [x9]
    movn x0, #0
    mov x1, x9
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    adr x9, mixed_cross_thread_stack_base
    ldr x11, [x9]
    cbz x11, 2f
    add x12, x11, #0x1000
    adr x0, mixed_cross_thread_handle
    adr x1, mixed_thread_wait_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f
    adr x9, mixed_cross_thread_handle
    ldr x22, [x9]
    cbz x22, 2f
    cmp w21, w22
    b.ne 2f
    cmp x21, x22
    b.eq 2f

    adr x9, mixed_thread_target_handle
    ldr x23, [x9]
    mov x0, x23
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    adr x0, mixed_cross_process_handle
    mov x1, xzr
    mov x2, #0x60
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    adr x9, mixed_cross_process_handle
    ldr x24, [x9]
    cbz x24, 2f
    cmp w23, w24
    b.ne 2f
    cmp x23, x24
    b.eq 2f

    movz x20, #0x8
    movk x20, #0xc000, lsl #16
    mov x14, #2
    str x14, [sp, #48]
    mov x0, x21
    mov x1, xzr
    add x2, sp, #48
    mov x3, xzr
    mov x8, #0x4
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x21
    mov x1, #0x67
    mov x8, #0x29
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x21
    mov x8, #0xf
    svc #0
    cmp x0, x20
    b.ne 2f

    mov x0, x23
    mov x1, xzr
    add x2, sp, #48
    mov x3, xzr
    mov x8, #0x4
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x23
    mov x1, #0x68
    mov x8, #0x30
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x23
    mov x8, #0xf
    svc #0
    cmp x0, x20
    b.ne 2f

    mov x0, x22
    mov x1, #0x69
    mov x8, #0x30
    svc #0
    cbnz x0, 2f
    mov x0, x22
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #56]
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #56]
    cmp x14, #0x69
    b.ne 2f
    mov x0, x24
    mov x1, #0x6a
    mov x8, #0x29
    svc #0
    cbnz x0, 2f
    mov x0, x24
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #56]
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #56]
    cmp x14, #0x6a
    b.ne 2f

    mov x0, x22
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    mov x0, x24
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    // The freed process slot has held a process and a thread. Issue two
    // more generations there: process, then thread. Keep both older values
    // and the intervening process value to test against the newest object.
    adr x0, mixed_churn_process_handle
    mov x1, xzr
    mov x2, #0x60
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    adr x9, mixed_churn_process_handle
    ldr x25, [x9]
    cbz x25, 2f
    cmp w21, w25
    b.ne 2f
    cmp x21, x25
    b.eq 2f
    cmp x22, x25
    b.eq 2f

    mov x27, xzr
mixed_churn_process_stale_loop:
    cmp x27, #0
    csel x28, x21, x22, eq
    mov x0, x28
    mov x1, xzr
    add x2, sp, #48
    mov x3, xzr
    mov x8, #0x4
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x28
    mov x8, #0xf
    svc #0
    cmp x0, x20
    b.ne 2f
    add x27, x27, #1
    cmp x27, #2
    b.ne mixed_churn_process_stale_loop

    mov x0, x25
    mov x1, #0x6b
    mov x8, #0x29
    svc #0
    cbnz x0, 2f
    mov x0, x25
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #56]
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #56]
    cmp x14, #0x6b
    b.ne 2f
    mov x0, x25
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    adr x9, mixed_cross_thread_stack_base
    ldr x11, [x9]
    add x12, x11, #0x1000
    adr x0, mixed_churn_thread_handle
    adr x1, mixed_thread_wait_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f
    adr x9, mixed_churn_thread_handle
    ldr x26, [x9]
    cbz x26, 2f
    cmp w21, w26
    b.ne 2f
    cmp x21, x26
    b.eq 2f
    cmp x22, x26
    b.eq 2f
    cmp x25, x26
    b.eq 2f

    mov x27, xzr
mixed_churn_thread_stale_loop:
    cmp x27, #2
    b.eq mixed_churn_select_process
    cmp x27, #0
    csel x28, x21, x22, eq
    b mixed_churn_stale_selected
mixed_churn_select_process:
    mov x28, x25
mixed_churn_stale_selected:
    mov x0, x28
    mov x1, xzr
    add x2, sp, #48
    mov x3, xzr
    mov x8, #0x4
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x28
    mov x8, #0xf
    svc #0
    cmp x0, x20
    b.ne 2f
    add x27, x27, #1
    cmp x27, #3
    b.ne mixed_churn_thread_stale_loop

    mov x0, x26
    mov x1, #0x6c
    mov x8, #0x30
    svc #0
    cbnz x0, 2f
    mov x0, x26
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #56]
    add x3, sp, #56
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x14, [sp, #56]
    cmp x14, #0x6c
    b.ne 2f
    mov x0, x26
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    adr x9, mixed_cross_thread_stack_base
    ldr x11, [x9]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f

    movn x0, #0
    adr x1, cross_type_stale_handle_message
    adr x2, cross_type_stale_handle_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f
    movn x0, #0
    adr x1, multi_generation_handle_message
    adr x2, multi_generation_handle_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f
    b mixed_original_target_handles_closed
mixed_cross_type_reuse_done:
    adr x9, mixed_process_target_handle
    ldr x0, [x9]
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    adr x9, mixed_thread_target_handle
    ldr x0, [x9]
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
mixed_original_target_handles_closed:
    adr x9, mixed_process_waiter_handle
    ldr x0, [x9]
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    adr x9, mixed_thread_waiter_handle
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
    ldr x11, [sp, #40]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f
    add sp, sp, #64

    movn x0, #0
    adr x1, mixed_lifecycle_message
    adr x2, mixed_lifecycle_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f
    subs x19, x19, #1
    b.eq mixed_lifecycle_done
    adr x9, mixed_process_target_handle
    ldr x10, [x9]
    adr x9, mixed_old_process_handle
    str x10, [x9]
    adr x9, mixed_thread_target_handle
    ldr x10, [x9]
    adr x9, mixed_old_thread_handle
    str x10, [x9]
    b mixed_lifecycle_round
mixed_lifecycle_done:

    // Failed creation must not publish a handle or start its target. The
    // second mixed round left the lowest slot closed in x26; successful
    // creations below must receive exactly its next generations.
    sub sp, sp, #48
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x21, [sp]
    cbz x21, 2f
    add x22, x21, #0x1000
    sub x23, x22, #4
    mov w9, #0x5a5a
    str w9, [x23]
    movz x19, #0xd
    movk x19, #0xc000, lsl #16
    movz x20, #0x5
    movk x20, #0xc000, lsl #16
    movz x27, #0x8
    movk x27, #0xc000, lsl #16

    mov x0, xzr
    adr x1, unexpected_creation_entry
    mov x2, x22
    mov x8, #0x4e
    svc #0
    cmp x0, x19
    b.ne 2f
    mov x0, #1
    adr x1, unexpected_creation_entry
    mov x2, x22
    mov x8, #0x4e
    svc #0
    cmp x0, x20
    b.ne 2f
    adr x0, message
    adr x1, unexpected_creation_entry
    mov x2, x22
    mov x8, #0x4e
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x23
    adr x1, unexpected_creation_entry
    mov x2, x22
    mov x8, #0x4e
    svc #0
    cmp x0, x20
    b.ne 2f
    ldr w9, [x23]
    mov w11, #0x5a5a
    cmp w9, w11
    b.ne 2f

    movz x9, #1, lsl #32
    add x10, x26, x9
    mov x0, x10
    mov x8, #0xf
    svc #0
    cmp x0, x27
    b.ne 2f
    str xzr, [sp, #8]
    add x0, sp, #8
    adr x1, mixed_thread_wait_target_entry
    mov x2, x22
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f
    ldr x24, [sp, #8]
    cmp x24, x10
    b.ne 2f
    mov x0, x24
    mov x1, #0x85
    mov x8, #0x30
    svc #0
    cbnz x0, 2f
    mov x0, x24
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #24]
    add x3, sp, #24
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x9, [sp, #24]
    cmp x9, #0x85
    b.ne 2f
    mov x0, x24
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    mov x0, xzr
    mov x1, xzr
    mov x2, #0x77
    mov x8, #0x4c
    svc #0
    cmp x0, x19
    b.ne 2f
    mov x0, #1
    mov x1, xzr
    mov x2, #0x77
    mov x8, #0x4c
    svc #0
    cmp x0, x20
    b.ne 2f
    adr x0, message
    mov x1, xzr
    mov x2, #0x77
    mov x8, #0x4c
    svc #0
    cmp x0, x20
    b.ne 2f
    mov x0, x23
    mov x1, xzr
    mov x2, #0x77
    mov x8, #0x4c
    svc #0
    cmp x0, x20
    b.ne 2f
    ldr w9, [x23]
    mov w11, #0x5a5a
    cmp w9, w11
    b.ne 2f

    movz x9, #1, lsl #32
    add x10, x24, x9
    mov x0, x10
    mov x8, #0xf
    svc #0
    cmp x0, x27
    b.ne 2f
    str xzr, [sp, #16]
    add x0, sp, #16
    mov x1, xzr
    mov x2, #0x60
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    ldr x25, [sp, #16]
    cmp x25, x10
    b.ne 2f
    mov x0, x25
    mov x1, #0x86
    mov x8, #0x29
    svc #0
    cbnz x0, 2f
    mov x0, x25
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #24]
    add x3, sp, #24
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x9, [sp, #24]
    cmp x9, #0x86
    b.ne 2f
    mov x0, x25
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    movn x0, #0
    mov x1, x21
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f
    add sp, sp, #48
    movn x0, #0
    adr x1, create_output_failure_message
    adr x2, create_output_failure_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    // A fresh selector-zero child has its own empty handle table. Give it a
    // live process value, then a live thread value; in each case its failed
    // operations must leave the parent's target usable.
    sub sp, sp, #48
    str xzr, [sp]
    mov x0, sp
    mov x1, xzr
    mov x2, #0x60
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    ldr x21, [sp]
    cbz x21, 2f

    str xzr, [sp, #8]
    add x0, sp, #8
    mov x1, xzr
    mov x2, x21
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    ldr x22, [sp, #8]
    cbz x22, 2f
    mov x9, #200
    str x9, [sp, #40]
    str xzr, [sp, #16]
    mov x0, x22
    mov x1, xzr
    add x2, sp, #40
    add x3, sp, #16
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x9, [sp, #16]
    cmp x9, #0x71
    b.ne 2f
    mov x0, x22
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    mov x0, x21
    mov x1, #0x72
    mov x8, #0x29
    svc #0
    cbnz x0, 2f
    mov x0, x21
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #16]
    add x3, sp, #16
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x9, [sp, #16]
    cmp x9, #0x72
    b.ne 2f
    mov x0, x21
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    str xzr, [sp, #24]
    movn x0, #0
    add x1, sp, #24
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, 2f
    ldr x11, [sp, #24]
    cbz x11, 2f
    add x12, x11, #0x1000
    str xzr, [sp, #32]
    add x0, sp, #32
    adr x1, mixed_thread_wait_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, 2f
    ldr x23, [sp, #32]
    cbz x23, 2f

    str xzr, [sp, #8]
    add x0, sp, #8
    mov x1, xzr
    mov x2, x23
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    ldr x24, [sp, #8]
    cbz x24, 2f
    mov x9, #200
    str x9, [sp, #40]
    str xzr, [sp, #16]
    mov x0, x24
    mov x1, xzr
    add x2, sp, #40
    add x3, sp, #16
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x9, [sp, #16]
    cmp x9, #0x71
    b.ne 2f
    mov x0, x24
    mov x8, #0xf
    svc #0
    cbnz x0, 2f

    mov x0, x23
    mov x1, #0x73
    mov x8, #0x30
    svc #0
    cbnz x0, 2f
    mov x0, x23
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #16]
    add x3, sp, #16
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x9, [sp, #16]
    cmp x9, #0x73
    b.ne 2f
    mov x0, x23
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    ldr x11, [sp, #24]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, 2f
    add sp, sp, #48

    movn x0, #0
    adr x1, process_local_handle_message
    adr x2, process_local_handle_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, 2f

    // A fresh controlled process creates a first thread handle equal to its
    // own child's first thread handle. Both tables must keep their targets
    // independent even while the numeric values collide.
    sub sp, sp, #32
    str xzr, [sp]
    mov x0, sp
    mov x1, xzr
    mov x2, #0x75
    mov x8, #0x4c
    svc #0
    cbnz x0, 2f
    ldr x13, [sp]
    cbz x13, 2f
    mov x9, #200
    str x9, [sp, #8]
    str xzr, [sp, #16]
    mov x0, x13
    mov x1, xzr
    add x2, sp, #8
    add x3, sp, #16
    mov x8, #0x4
    svc #0
    cbnz x0, 2f
    ldr x9, [sp, #16]
    cmp x9, #0x7d
    b.ne 2f
    mov x0, x13
    mov x8, #0xf
    svc #0
    cbnz x0, 2f
    add sp, sp, #32
    movn x0, #0
    adr x1, el0_numeric_collision_message
    adr x2, el0_numeric_collision_message_end
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

handle_isolation_probe_entry:
    mov x19, x1
    sub sp, sp, #16
    mov x9, #2
    str x9, [sp]
    movz x20, #8
    movk x20, #0xc000, lsl #16
    mov x0, x19
    mov x1, xzr
    mov x2, sp
    mov x3, xzr
    mov x8, #0x4
    svc #0
    cmp x0, x20
    b.ne handle_isolation_probe_failed
    mov x0, x19
    mov x1, #0x74
    mov x8, #0x29
    svc #0
    cmp x0, x20
    b.ne handle_isolation_probe_failed
    mov x0, x19
    mov x1, #0x74
    mov x8, #0x30
    svc #0
    cmp x0, x20
    b.ne handle_isolation_probe_failed
    mov x0, x19
    mov x8, #0xf
    svc #0
    cmp x0, x20
    b.ne handle_isolation_probe_failed

    movn x0, #0
    adr x1, process_local_child_message
    adr x2, process_local_child_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, handle_isolation_probe_failed
    movn x0, #0
    mov x1, #0x71
    mov x8, #0x29
    svc #0
handle_isolation_probe_failed:
    brk #0

handle_collision_owner_entry:
    sub sp, sp, #48
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, handle_collision_failed
    ldr x11, [sp]
    cbz x11, handle_collision_failed
    add x12, x11, #0x1000
    str xzr, [sp, #8]
    add x0, sp, #8
    adr x1, mixed_thread_wait_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, handle_collision_failed
    ldr x21, [sp, #8]
    cmp x21, #1
    b.ne handle_collision_failed

    str xzr, [sp, #16]
    add x0, sp, #16
    mov x1, xzr
    mov x2, #0x76
    mov x8, #0x4c
    svc #0
    cbnz x0, handle_collision_failed
    ldr x22, [sp, #16]
    cbz x22, handle_collision_failed
    mov x9, #200
    str x9, [sp, #24]
    str xzr, [sp, #32]
    mov x0, x22
    mov x1, xzr
    add x2, sp, #24
    add x3, sp, #32
    mov x8, #0x4
    svc #0
    cbnz x0, handle_collision_failed
    ldr x9, [sp, #32]
    cmp x9, #0x7b
    b.ne handle_collision_failed
    mov x0, x22
    mov x8, #0xf
    svc #0
    cbnz x0, handle_collision_failed

    // The child has already closed its handle value 1. Our value 1 still
    // names a live thread, so the finite wait must time out unchanged.
    mov x9, #2
    str x9, [sp, #24]
    mov x9, #0x7e
    str x9, [sp, #32]
    mov x0, x21
    mov x1, xzr
    add x2, sp, #24
    add x3, sp, #32
    mov x8, #0x4
    svc #0
    cmp x0, #0x102
    b.ne handle_collision_failed
    ldr x9, [sp, #32]
    cmp x9, #0x7e
    b.ne handle_collision_failed
    mov x0, x21
    mov x1, #0x7c
    mov x8, #0x30
    svc #0
    cbnz x0, handle_collision_failed
    mov x0, x21
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #32]
    add x3, sp, #32
    mov x8, #0x4
    svc #0
    cbnz x0, handle_collision_failed
    ldr x9, [sp, #32]
    cmp x9, #0x7c
    b.ne handle_collision_failed
    mov x0, x21
    mov x8, #0xf
    svc #0
    cbnz x0, handle_collision_failed
    ldr x11, [sp]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, handle_collision_failed

    movn x0, #0
    adr x1, el0_collision_owner_message
    adr x2, el0_collision_owner_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, handle_collision_failed
    movn x0, #0
    mov x1, #0x7d
    mov x8, #0x29
    svc #0
    b handle_collision_failed

handle_collision_child_entry:
    sub sp, sp, #32
    str xzr, [sp]
    movn x0, #0
    mov x1, sp
    mov x2, #0x1000
    mov x8, #0x15
    svc #0
    cbnz x0, handle_collision_failed
    ldr x11, [sp]
    cbz x11, handle_collision_failed
    add x12, x11, #0x1000
    str xzr, [sp, #8]
    add x0, sp, #8
    adr x1, mixed_thread_wait_target_entry
    mov x2, x12
    mov x8, #0x4e
    svc #0
    cbnz x0, handle_collision_failed
    ldr x21, [sp, #8]
    cmp x21, #1
    b.ne handle_collision_failed
    mov x0, x21
    mov x1, #0x78
    mov x8, #0x30
    svc #0
    cbnz x0, handle_collision_failed
    mov x0, x21
    mov x1, xzr
    mov x2, xzr
    str xzr, [sp, #24]
    add x3, sp, #24
    mov x8, #0x4
    svc #0
    cbnz x0, handle_collision_failed
    ldr x9, [sp, #24]
    cmp x9, #0x78
    b.ne handle_collision_failed
    mov x0, x21
    mov x8, #0xf
    svc #0
    cbnz x0, handle_collision_failed
    ldr x11, [sp]
    movn x0, #0
    mov x1, x11
    mov x2, #0x1000
    mov x8, #0x1b
    svc #0
    cbnz x0, handle_collision_failed

    movn x0, #0
    adr x1, el0_collision_child_message
    adr x2, el0_collision_child_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    cbnz x0, handle_collision_failed
    movn x0, #0
    mov x1, #0x7b
    mov x8, #0x29
    svc #0
handle_collision_failed:
    brk #0

unexpected_creation_entry:
    movn x0, #0
    adr x1, unexpected_creation_message
    adr x2, unexpected_creation_message_end
    sub x2, x2, x1
    mov x8, #0x8
    svc #0
    brk #0

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

finite_blocked_thread_wait_target_entry:
1:
    nop
    b 1b

finite_blocked_thread_waiting_sibling_entry:
    adr x9, finite_blocked_thread_wait_target_handle
    ldr x0, [x9]
    cbz x0, finite_blocked_thread_waiting_sibling_failed
    sub sp, sp, #16
    mov x9, #50
    str x9, [sp]
    mov x1, xzr
    mov x2, sp
    mov x3, xzr
    mov x8, #0x4
    svc #0

finite_blocked_thread_waiting_sibling_failed:
    brk #0

process_wait_target_entry:
1:
    nop
    b 1b

process_wait_infinite_sibling_entry:
    adr x9, process_wait_target_handle
    ldr x0, [x9]
    cbz x0, process_wait_infinite_sibling_failed
    mov x1, xzr
    mov x2, xzr
    mov x3, xzr
    mov x8, #0x4
    svc #0

process_wait_infinite_sibling_failed:
    brk #0

process_wait_finite_sibling_entry:
    adr x9, process_wait_target_handle
    ldr x0, [x9]
    cbz x0, process_wait_finite_sibling_failed
    sub sp, sp, #16
    mov x9, #50
    str x9, [sp]
    mov x1, xzr
    mov x2, sp
    mov x3, xzr
    mov x8, #0x4
    svc #0

process_wait_finite_sibling_failed:
    brk #0

mixed_thread_wait_target_entry:
1:
    nop
    b 1b

mixed_thread_waiting_sibling_entry:
    adr x9, mixed_thread_target_handle
    ldr x0, [x9]
    cbz x0, mixed_thread_waiting_sibling_failed
    sub sp, sp, #16
    mov x9, #100
    str x9, [sp]
    mov x1, xzr
    mov x2, sp
    mov x3, xzr
    mov x8, #0x4
    svc #0

mixed_thread_waiting_sibling_failed:
    brk #0

mixed_process_waiting_sibling_entry:
    adr x9, mixed_process_target_handle
    ldr x0, [x9]
    cbz x0, mixed_process_waiting_sibling_failed
    sub sp, sp, #16
    mov x9, #100
    str x9, [sp]
    mov x1, xzr
    mov x2, sp
    mov x3, xzr
    mov x8, #0x4
    svc #0

mixed_process_waiting_sibling_failed:
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
finite_blocked_thread_wait_message:
    .ascii "[user-init] finite blocked typed wait thread termination validated\n"
finite_blocked_thread_wait_message_end:
process_wait_message:
    .ascii "[user-init] blocked process wait thread termination validated\n"
process_wait_message_end:
finite_process_wait_message:
    .ascii "[user-init] finite blocked process wait thread termination validated\n"
finite_process_wait_message_end:
mixed_lifecycle_message:
    .ascii "[user-init] mixed finite lifecycle cancellation validated\n"
mixed_lifecycle_message_end:
stale_handle_message:
    .ascii "[user-init] stale typed handles rejected after reuse\n"
stale_handle_message_end:
cross_type_stale_handle_message:
    .ascii "[user-init] cross-type stale handles rejected after reuse\n"
cross_type_stale_handle_message_end:
multi_generation_handle_message:
    .ascii "[user-init] multi-generation stale handles rejected after churn\n"
multi_generation_handle_message_end:
process_local_child_message:
    .ascii "[user-init] isolated child rejected parent handle\n"
process_local_child_message_end:
process_local_handle_message:
    .ascii "[user-init] process-local parent handles validated\n"
process_local_handle_message_end:
el0_collision_child_message:
    .ascii "[user-init] colliding child thread handle completed\n"
el0_collision_child_message_end:
el0_collision_owner_message:
    .ascii "[user-init] colliding parent thread handle remained live\n"
el0_collision_owner_message_end:
el0_numeric_collision_message:
    .ascii "[user-init] EL0 numeric handle collision validated\n"
el0_numeric_collision_message_end:
create_output_failure_message:
    .ascii "[user-init] create output failures left no handles\n"
create_output_failure_message_end:
unexpected_creation_message:
    .ascii "[user-init] ERROR failed creation started target\n"
unexpected_creation_message_end:
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
finite_blocked_thread_wait_target_handle:
    .quad 0
finite_blocked_thread_waiter_handle:
    .quad 0
process_wait_target_handle:
    .quad 0
process_waiter_handle:
    .quad 0
mixed_process_target_handle:
    .quad 0
mixed_thread_target_handle:
    .quad 0
mixed_thread_waiter_handle:
    .quad 0
mixed_process_waiter_handle:
    .quad 0
mixed_old_process_handle:
    .quad 0
mixed_old_thread_handle:
    .quad 0
mixed_cross_thread_stack_base:
    .quad 0
mixed_cross_thread_handle:
    .quad 0
mixed_cross_process_handle:
    .quad 0
mixed_churn_process_handle:
    .quad 0
mixed_churn_thread_handle:
    .quad 0
