# CantayaOS Architecture

This document describes stable boundaries in the current design. For active
implementation priorities and deliberate omissions, read [STATUS.md](../STATUS.md).

## System Shape

```text
UEFI firmware
  -> cantaya-boot.efi
  -> BootInfo handoff
  -> AArch64 kernel bootstrap and TTBR1 high-half execution
  -> HAL, drivers, and NT-style executive subsystems
  -> timer-scheduled EL0 processes with private TTBR0 roots
  -> CantayaOS banner and kernel terminal after both init processes exit
```

## UEFI Boot And Handoff

The UEFI bootloader is a PE32+ application. It initializes UEFI logging,
selects a 1024x768 Graphics Output Protocol mode when available, loads
`\\EFI\\CantayaOS\\kernel.elf`, and retains
`\\EFI\\CantayaOS\\init.elf` in loader-data memory for the initial EL0 processes.
It allocates the initial kernel stack, exits boot services, converts the UEFI
memory map, and branches to the kernel with a pointer to `BootInfo` in `x0`.

`shared` defines the `no_std`, `#[repr(C)]` handoff structures. `BootInfo`
contains a magic value, version, framebuffer descriptor, memory map, RSDP,
kernel physical base, kernel-stack top, and the retained initial-ELF physical
address and size. The kernel validates the magic before architecture setup.

## AArch64 Address Spaces And High-Half Execution

The kernel is loaded at the lower physical/link address `0x4000_0000`. During
bootstrap, TTBR0 supplies the mappings needed for platform RAM and devices.
TTBR1 maps the kernel at `0xffff800000000000`, and the kernel branches to that
alias before memory-manager, hardware, driver, and executive initialization.
Exception vectors execute through the high-half mapping.

TTBR1 also provides a direct physical-memory window for the first 16 GiB and a
dynamic kernel virtual-memory region beginning at `0xffff800100000000`. Kernel
stacks, dynamically allocated page tables, ELF backing pages, and supported
MMIO are accessed through the high-half mapping after it is ready.

Each EL0 process owns a private four-level TTBR0 root. Its user mappings are
explicitly installed and occupy the lower half of the 48-bit address space.
The root retains only a supervisor-only low alias of the still low-linked
kernel image at `0x4000_0000`; it grants no EL0 access and excludes low MMIO.
The low device identity mapping is therefore not inherited by user roots.

## Executive And Platform Boundaries

The kernel separates architecture, hardware abstraction, drivers, and an
NT-style executive:

- `arch`: MMU setup, exception vectors, fault decoding, and context-switch
  support.
- `hal`: PL011 UART, framebuffer console, GICv2, and ARM generic timer.
- `drivers`: VirtIO-MMIO block and input keyboard devices, plus FAT32.
- `shell`: the bounded kernel command prompt shown after boot validation.
- `Ke`: spinlocks, mutexes, DPCs, and waiting primitives.
- `Mm`: physical pages, heap, kernel virtual mappings, and user address-space
  ownership.
- `Ob`: typed objects and handle tables.
- `Ps`: `EPROCESS`, `ETHREAD`, executable loading, and timer-driven round-robin
  scheduling.
- `Io`: IRPs, driver/device dispatch, and the fixed image-read boundary.
- `Se`: security-reference-monitor scaffolding.

The layering is intentional: process loading consumes the I/O-owned file
object rather than FAT or VirtIO internals, while `Io` dispatches requests to a
fixed driver/device path.

## Boot Screen And Terminal

The two initial EL0 `init.elf` processes run the boot validation workload.
The System kernel thread waits for both processes to exit, then clears the
framebuffer boot log and draws a CantayaOS banner with the kernel version.
Terminal text occupies a scrolling pane below the banner. The prompt accepts
bounded ASCII lines and runs `help`, `info`, `uptime`, `mem`, `echo`, and `clear`
inside the kernel. Terminal output goes to both the framebuffer and PL011 UART.
The recurring System heartbeat and Thread-A/B demonstration loops are disabled.

QEMU attaches a modern VirtIO-MMIO keyboard. The driver scans the MMIO slots,
negotiates VirtIO 1, checks the advertised key bitmap, and polls its event
queue from the System thread. It decodes US ASCII keys plus Shift, Caps Lock,
Backspace, Enter, Ctrl-U, and Ctrl-L. The status queue is present, but keyboard
LED feedback is not implemented. PL011 serial input remains available through
`-serial stdio`. The terminal runs built-in kernel commands only.

## Processes, Threads, And Lifetime

`Ps` creates the initial pair of independently mapped EL0 `init.elf` processes
and places their threads on a timer-preemptive round-robin scheduler. A process
owns its TTBR0 mappings, page-table pages, executable image mappings, and
guarded EL0 stack. The page below each downward-growing user stack is unmapped
so stack overflow can be identified separately.

