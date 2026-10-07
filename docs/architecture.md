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
  -> CantayaOS banner and EL0 shell after both init processes exit
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
- `drivers`: VirtIO-MMIO block, keyboard and absolute tablet devices, plus FAT32.
- `shell`: the bounded EL1 fallback prompt.
- `Ke`: spinlocks, mutexes, DPCs, and waiting primitives.
- `Mm`: physical pages, heap, kernel virtual mappings, and user address-space
  ownership.
- `Ob`: typed objects and handle tables.
- `Ps`: `EPROCESS`, `ETHREAD`, executable loading, and timer-driven round-robin
  scheduling.
- `Io`: IRPs, fixed image dispatch, and bounded read-only FAT file access.
- `Se`: security-reference-monitor scaffolding.

The layering is intentional: fixed child loading consumes the I/O-owned file
object rather than FAT or VirtIO internals. Named programs and user file reads
also enter through `Io` and its bounded FAT path.

## Desktop And Terminal

The two initial EL0 `init.elf` processes run the boot validation workload.
The System thread waits for both to exit, then starts a controlled copy in
shell mode. On the supported framebuffer that process starts its own desktop:
a software-composed background, taskbar, cursor, terminal, and read-only file
browser. Independent EL0 apps submit bounded copied surfaces; the desktop
composes them and routes focus, keys and content-local pointer input. Application
waits block in the scheduler and are cancelled before thread reaping. App exit
cleans up its surfaces; desktop exit invalidates every window. The kernel retains
exclusive framebuffer ownership and accepts copied, bounded rectangle updates;
no display physical address is exposed to a user process.

Window resizing is a desktop-only copied-surface transaction: allocate first,
copy intersecting rows, zero growth, then publish dimensions and a new revision.
Reset/resize events cancel old gestures and wake app waiters. Owner queries and
revision-guarded submissions let an app recompose safely after a racing resize.
Minimizing lives in the EL0 compositor and preserves the kernel surface and ID.
Terminal reflows visible text; Paint stores its full drawing separately from
its current presentation buffer. Desktop state uses allocated user memory to
leave room for startup and reflow on the guarded 16 KiB user stack.

Keyboard and absolute pointer devices share a VirtIO-MMIO queue transport,
selecting devices by their advertised event capabilities. The keyboard decodes
US ASCII with Shift, Caps Lock, Backspace, Enter, Ctrl-U and Ctrl-L. During a
desktop session it publishes structured key press/release/repeat events with
optional text. The tablet publishes complete absolute position and button state
at SYN_REPORT. Input IRQs wake the desktop through the existing console wait.
The session is tied to the process that owns the exclusive `-2` input claim.

Terminal commands remain `help`, `info`, `uptime`, `mem`, `echo`, `clear`, `ls`,
`cat`, and `run`. Accepted console output reaches PL011 and a bounded desktop
output mirror. Kernel diagnostic logs remain on UART while graphics owns the
screen. Desktop startup diagnostics are discarded from the visible transcript.
The user process consumes the mirror and renders the terminal with the shared
bitmap font. Child program output follows the same path. Console clear resets
only the terminal pane during a graphical session.

Without a supported display, the EL0 text shell remains available. If the
user process cannot start or exits, the System thread restores the EL1 prompt,
resets graphical ownership and its bounded queues, and redraws the text screen.
See [desktop.md](desktop.md) for the ABI, controls, limits and visual checks.

EL0 diagnostic output uses only `NtWriteFile(-1, text, length)`, where `-1` is a
fixed console-output pseudo-handle rather than a closable file-table entry.
The syscall validates a bounded user range and supported ASCII text before a
shared console path writes to both UART and the framebuffer console or desktop output mirror.
`NtReadFile(-2, buffer, capacity, count)` provides a separate, fixed input
pseudo-handle.
It validates both writable EL0 ranges before claiming PL011 and keyboard input
for one process; an empty read returns `STATUS_TIMEOUT` without writing output.
Other processes are denied until the owner calls `NtClose(-2)` or exits. The
EL1 fallback parser does not consume input while a live EL0 owner holds that
claim.
`NtWaitForConsoleInput(-2)` blocks a thread on the live owner's claim without
consuming a byte. PL011 RX and VirtIO keyboard IRQs queue input and wake one
registered reader when data is ready. The System thread no longer polls
readiness in its hot loops. A close advances the claim generation and wakes
old waiters with `STATUS_INVALID_HANDLE` and releases its desktop session; external
thread or process termination cancels the registration before reaping. The EL0
shell uses this wait after an empty nonblocking `NtReadFile`, while the EL1
fallback remains available if the shell exits.
Normal boots give the EL0 shell input ownership; one private smoke ESP
requests a controlled input probe through a `BootInfo` flag before starting
that shell. The EL0 `clear` command uses a narrow owner-only redraw syscall
rather than broadening `NtWriteFile` to accept escape sequences. System-info
classes 0 and 1 provide free pages and elapsed 100 Hz ticks for `mem` and
`uptime`.
A separate private fallback boot requests a shell mode that claims input and
then exits, allowing smoke to verify that EL1 reclaims both input devices.

