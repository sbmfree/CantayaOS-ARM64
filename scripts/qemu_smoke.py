#!/usr/bin/env python3
"""Boot CantayaOS headlessly and verify the runtime marker contract."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

TERMINAL_PROMPT = "CantayaOS terminal. Type 'help' for commands."
CONSOLE_INPUT_READY = "[user-init] EL0 console input ready"
CONSOLE_INPUT_ISOLATED = "[user-init] EL0 console input isolation validated"
CONSOLE_KEYBOARD_READY = "[user-init] EL0 console keyboard ready"
CONSOLE_INPUT_VALIDATED = "[user-init] EL0 console input validated"
USER_SHELL_READY = "[user-shell] EL0 command loop ready"
KEYBOARD_HELP_RESPONSE = "help          Show commands"
KEYBOARD_EDIT_RESPONSE = "\nedited\ncantaya> "
SERIAL_ECHO_RESPONSE = "\nserialprobe\ncantaya> "
NORMAL_BOOT_SERIAL_RESPONSE = "\nnormalboot\ncantaya> "
KEYBOARD_MODIFIER_RESPONSE = "\nAbCd\ncantaya> "
CTRL_L_REDRAW = "\x1b[2J\x1b[Hcantaya> echo saved"
CTRL_L_RESPONSE = "\nsaved\ncantaya> "
CLEAR_COMMAND_RESPONSE = "clear\n\x1b[2J\x1b[Hcantaya> "
CLEAR_FOLLOWUP_RESPONSE = "\nclearok\ncantaya> "
INFO_RESPONSE = "CantayaOS v0.1.0 (AArch64, QEMU virt)\ncantaya> "
UPTIME_RESPONSE = re.compile(r"\nUptime: \d+\.\d{2} seconds\ncantaya> ")
MEM_RESPONSE = re.compile(r"\nFree physical memory: \d+ MiB \(\d+ pages\)\ncantaya> ")
CRLF_RESPONSE = "\ncrlfprobe\ncantaya> "
CRLF_EXTRA_PROMPT = CRLF_RESPONSE + "\ncantaya> "
UNKNOWN_RESPONSE = "Unknown command: boguscmd\ncantaya> "
RECOVERY_RESPONSE = "\nrecovered\ncantaya> "
LINE_CAPACITY = 128
OVERFLOW_TEXT = b"boguscmd"
BOUNDED_LINE = b"echo " + b"x" * (LINE_CAPACITY - len(b"echo "))
OVERFLOW_INPUT = BOUNDED_LINE + OVERFLOW_TEXT + b"\r"
OVERFLOW_RESPONSE = (
    "\x07" * len(OVERFLOW_TEXT)
    + "\n"
    + "x" * (LINE_CAPACITY - len(b"echo "))
    + "\ncantaya> "
)
OVERFLOW_FOLLOWUP_RESPONSE = "\nboundok\ncantaya> "
QUESTION_HELP_RESPONSE = "?\nhelp          Show commands"
BARE_ECHO_RESPONSE = "echo\n\ncantaya> "
SERIAL_STEPS = (
    (CONSOLE_INPUT_READY, b"@"),
    (KEYBOARD_EDIT_RESPONSE, b"echo serialprobe\r"),
    (CLEAR_FOLLOWUP_RESPONSE, b"info\r"),
    (INFO_RESPONSE, b"uptime\r"),
    (UPTIME_RESPONSE, b"mem\r"),
    (MEM_RESPONSE, b"echo crlfprobe\r\n"),
    (CRLF_RESPONSE, b"boguscmd\r"),
    (UNKNOWN_RESPONSE, b"echo recovered\r"),
    (RECOVERY_RESPONSE, OVERFLOW_INPUT),
    (OVERFLOW_RESPONSE, b"echo boundok\r"),
    (OVERFLOW_FOLLOWUP_RESPONSE, b"?\r"),
    (QUESTION_HELP_RESPONSE, b"echo\r"),
)
KEYBOARD_STEPS = (
    (CONSOLE_KEYBOARD_READY, ("k",)),
    (USER_SHELL_READY, ("h", "e", "l", "p", "ret")),
    (
        KEYBOARD_HELP_RESPONSE,
        (
            "j", "u", "n", "k", "ctrl-u",
            "e", "c", "h", "o", "spc", "e", "d", "i", "t", "e", "x",
            "backspace", "d", "ret",
        ),
    ),
    (
        SERIAL_ECHO_RESPONSE,
        (
            "e", "c", "h", "o", "spc", "shift-a", "b",
            "caps_lock", "c", "caps_lock", "d", "ret",
        ),
    ),
    (
        KEYBOARD_MODIFIER_RESPONSE,
        (
            "e", "c", "h", "o", "spc", "s", "a", "v", "e", "d",
            "ctrl-l", "ret",
        ),
    ),
    (CTRL_L_RESPONSE, ("c", "l", "e", "a", "r", "ret")),
    (
        CLEAR_COMMAND_RESPONSE,
        ("e", "c", "h", "o", "spc", "c", "l", "e", "a", "r", "o", "k", "ret"),
    ),
)


# Presence-only markers. Markers with minimum counts are checked below.
REQUIRED_MARKERS = (
    "MMU enabled:",
    "Kernel executing through TTBR1:",
    "Mm high-half map/unmap probe passed",
    "Mm user address-space probe passed",
    "Mm user stack guard probe passed",
    "AArch64 fault decoder probe passed",
    "Ps: init ELF mapped",
    "VirtIO block: live read-only boot disk ready",
    "VirtIO keyboard: MMIO input ready",
    "Ps: external current-thread termination rejected",
    "Ps: external queued-thread termination validated",
    "Ps: external completed-thread termination validated",
    "Ps: typed handle access rights validated",
    "Ps: stale typed handle reuse rejected",
    "Ps: exhausted typed handle slot skipped",
    "Ps: process-local numeric handle collision validated",
    "Ps: malformed typed handle values rejected",
    "EL0 timer preemption captured",
    "[user-init] EL0 context resume validated",
    "NtWriteFile copied",
    "NtAllocateVirtual mapped",
    "NtFreeVirtual released",
    "NtQuerySystemInfo copied validated EL0 output",
    "Ps: typed process and thread handle waits validated",
    "Ps: process pid=",
    TERMINAL_PROMPT,
    CONSOLE_INPUT_READY,
    CONSOLE_INPUT_ISOLATED,
    CONSOLE_KEYBOARD_READY,
    "[user-init] EL0 console wait cancellation armed",
    "[user-init] EL0 console wait cancellation validated",
    "Ps: external process termination cleared 1 console input wait registration(s)",
    "Console: PL011 and VirtIO input IRQs validated",
    CONSOLE_INPUT_VALIDATED,
    USER_SHELL_READY,
    KEYBOARD_HELP_RESPONSE,
    KEYBOARD_EDIT_RESPONSE,
    SERIAL_ECHO_RESPONSE,
    KEYBOARD_MODIFIER_RESPONSE,
    CTRL_L_REDRAW,
    CTRL_L_RESPONSE,
    CLEAR_COMMAND_RESPONSE,
    CLEAR_FOLLOWUP_RESPONSE,
    INFO_RESPONSE,
    CRLF_RESPONSE,
    UNKNOWN_RESPONSE,
    RECOVERY_RESPONSE,
    OVERFLOW_RESPONSE,
    OVERFLOW_FOLLOWUP_RESPONSE,
    QUESTION_HELP_RESPONSE,
    BARE_ECHO_RESPONSE,
)
REQUIRED_PATTERNS = (
    ("uptime command response", UPTIME_RESPONSE),
    ("memory command response", MEM_RESPONSE),
)
REQUIRED_MARKER_COUNTS = {
    "Ps: reaped thread": 5,
    "[user-init] EL0 fixed VM reuse validated": 2,
    "[user-init] EL0 console output contract validated": 2,
    "[user-init] EL0 thread handle wait validated": 2,
    "[user-init] external thread handle termination validated": 2,
    "[user-init] blocked typed wait thread termination validated": 2,
    "[user-init] finite blocked typed wait thread termination validated": 2,
    "[user-init] blocked process wait thread termination validated": 2,
    "[user-init] finite blocked process wait thread termination validated": 2,
    # Two complete rounds in each of the two initial EL0 processes.
    "[user-init] mixed finite lifecycle cancellation validated": 4,
    "[user-init] stale typed handles rejected after reuse": 2,
    "[user-init] cross-type stale handles rejected after reuse": 2,
    "[user-init] multi-generation stale handles rejected after churn": 2,
    "[user-init] isolated child rejected parent handle": 4,
    "[user-init] process-local parent handles validated": 2,
    "[user-init] colliding child thread handle completed": 2,
    "[user-init] colliding parent thread handle remained live": 2,
    "[user-init] EL0 numeric handle collision validated": 2,
    "[user-init] create output failures left no handles": 2,
    "[user-init] thread entry and stack preflight validated": 2,
    "[user-init] typed wait argument preflight validated": 2,
    "[user-init] cross-page typed wait preflight validated": 2,
    "[user-init] finite typed wait timeout validated": 2,
    "[user-init] process-wide blocked wait termination validated": 2,
    "Ps: current-process termination cleared 1 typed wait registration(s)": 2,
    "[user-init] external process blocked wait termination validated": 2,
    "Ps: external thread termination cleared typed wait registration": 16,
    "Ps: external thread termination cleared typed process wait registration": 8,
    "Ps: external thread termination cleared finite typed wait": 12,
    "Ps: external process termination cleared ": 2,
    "NtCreateThread created a validated EL0 thread": 2,
    "NtCreateProcess created pid=": 2,
    "Io: read-only file CHILD.ELF dispatching image IRP": 2,
    "Io: IRP read-only image CHILD.ELF completed": 2,
    "Ps: FAT CHILD.ELF mapped": 2,
    "source=FAT CHILD.ELF": 2,
    "[user-child] FAT image executed": 2,
    "[user-init] EL0 process handle wait validated": 2,
    "[user-init] FAT child status wait validated": 2,
    "NtWaitForSingleObject observed exit status=": 2,
}
FAILURE_MARKERS = (
    "KERNEL PANIC",
    "EL1 instruction abort",
    "EL1 data abort",
    "[user-init] ERROR failed creation started target",
    "[user-init] ERROR console input probe failed",
    "Ps: EL0 shell exited",
    "Ps: EL0 shell could not start",
    "[System] heartbeat",
    "[Thread-A] alive",
    "[Thread-B] alive",
    CRLF_EXTRA_PROMPT,
)


def qmp_execute(stream, command: dict[str, object]) -> None:
    stream.write(json.dumps(command).encode() + b"\n")
    stream.flush()
    while True:
        line = stream.readline()
        if not line:
            raise RuntimeError("QMP connection closed before a reply")
        response = json.loads(line)
        if "error" in response:
            raise RuntimeError(f"QMP rejected {command['execute']}: {response['error']}")
        if "return" in response:
            return


def connect_keyboard_monitor(monitor_path: Path):
    monitor = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    monitor.settimeout(2)
    monitor.connect(str(monitor_path))
    stream = monitor.makefile("rwb")
    line = stream.readline()
    if not line:
        raise RuntimeError("QMP connection closed before its greeting")
    greeting = json.loads(line)
    if "QMP" not in greeting:
        raise RuntimeError("QMP greeting was missing")
    qmp_execute(stream, {"execute": "qmp_capabilities"})
    return monitor, stream


def send_keyboard_keys(stream, keys: tuple[str, ...], serial_log: Path, deadline: float) -> None:
    for key in keys:
        previous_size = serial_log.stat().st_size
        sequence = key.split("-")
        for down, codes in ((True, sequence), (False, reversed(sequence))):
            qmp_execute(
                stream,
                {
                    "execute": "input-send-event",
                    "arguments": {
                        "events": [
                            {
                                "type": "key",
                                "data": {
                                    "down": down,
                                    "key": {"type": "qcode", "data": code},
                                },
                            }
                            for code in codes
                        ]
                    },
                },
            )
        # Explicit key-up avoids a QEMU virtual-time hold crossing the next
        # press when TCG runs much slower than host time.
        # Wait for the guest's echo so the next key cannot outrun descriptor
        # recycling on a slow emulated CPU. Caps Lock has no direct echo.
        if key != "caps_lock":
            while serial_log.stat().st_size <= previous_size:
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"VirtIO keyboard stopped responding after {key}")
                time.sleep(0.01)
        time.sleep(0.1)


def response_seen(trigger: str | re.Pattern[str], output: str) -> bool:
    return trigger in output if isinstance(trigger, str) else trigger.search(output) is not None


def capture_serial(connection: socket.socket, serial_log: Path, stop: threading.Event) -> None:
    connection.settimeout(0.2)
    with serial_log.open("wb") as output:
        while not stop.is_set():
            try:
                chunk = connection.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                break
            if not chunk:
                break
            output.write(chunk)
            output.flush()


def connect_serial(serial_path: Path, process: subprocess.Popen, deadline: float) -> socket.socket:
    while time.monotonic() < deadline:
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            connection.connect(str(serial_path))
            return connection
        except OSError:
            connection.close()
            if process.poll() is not None:
                raise RuntimeError("QEMU exited before opening its serial socket")
            time.sleep(0.01)
    raise RuntimeError("QEMU serial socket did not become ready")


def send_serial_input(connection: socket.socket, payload: bytes) -> None:
    if payload == OVERFLOW_INPUT:
        # A sleeping EL0 reader drains the small PL011 FIFO at scheduler-tick
        # cadence. Pace each byte so this remains a line-limit test.
        for byte in payload:
            connection.sendall(bytes((byte,)))
            time.sleep(0.08)
    else:
        connection.sendall(payload)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--ovmf", type=Path, required=True)
    parser.add_argument("--ovmf-vars", type=Path, required=True)
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--normal-boot", action="store_true")
    args = parser.parse_args()

    required_markers = (
        (USER_SHELL_READY, TERMINAL_PROMPT, KEYBOARD_HELP_RESPONSE, NORMAL_BOOT_SERIAL_RESPONSE)
        if args.normal_boot else REQUIRED_MARKERS
    )
    keyboard_steps = (
        ((USER_SHELL_READY, ("h", "e", "l", "p", "ret")),)
        if args.normal_boot else KEYBOARD_STEPS
    )
    serial_steps = (
        ((KEYBOARD_HELP_RESPONSE, b"echo normalboot\r"),)
        if args.normal_boot else SERIAL_STEPS
    )

    with tempfile.TemporaryDirectory(prefix="cantaya-smoke-") as directory:
        serial_log = Path(directory) / "serial.log"
        serial_path = Path(directory) / "serial.sock"
        monitor_path = Path(directory) / "qmp.sock"
        # A visible `make run` may have these writable images open already.
        # Give this headless guest private copies so the smoke test can boot.
        vars_copy = Path(directory) / "ovmf-vars.fd"
        image_copy = Path(directory) / "cantaya.img"
        shutil.copyfile(args.ovmf_vars, vars_copy)
        shutil.copyfile(args.image, image_copy)
        if not args.normal_boot:
            smoke_flag = Path(directory) / "SMOKE.FLG"
            smoke_flag.write_bytes(b"console-input-probe\n")
            subprocess.run(
                ["mcopy", "-i", str(image_copy), str(smoke_flag),
                 "::/EFI/CantayaOS/SMOKE.FLG"],
                check=True,
                capture_output=True,
            )
        command = [
            args.qemu,
            "-machine", "virt,highmem=on",
            "-cpu", "cortex-a57",
            "-m", "512M",
            "-device", "ramfb",
            "-device", "virtio-keyboard-device",
            "-nic", "none",
            "-drive", f"if=pflash,format=raw,file={args.ovmf},readonly=on",
            "-drive", f"if=pflash,format=raw,file={vars_copy}",
            "-drive", f"if=none,format=raw,file={image_copy},id=cantaya-disk",
            "-global", "virtio-mmio.force-legacy=false",
            "-device", "virtio-blk-device,drive=cantaya-disk",
            "-chardev", f"socket,path={serial_path},id=serial0,server=on,wait=on",
            "-serial", "chardev:serial0",
            "-qmp", f"unix:{monitor_path},server=on,wait=off",
            "-display", "none",
            "-no-reboot",
        ]
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
        serial_connection = None
        serial_stop = threading.Event()
        serial_reader = None
        output = ""
        keyboard_step = 0
        serial_step = 0
        input_error = None
        keyboard_monitor = None
        keyboard_stream = None
        deadline = time.monotonic() + args.timeout

        try:
            serial_connection = connect_serial(serial_path, process, deadline)
            serial_reader = threading.Thread(
                target=capture_serial,
                args=(serial_connection, serial_log, serial_stop),
                daemon=True,
            )
            serial_reader.start()
            while time.monotonic() < deadline:
                if serial_log.exists():
                    output = serial_log.read_text(errors="replace")
                    if any(marker in output for marker in FAILURE_MARKERS):
                        break
                    if output.count("Unknown command:") > 1:
                        break
                    if keyboard_step < len(keyboard_steps) and (
                        keyboard_steps[keyboard_step][0] in output
                    ):
                        try:
                            if keyboard_stream is None:
                                keyboard_monitor, keyboard_stream = connect_keyboard_monitor(
                                    monitor_path
                                )
                            send_keyboard_keys(
                                keyboard_stream,
                                keyboard_steps[keyboard_step][1],
                                serial_log,
                                deadline,
                            )
                            keyboard_step += 1
                        except (OSError, ValueError, RuntimeError) as error:
                            input_error = str(error)
                            break
                    if serial_step < len(serial_steps) and response_seen(
                        serial_steps[serial_step][0], output
                    ):
                        try:
                            send_serial_input(serial_connection, serial_steps[serial_step][1])
                            serial_step += 1
                        except (OSError, RuntimeError) as error:
                            input_error = str(error)
                            break
                    full_contract_passed = args.normal_boot or (
                        all(output.count(marker) >= count
                            for marker, count in REQUIRED_MARKER_COUNTS.items())
                        and all(pattern.search(output) for _, pattern in REQUIRED_PATTERNS)
                        and output.count("Unknown command:") == 1
                    )
                    if all(marker in output for marker in required_markers) and full_contract_passed:
                        break
                if process.poll() is not None:
                    break
                time.sleep(0.1)
        finally:
            if keyboard_stream is not None:
                keyboard_stream.close()
            if keyboard_monitor is not None:
                keyboard_monitor.close()
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            serial_stop.set()
            if serial_connection is not None:
                serial_connection.close()
            if serial_reader is not None:
                serial_reader.join(timeout=2)

        if serial_log.exists():
            output = serial_log.read_text(errors="replace")

    failures = [marker for marker in FAILURE_MARKERS if marker in output]
    missing = [marker for marker in required_markers if marker not in output]
    if args.normal_boot:
        if CONSOLE_INPUT_READY in output:
            failures.append("console input probe started during normal boot")
    else:
        missing.extend(
            f"{marker} (expected at least {count})"
            for marker, count in REQUIRED_MARKER_COUNTS.items()
            if output.count(marker) < count
        )
        missing.extend(
            description
            for description, pattern in REQUIRED_PATTERNS
            if not pattern.search(output)
        )
        if output.count("Unknown command:") != 1:
            missing.append("exactly one intentional unknown-command response")
        if output.count("\x07") != len(OVERFLOW_TEXT):
            missing.append(f"exactly {len(OVERFLOW_TEXT)} overflow bells")
    if failures or missing or input_error:
        print("CantayaOS QEMU smoke test failed.", file=sys.stderr)
        if input_error:
            print(f"Input injection failed: {input_error}", file=sys.stderr)
        if failures:
            print(f"Failure markers: {', '.join(failures)}", file=sys.stderr)
        if missing:
            print(f"Missing markers: {', '.join(missing)}", file=sys.stderr)
        print(output[-4000:], file=sys.stderr)
        return 1

    label = "normal boot" if args.normal_boot else "QEMU smoke test"
    print(f"CantayaOS {label} passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
