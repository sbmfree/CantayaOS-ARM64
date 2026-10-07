# CantayaOS Desktop

The first desktop runs inside the initial EL0 shell process. It draws a
background, taskbar and software cursor, then composes Terminal and Files
windows and independent application surfaces into a private pixel buffer. A second private buffer tracks presented
pixels so only changed four-row bands cross the syscall boundary. At 1024x768
the two screen buffers occupy 6 MiB. Application caches and one scratch surface
add about 1.5 MiB. Each app owns its own address space and drawing buffer. The kernel keeps its hardware framebuffer
private and converts canonical RGB pixels to GOP RGB/BGR layout.

## Run And Controls

`make run` boots the desktop after the existing validation programs complete.
The supported graphical range is 800x600 through 1024x768 with 32-bit RGB/BGR
pixels; unsupported modes retain the EL0 text shell. Serial input remains
available alongside QEMU's VirtIO keyboard and absolute tablet pointer.

- Click a window or taskbar button to focus it. F1 selects Terminal, F2 selects
  Files, and F3 opens or focuses Paint. Tab cycles through built-in and app windows.
- Drag a window's title bar to move it, or its bottom-right grip to resize it.
  Terminal reflows its visible text and preserves an unfinished command. Files
  adjusts its list and preview to the available space.
- Click `_` to minimize a window. Its process and drawing stay alive; restore it
  with the taskbar, F1/F2/F3, or Tab.
- Close Terminal or Files with `x` and
  reopen from the taskbar. Closing an app destroys its surface; Paint exits and
  the Paint taskbar button starts a fresh instance.
- In Files, click a folder, text file, or `.ELF` app, or use Up/Down, Home and Enter.
  Back or Esc returns to the boot disk root.
- Terminal retains all existing commands and editing shortcuts. Serial input
  always goes to Terminal, including when Files has keyboard focus.

Files uses the existing read-only FAT calls and the root/one-subdirectory 8.3
path limit. Its preview reads at most 1 KiB and displays four wrapped lines.
`cat PATH` remains available for complete text. Non-text files are identified
in the preview rather than interpreted as text. `.ELF` selection launches the
program asynchronously. Terminal `run` also runs asynchronously in graphical
mode, retains up to four completion handles, and reports exits without blocking
the desktop. The text-only shell retains its synchronous wait.
The taskbar uptime updates on interaction rather than waking an idle desktop.

Paint is installed as `BIN/PAINT.ELF`. Click or drag on the canvas to draw, use
its palette or keys 1–4 to choose a color, click CLR or press C to clear, and
press Esc to quit. Drawing remains in memory across focus changes, minimizing
and resizing. A smaller canvas hides part of the drawing; growing it reveals those strokes
again. Clear erases the full drawing, including hidden areas. Restarting Paint
creates a blank canvas. Saving is not available on the read-only disk.

## Version-One ABI

The structures and numbers are in `shared/src/desktop.rs`. The SVC convention
is unchanged: number in x8, arguments in x0..x5, NTSTATUS in x0.

| Service | Number | Arguments | Result |
| --- | --- | --- | --- |
| Query display | `0x40` | x0: writable `DisplayInfo` | Width, height, version 1, maximum blit pixels; no hardware address |
| Present rectangle | `0x41` | x0: pixels, x1/x2: destination x/y, x3/x4: width/height, x5: source stride in bytes | Copied `0x00RRGGBB` pixels |
| Read event | `0x42` | x0: writable `DesktopEvent` | One event; empty read returns `STATUS_TIMEOUT` without changing output |
| Read console mirror | `0x43` | x0: writable buffer, x1: capacity 1..1024, x2: writable u64 count | Console bytes; empty read returns `STATUS_TIMEOUT` without changing outputs |