For read-only files, `NtCreateFile` accepts a writable handle output and a
bounded path pointer/length. `NtReadFile` accepts the typed file handle, a
writable buffer, its capacity, and a writable byte count; a zero count marks
end-of-file. `NtQueryRootDirectory` and `NtQueryDirectory` return a 16-byte
entry by ordinal for shell `ls`. `NtCreateProcess` selector two accepts a
bounded path in `x3`/`x4` and optional argument bytes in `x2`/`x5`; it passes
a child-owned, zero-terminated copy to the new program in `x1`.

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
typed process/thread completions or console readiness; it is distinct from the ready run queue
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
pointer may reference a relative `u64` count of zero through 1,000 100 Hz
scheduler ticks. Zero polls completion without registering a waiter. Positive
timeouts record an absolute deadline for the sleeping
thread in an ordered internal queue; the timer IRQ readies only expired entries.
Before returning `STATUS_TIMEOUT`, the thread removes itself from the typed
process or thread completion queue, so later completion cannot wake a stale
waiter. A signaled finite wait retains the same final-status output behavior as
an infinite wait, while a timeout leaves that output untouched.

Exception frames preserve all FP/SIMD registers plus FPCR/FPSR. Kernel context
switches preserve q8–q15 and FP controls, and first EL0 entry clears all vectors
and FP state. Integer Rust copies can use SIMD, so this is part of user-state
isolation. An assembly probe holds all lanes live across syscalls and preemption.

## Syscalls And User-Memory Validation

EL0 enters the syscall layer through `SVC #0`: `x8` selects a syscall and
`x0` through `x5` carry arguments. The exception handler supplies a saved
register frame and the dispatcher writes the result back to saved `x0`.

User pointers are never trusted directly. User-memory copy-in and copy-out
validate the complete required range and access mode before the kernel copies.
Console writes additionally reject non-console handles, unsupported bytes,
and lengths outside 1–1,024 bytes before producing output.

The same ownership model supports process-local, zeroed virtual-memory regions,
guarded stacks, W^X ELF mapping, and instruction-cache synchronization after
mapping executable pages. Detailed current syscall limits are in
[STATUS.md](../STATUS.md).

## Read-Only VirtIO/FAT Paths

The synchronous block reader uses a 250 ms ARM counter deadline per wait.
A timeout leaves its DMA request pending; the next read drains that completion
before changing descriptors or the shared buffer. Invalid completion identity
stops further queue reuse. Output bytes are copied only after a successful
completion. Ordinary desktop smoke forces a timeout with QMP disk throttling
and verifies correct file contents after removing the throttle.

QEMU attaches the boot image as an explicit modern VirtIO-MMIO block device.
The driver scans the standard MMIO slots, accepts only a VirtIO 1.0 block
device, and performs synchronous bounded sector reads with physical-page queue
storage. The FAT32 reader validates volume geometry and reads bounded 8.3
files from the root or one subdirectory without retaining the disk image in
UEFI or kernel heap memory.

A second VirtIO block device is identified by the `CANTDATA` FAT32 volume label.
It is independent of the read-only boot ESP and serves only the private,
bounded one-cluster FAT32 create protocol. The protocol mirrors a transaction
record in reserved sectors, flushes every durable phase, and performs bounded
recovery before accepting another internal create. It has no public syscall or
shell interface.

The I/O manager owns a private `ReadOnlyImage` descriptor for `CHILD.ELF` and
exposes a fixed kernel-owned `ReadOnlyFile` object. It is not in the object-manager
name directory. A full-image read
creates a synchronous kernel-only `IRP_MJ_READ`, dispatches it through the
fixed FAT image driver/device path, and accepts it only when the completion
status and byte count agree. The named-file source uses the separate bounded
read-only FAT path in `Io`, then the same ELF-validation and mapping path as
the retained boot image and fixed child image. User file handles are typed,
process-local, read-only, and own a current offset; `NtReadFile` returns a zero
count at EOF. A directory query returns bounded 8.3 entries. Named processes
may receive up to 64 printable argument bytes copied into child-owned memory.

## Test Platform And Smoke Testing

The development test platform is QEMU `aarch64` `virt` with OVMF, using the
`cortex-a57` TCG CPU model on macOS. `make smoke` builds the boot image and
starts headless QEMU; its marker contract is the regression check for the MMU,
EL0, scheduler, user-memory, process/thread, read-only I/O, terminal startup,
and VirtIO keyboard input: a private QMP monitor sends `help`, an edited
`echo` command, a Shift/Caps Lock mixed-case payload, and Ctrl-L with an
unfinished line, then runs `clear` and a follow-up `echo`. The serial log must
contain the responses and redraw. Smoke also sends `echo`, `info`, `uptime`,
and `mem` to PL011, then checks `ls`, `cat`, and `run` on root and subdirectory
paths. It rejects a duplicate prompt after CRLF and verifies unknown-command
recovery. A separate private boot checks EL1 fallback after EL0 exits. This is
not hardware certification or a Windows-compatibility claim. See
[verified-features.md](verified-features.md) for the complete evidence scope.
