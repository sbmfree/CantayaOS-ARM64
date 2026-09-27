#!/usr/bin/env python3
"""Boot CantayaOS headlessly and verify the baseline MMU/scheduler contract."""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
import time
from pathlib import Path


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
    "[System] heartbeat",
    "[Thread-A] alive",
    "[Thread-B] alive",
)
REQUIRED_MARKER_COUNTS = {
    "Ps: reaped thread": 5,
    "[user-init] EL0 fixed VM reuse validated": 2,
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
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--ovmf", type=Path, required=True)
    parser.add_argument("--ovmf-vars", type=Path, required=True)
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=25.0)
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="cantaya-smoke-") as directory:
        serial_log = Path(directory) / "serial.log"
        command = [
            args.qemu,
            "-machine", "virt,highmem=on",
            "-cpu", "cortex-a57",
            "-m", "512M",
            "-device", "ramfb",
            "-nic", "none",
            "-drive", f"if=pflash,format=raw,file={args.ovmf},readonly=on",
            "-drive", f"if=pflash,format=raw,file={args.ovmf_vars}",
            "-drive", f"if=none,format=raw,file={args.image},id=cantaya-disk",
            "-global", "virtio-mmio.force-legacy=false",
            "-device", "virtio-blk-device,drive=cantaya-disk",
            "-serial", f"file:{serial_log}",
            "-display", "none",
            "-no-reboot",
        ]
        process = subprocess.Popen(command)
        output = ""
        deadline = time.monotonic() + args.timeout

        try:
            while time.monotonic() < deadline:
                if serial_log.exists():
                    output = serial_log.read_text(errors="replace")
                    if any(marker in output for marker in FAILURE_MARKERS):
                        break
                    if all(marker in output for marker in REQUIRED_MARKERS) and all(
                        output.count(marker) >= count
                        for marker, count in REQUIRED_MARKER_COUNTS.items()
                    ):
                        break
                if process.poll() is not None:
                    break
                time.sleep(0.1)
        finally:
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
    if failures or missing:
        print("CantayaOS QEMU smoke test failed.", file=sys.stderr)
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