Presentation, events and output require the current console-input owner. The
first valid presentation attaches its session. The query is read-only and
available without ownership. Presentation rejects zero or off-screen
rectangles, more than 4,096 pixels, invalid/unaligned stride, address overflow,
and inaccessible source rows. Stride is bounded to 4,096 bytes. The kernel
copies all rows into a 16 KiB staging buffer before updating the screen, so a
fault on a later row cannot partially paint the display. Each bounded copy and
blit masks local IRQs to protect the single-core state and address-space locks.

Events contain a kind, evdev code or button bitmap, signed value and text:

- Key events preserve press/release/repeat and optional decoded ASCII.
- Pointer events carry complete button state and absolute x/y in 0..32767,
  published at SYN_REPORT. EL0 scales and clamps them to the screen.
- Serial events carry a text byte.
- Queue overflow discards stale events and delivers a reset before new input.
  The event queue holds 128 records. The console mirror holds 8 KiB and retains
  the newest bytes on overflow. Neither queue grows without bound.

`NtWaitForConsoleInput(-2)` now also observes desktop events and mirrored
output. Kernel diagnostic logs continue on UART and do not paint over the
desktop. `NtClearConsole` clears its terminal pane while keeping the windows.
Closing `-2` releases the session and clears its queues atomically with input
ownership. A new owner also resets an exited owner's graphical state. On
user-process exit,
the System thread starts the existing EL1 fallback, resets graphics state,
and redraws the text console before accepting input.

## Application Windows And Copied IPC

`WindowInfo` is 64 bytes: ID, surface revision, change epoch, content width and
height, and a zero-padded 32-byte ASCII title. At most four application windows
exist, each between 32x32 and 320x240. IDs increase monotonically, are never
reused across sessions, and are separate from the typed process/file handles.
The kernel checks process ownership on every application operation. Titles
contain 1–31 printable ASCII bytes. Pixels use the display ABI's packed RGB.

| Service | Number | Arguments | Access |
| --- | --- | --- | --- |
| Create window | `0x44` | x0: writable u64 ID, x1/x2: title pointer/length, x3/x4: content width/height | Any process during an active desktop session |
| Submit pixels | `0x45` | x0: ID, x1: source, x2: linear pixel offset, x3: count 1..4096, x4: expected revision (zero disables the guard) | Window owner; stale revision = `STATUS_RETRY` |
| Read window event | `0x46` | x0: ID, x1: writable `DesktopEvent` | Window owner; empty = unchanged output and `STATUS_TIMEOUT` |
| Wait for event | `0x47` | x0: ID | Window owner; blocks until input or invalidation |
| Close window | `0x48` | x0: ID | Window owner or desktop |
| Enumerate slot | `0x49` | x0: slot 0..3, x1: writable `WindowInfo` | Desktop; empty slot has ID zero |
| Copy surface | `0x4a` | x0: ID, x1: output, x2: pixel offset, x3: count 1..4096, x4: expected revision | Desktop; changed revision = `STATUS_RETRY` with unchanged output |
| Route event | `0x4b` | x0: ID, x1: event pointer | Desktop |
| Acknowledge changes | `0x50` | x0: epoch observed before scanning slots | Desktop |
| Resize window | `0x51` | x0: ID, x1/x2: content width/height | Desktop; 32x32 through 320x240 |
| Query own window | `0x52` | x0: ID, x1: writable `WindowInfo` | Window owner; does not consume events |

Every submission copies its complete source into staging before changing the
surface, including when a source crosses into an unmapped page. The desktop
fetches tiles against a revision into a scratch surface and publishes its cache
only after every tile succeeds. A racing update or close causes a retry without
exposing a partial cache. Frame submission is atomic per copied tile; apps can
submit multiple tiles to update their full surface.

