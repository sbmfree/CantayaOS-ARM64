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
import time
from pathlib import Path

TERMINAL_PROMPT = "CantayaOS terminal. Type 'help' for commands."
KEYBOARD_HELP_RESPONSE = "help          Show commands"
KEYBOARD_EDIT_RESPONSE = "\nedited\ncantaya> "
SERIAL_ECHO_RESPONSE = "\nserialprobe\ncantaya> "
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
SERIAL_STEPS = (
    (KEYBOARD_EDIT_RESPONSE, b"echo serialprobe\r"),
    (CLEAR_FOLLOWUP_RESPONSE, b"info\r"),
    (INFO_RESPONSE, b"uptime\r"),
    (UPTIME_RESPONSE, b"mem\r"),
    (MEM_RESPONSE, b"echo crlfprobe\r\n"),
    (CRLF_RESPONSE, b"boguscmd\r"),
    (UNKNOWN_RESPONSE, b"echo recovered\r"),
    (RECOVERY_RESPONSE, OVERFLOW_INPUT),
    (OVERFLOW_RESPONSE, b"echo boundok\r"),
)
KEYBOARD_STEPS = (
    (TERMINAL_PROMPT, ("h", "e", "l", "p", "ret")),
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


def send_keyboard_keys(monitor_path: Path, keys: tuple[str, ...]) -> None:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as monitor:
        monitor.settimeout(2)
        monitor.connect(str(monitor_path))
        with monitor.makefile("rwb") as stream:
            line = stream.readline()
            if not line:
                raise RuntimeError("QMP connection closed before its greeting")
            greeting = json.loads(line)
            if "QMP" not in greeting:
                raise RuntimeError("QMP greeting was missing")
            qmp_execute(stream, {"execute": "qmp_capabilities"})
            for key in keys:
                qmp_execute(
                    stream,
                    {
                        "execute": "human-monitor-command",
                        "arguments": {"command-line": f"sendkey {key} 20"},
                    },
                )
                time.sleep(0.05)


def response_seen(trigger: str | re.Pattern[str], output: str) -> bool:
    return trigger in output if isinstance(trigger, str) else trigger.search(output) is not None


def send_serial_input(stream, payload: bytes) -> None:
    if payload == OVERFLOW_INPUT:
        # QEMU's PL011 FIFO is small. Pace the boundary probe so it measures
        # the shell's line limit instead of dropped UART input bytes.
        for offset in range(0, len(payload), 4):
            stream.write(payload[offset : offset + 4])
            stream.flush()
            time.sleep(0.05)
    else:
        stream.write(payload)
        stream.flush()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--ovmf", type=Path, required=True)
    parser.add_argument("--ovmf-vars", type=Path, required=True)
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=35.0)
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="cantaya-smoke-") as directory:
        serial_log = Path(directory) / "serial.log"
        monitor_path = Path(directory) / "qmp.sock"
        # A visible `make run` may have these writable images open already.
        # Give this headless guest private copies so the smoke test can boot.
        vars_copy = Path(directory) / "ovmf-vars.fd"
        image_copy = Path(directory) / "cantaya.img"
        shutil.copyfile(args.ovmf_vars, vars_copy)
        shutil.copyfile(args.image, image_copy)
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
            "-serial", "stdio",
            "-qmp", f"unix:{monitor_path},server=on,wait=off",
            "-display", "none",
            "-no-reboot",
        ]
        # QEMU's PL011 stdio backend reads the pipe while its output still
        # lands in the same file used by the marker contract.
        with serial_log.open("wb") as serial_output:
            process = subprocess.Popen(
                command, stdin=subprocess.PIPE, stdout=serial_output
            )
        output = ""
        keyboard_step = 0
        serial_step = 0
        input_error = None
        deadline = time.monotonic() + args.timeout

        try:
            while time.monotonic() < deadline:
                if serial_log.exists():
                    output = serial_log.read_text(errors="replace")
                    if any(marker in output for marker in FAILURE_MARKERS):
                        break
                    if output.count("Unknown command:") > 1:
                        break
                    if keyboard_step < len(KEYBOARD_STEPS) and (
                        KEYBOARD_STEPS[keyboard_step][0] in output
                    ):
                        try:
                            send_keyboard_keys(
                                monitor_path, KEYBOARD_STEPS[keyboard_step][1]
                            )
                            keyboard_step += 1
                        except (OSError, ValueError, RuntimeError) as error:
                            input_error = str(error)
                            break
                    if serial_step < len(SERIAL_STEPS) and response_seen(
                        SERIAL_STEPS[serial_step][0], output
                    ):
                        try:
                            if process.stdin is None:
                                raise RuntimeError("PL011 input pipe is unavailable")
                            send_serial_input(
                                process.stdin, SERIAL_STEPS[serial_step][1]
                            )
                            serial_step += 1
                        except (OSError, RuntimeError) as error:
                            input_error = str(error)
                            break
                    if all(marker in output for marker in REQUIRED_MARKERS) and all(
                        output.count(marker) >= count
                        for marker, count in REQUIRED_MARKER_COUNTS.items()
                    ) and all(pattern.search(output) for _, pattern in REQUIRED_PATTERNS) and (
                        output.count("Unknown command:") == 1
                    ):
                        break
                if process.poll() is not None:
                    break
                time.sleep(0.1)
        finally:
            if process.stdin is not None:
                process.stdin.close()
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()

        if serial_log.exists():
            output = serial_log.read_text(errors="replace")

    failures = [marker for marker in FAILURE_MARKERS if marker in output]
    missing = [marker for marker in REQUIRED_MARKERS if marker not in output]
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

    print("CantayaOS QEMU smoke test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