Thread context preserves `SP_EL0`; exception frames preserve EL0 general
registers, `ELR_EL1`, and `SPSR_EL1`. A thread's completion object can outlive
its scheduler record through typed handles. A terminated thread is reaped only
from a later active context, which prevents freeing its currently active kernel
stack. A process becomes signaled after its final active thread exits.
The scheduler keeps a separate internal set for threads blocked on retained
typed process or thread completions; it is distinct from the ready run queue
and the ordered finite-timeout queue. Wake and timeout remove a thread from
that set before it is made ready. `NtTerminateProcess(-1, status)` is
process-wide: it removes ready siblings and typed-completion-blocked siblings
from their scheduler-owned sets, clears their timer and completion
registrations, completes them with `status`, and defers their raw-record
reaping. It then completes the caller and schedules away; because the caller
is last in the process active-thread count, the retained `EProcess` completion
object signals the same final status. This ordering keeps every kernel stack
alive until a different context can reap it.
External `NtTerminateProcess(process_handle, status)` applies the same two
set removal and registration-cancellation pass to every non-current target
thread before it signals any target completion or queues raw records for
reaping. This prevents a target completion from waking another target through
a stale typed-completion registration.
`NtTerminateThread` accepts a parent-owned typed thread handle, rejects the
current thread, and removes a ready target or a typed-completion-blocked target
from the appropriate scheduler set before signaling its completion. A
completed target is a successful no-op; raw execution storage remains deferred
until another stack is active.
Each handle-table entry also carries table-local lifecycle rights. `WAIT` is
required for `NtWaitForSingleObject` and `TERMINATE` is required for external
process or thread termination. The table checks the requested rights before it
clones the retained typed object, so denied operations cannot observe it.
Handles encode a process-local slot and issuance generation. Closing a handle
advances the slot generation; a stale value cannot resolve or close a later
object in that slot. Exhausted generations are not reissued, and zero and the
current-process pseudo-handle are never real table handles.
These masks are limited handle metadata, not security tokens, ACLs, inheritance,
or a general object-permission system.

Finite waits share the existing `NtWaitForSingleObject` entry: its timeout
pointer may reference a relative `u64` count of one through 1,000 100 Hz
scheduler ticks. The scheduler records an absolute deadline for the sleeping
thread in an ordered internal queue; the timer IRQ readies only expired entries.
Before returning `STATUS_TIMEOUT`, the thread removes itself from the typed
process or thread completion queue, so later completion cannot wake a stale
waiter. A signaled finite wait retains the same final-status output behavior as
an infinite wait, while a timeout leaves that output untouched.

## Syscalls And User-Memory Validation

EL0 enters the syscall layer through `SVC #0`: `x8` selects a syscall and
`x0` through `x5` carry arguments. The exception handler supplies a saved
register frame and the dispatcher writes the result back to saved `x0`.

User pointers are never trusted directly. User-memory copy-in and copy-out
validate the complete required range and access mode before the kernel copies.
The same ownership model supports process-local, zeroed virtual-memory regions,
guarded stacks, W^X ELF mapping, and instruction-cache synchronization after
mapping executable pages. Detailed current syscall limits are in
[STATUS.md](../STATUS.md).

## Fixed VirtIO/FAT Image Path

QEMU attaches the boot image as an explicit modern VirtIO-MMIO block device.
The driver scans the standard MMIO slots, accepts only a VirtIO 1.0 block
device, and performs synchronous bounded sector reads with physical-page queue
storage. The FAT32 reader validates volume geometry and reads root-level 8.3
files from the live disk without keeping the image in UEFI or kernel heap
memory.

The I/O manager owns a private `ReadOnlyImage` descriptor for `CHILD.ELF` and
exposes the sole public `ReadOnlyFile` object. It is not in the object-manager
name directory, so EL0 cannot enumerate or open it by name. A full-image read
creates a synchronous kernel-only `IRP_MJ_READ`, dispatches it through the
fixed FAT image driver/device path, and accepts it only when the completion
status and byte count agree. Process creation supports only the retained
boot-image source or this fixed live child source; both are parsed and mapped
through the same ELF-validation path.

## Test Platform And Smoke Testing

The development test platform is QEMU `aarch64` `virt` with OVMF, using the
`cortex-a57` TCG CPU model on macOS. `make smoke` builds the boot image and
starts headless QEMU; its marker contract is the regression check for the MMU,
EL0, scheduler, user-memory, process/thread, fixed-image I/O, terminal startup,
and VirtIO keyboard input: a private QMP monitor sends `help`, an edited
`echo` command, a Shift/Caps Lock mixed-case payload, and Ctrl-L with an
unfinished line, then runs `clear` and a follow-up `echo`. The serial log must
contain the responses and redraw. Smoke also sends `echo`, `info`, `uptime`,
and `mem` to PL011 and checks their responses. This is not hardware
certification or a Windows-compatibility claim. See
[verified-features.md](verified-features.md) for the complete evidence scope.