Resize allocates a replacement surface before committing any change, copies
intersecting rows and zeroes newly exposed pixels. Invalid sizes and allocation
failure leave dimensions, pixels and events unchanged; an identical size is a
no-op. A committed resize advances the revision, discards stale input and queues
RESET followed by RESIZE (kind 6, width in value, height in text), waking blocked
app readers. Apps query current geometry before drawing and submit with a
revision guard to reject stale strides if another resize races the upload.
Paint keeps a separate 304x162 drawing and recomposes it into the current surface.
The desktop coalesces app resize motion within each input batch. Its resize
minimum is 160x120 client pixels; the kernel also supports smaller surfaces.
Minimizing changes only desktop visibility and focus, so IDs remain valid.

Window creation, pixels, resizing, closure, and process completion advance an epoch.
Acknowledging an earlier epoch leaves later changes pending, so the existing
console wait cannot lose an application update. Apps receive only desktop-routed
keys, bounded content-local pointer coordinates, focus changes and resets,
plus kernel-generated resize events.
Pointer capture continues through a drag and sends a release outside the window;
focus loss cancels the app's held drawing state. Serial input goes to Terminal.
Each application queue holds 32 events; overflow cancels stale state with a
reset. Up to 16 threads may wait on a window. Wait registration and readiness
are coordinated with the scheduler under masked local IRQs; closing the window
or resetting the session wakes readers with `STATUS_INVALID_HANDLE`. Termination
cancels raw registrations before reclaiming thread stacks. Process exit removes
its windows even if the app did not explicitly close them.

A zero-tick `NtWaitForSingleObject` timeout now polls completion without
registering a waiter; a live object leaves the completion output unchanged.
The terminal uses this to release finished launch handles. FP/SIMD state is
preserved in exception frames, kernel context switches preserve the C ABI's
callee-saved vectors, and new EL0 processes start with cleared vectors and FP
control/status. This is required even for integer Rust code that uses vector
instructions for memory copies.

## Validation

`make smoke` still runs three guests. The private guest tests graphics bounds,
invalid pointers, an unmapped later row, unchanged empty-event output,
overlapping output/count rejection, release/reclaim of graphical ownership,
and access denial (including an attempted input close) from an isolated process.
A screenshot checks that the rejected blit did not write its first pixel.
Application probes additionally check resource limits, stale and foreign IDs,
source-copy atomicity, unchanged retry/timeout output, event preservation on an
invalid read, queue overflow, blocked close, external termination and session
reset. Resize probes cover permissions, rejected sizes, row preservation, zeroed
growth, no-op dimensions, stale upload guards and blocked-reader wakeup. Both
initial validation processes hold different patterns in all 32 SIMD registers
and FP control/status across copied syscalls and timer ticks.
The ordinary guest retains shell/file/program checks and adds QMP mouse and
keyboard interaction checks against actual framebuffer pixels and text. It
tests focus, previews, repeated folder navigation, dragging, close/reopen and
terminal output after window interaction. It also throttles its private disk
to force a read timeout, removes the throttle, and verifies the file preview
recovers with correct contents. The block reader uses a 250 ms counter-based
deadline and retains pending DMA descriptors/buffers until late completion,
before issuing a fresh read. Paint checks cover drawing, palette selection,
clearing, focus retention, terminal responsiveness, dragging, desktop- and
app-initiated close, Files/terminal launches, two simultaneous app windows and
asynchronous completion. Resize checks cover minimum/maximum sizes, repeated
shrink/grow, drawing retention and new input coordinates. Terminal tests reflow
an unfinished command and execute it after restoring the size; Files tests keep
selection and previews visible. All three window types are minimized and
restored, and Paint keeps its original process. It then saves
`target/desktop.ppm`. The fallback guest starts graphics before terminating the shell and checks recovered
keyboard and serial commands.

This is bounded software composition on single-core QEMU. Shared-memory
surfaces, GPU drivers, writable files and hardware certification
remain future work. App close invalidates its window; arbitrary apps must handle
that result themselves. Paint exits on invalidation. There is no general IPC
service or process sandbox beyond the existing user-memory and ownership checks.
