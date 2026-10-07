"""QMP interaction and framebuffer checks for the real EL0 desktop.

Uses QEMU's PPM screenshots directly; no GUI or imaging dependencies required.
"""
from __future__ import annotations

import re
import shutil
import time
from pathlib import Path


class Frame:
    def __init__(self, data: bytes):
        header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\n", data)
        if header is None:
            raise RuntimeError("QEMU screenshot is not an RGB PPM")
        self.width, self.height = map(int, header.groups())
        self.pixels = data[header.end():]
        if len(self.pixels) != self.width * self.height * 3:
            raise RuntimeError("QEMU screenshot has a truncated pixel buffer")

    def color(self, x: int, y: int) -> int:
        offset = (y * self.width + x) * 3
        return int.from_bytes(self.pixels[offset:offset + 3], "big")


def text_matches(frame: Frame, text: bytes, x: int, y: int, background: int = 0x0E2030) -> bool:
    source = (Path(__file__).resolve().parent.parent / "shared/src/font.rs").read_text()
    glyphs = {
        int(code, 16): bytes(int(value, 16) for value in re.findall(r"0x[0-9A-Fa-f]+", values))
        for code, values in re.findall(r"/\* 0x([0-9A-Fa-f]{2})[^\n]*?\*/\s*\[([^\]]+)\]", source)
    }
    for index, byte in enumerate(text):
        for row, bits in enumerate(glyphs[byte]):
            for col in range(8):
                expected = 0xE7F0FA if bits & (0x80 >> col) else background
                if frame.color(x + index * 8 + col, y + row) != expected:
                    return False
    return True

def screenshot(stream, execute, path: Path) -> Frame:
    execute(stream, {"execute": "screendump", "arguments": {"filename": str(path)}})
    return Frame(path.read_bytes())


def check_desktop(stream, execute, directory: Path, serial_log: Path,
                  deadline: float, artifact: Path | None = None) -> None:
    path = directory / "desktop.ppm"

    def await_frame(description, predicate):
        end = min(deadline, time.monotonic() + 10)
        while time.monotonic() < end:
            frame = screenshot(stream, execute, path)
            if predicate(frame):
                return frame
            time.sleep(0.1)
        raise RuntimeError(f"Desktop framebuffer check failed: {description}")

    def pointer(x, y, down=None):
        events = [
            {"type": "abs", "data": {"axis": "x", "value": (x * 32767 + 1022) // 1023}},
            {"type": "abs", "data": {"axis": "y", "value": (y * 32767 + 766) // 767}},
        ]
        if down is not None:
            events.append({"type": "btn", "data": {"button": "left", "down": down}})
        execute(stream, {"execute": "input-send-event", "arguments": {"events": events}})

    def click(x, y):
        pointer(x, y, True)
        pointer(x, y, False)

    def key(code):
        for down in (True, False):
            execute(stream, {"execute": "input-send-event", "arguments": {"events": [
                {"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": code}}}
            ]}})
        time.sleep(0.05)

    def resize(x, y, width, height, dx, dy):
        grip_x, grip_y = x + width - 6, y + height - 6
        pointer(grip_x, grip_y, True)
        pointer(grip_x + dx, grip_y + dy, True)
        pointer(grip_x + dx, grip_y + dy, False)
        pointer(1006, 684)

    initial = await_frame("initial terminal focus", lambda f: f.color(26, 144) == 0x23445A)
    if (initial.width, initial.height) != (1024, 768):
        raise RuntimeError("Desktop smoke requires the supported 1024x768 GOP mode")
    if initial.color(0, 0) != 0x081424:
        raise RuntimeError("Desktop wallpaper or RGB pixel conversion is incorrect")
    files_x = 1024 * 56 // 100 + 44
    preview_y = 142 + (768 - 218) - 144
    pointer(800, 300)
    await_frame("absolute mouse cursor", lambda f: f.color(800, 300) == 0x071422)
    click(220, 740)
    await_frame("Files taskbar focus", lambda f: f.color(files_x + 2, 144) == 0x23445A and f.color(26, 144) == 0x1D3248)
    for code, expected in (("f1", 0x23445A), ("tab", 0x1D3248), ("tab", 0x23445A), ("f2", 0x1D3248)):
        key(code)
        await_frame(f"{code} focus", lambda f, color=expected: f.color(26, 144) == color)

    # Derive file-row positions from the actual root listing tested earlier.
    output = serial_log.read_text(errors="replace")
    root = output.split("\nnormalboot\ncantaya> ", 1)[1].split("cantaya> ", 1)[0]
    names = re.findall(r"^([A-Z0-9_.]+)(?:/|  \d+ bytes)$", root, re.MULTILINE)
    if "README.TXT" not in names or "DOCS" not in names:
        raise RuntimeError("Cannot locate desktop file rows in the checked root listing")
    click(files_x + 80, 142 + 98 + names.index("README.TXT") * 24 + 10)
    await_frame("README text preview", lambda f: text_matches(f, b"CantayaOS boot disk", files_x + 22, preview_y + 32))
    click(files_x + 80, 142 + 98 + names.index("DOCS") * 24 + 10)
    await_frame("directory navigation", lambda f: f.color(files_x + 21, 142 + 98 + 24 + 7) == 0x14263A)
    click(files_x + 80, 142 + 98 + 10)
    await_frame("subdirectory text preview", lambda f: text_matches(f, b"This note lives in the DOCS directory.", files_x + 22, preview_y + 32))
    click(files_x + 36, 202)
    await_frame("Back navigation", lambda f: f.color(files_x + 21, 142 + 98 + 24 + 7) == 0x78AFDA)
    # Use structured non-text keys to open a file from the browser.
    key("home")
    await_frame("Home browser selection", lambda f: f.color(files_x + 14, 142 + 98 + 3) == 0x24485B)
    for index in range(names.index("README.TXT")):
        key("down")
        await_frame(f"Down browser selection {index + 1}", lambda f, row=index + 1:
                    f.color(files_x + 14, 142 + 98 + row * 24 + 3) == 0x24485B)
    key("ret")
    await_frame("keyboard file opening", lambda f: text_matches(f, b"CantayaOS boot disk", files_x + 22, preview_y + 32))

    # Repeated keyboard-only opens exercise event batches and real file I/O.
    for _ in range(3):
        key("home")
        for _ in range(names.index("DOCS")):
            key("down")
        key("ret")
        await_frame("repeated folder opening", lambda f: f.color(files_x + 21, 142 + 98 + 24 + 7) == 0x14263A)
        key("ret")
        await_frame("repeated note preview", lambda f: text_matches(f, b"This note lives in the DOCS directory.", files_x + 22, preview_y + 32))
        key("esc")
        await_frame("keyboard Back navigation", lambda f: f.color(files_x + 21, 142 + 98 + 24 + 7) == 0x78AFDA)
        for _ in range(names.index("README.TXT")):
            key("down")
        key("ret")
        await_frame("repeated root preview", lambda f: text_matches(f, b"CantayaOS boot disk", files_x + 22, preview_y + 32))

    pointer(files_x + 120, 157, True)
    pointer(files_x + 40, 197, True)
    pointer(files_x + 40, 197, False)
    moved_x, moved_y = files_x - 80, 182
    await_frame("window dragging", lambda f: f.color(moved_x + 2, moved_y + 2) == 0x23445A)
    click(moved_x + 383 - 18, moved_y + 16)
    await_frame("window close", lambda f: f.color(moved_x + 2, moved_y + 2) != 0x23445A)
    click(220, 740)
    await_frame("taskbar reopening", lambda f: f.color(moved_x + 2, moved_y + 2) == 0x23445A)
    # Leave a tidy default layout with the selected README preview visible.
    pointer(moved_x + 120, moved_y + 15, True)
    pointer(files_x + 120, 157, True)
    pointer(files_x + 120, 157, False)
    await_frame("restored window position", lambda f: f.color(files_x + 2, 144) == 0x23445A)
    pointer(1006, 684)
    await_frame("cursor after window interaction", lambda f: f.color(1006, 684) == 0x071422)
    throttle = {"device": "cantaya-disk", "bps": 0, "bps_rd": 0, "bps_wr": 0,
                "iops": 1, "iops_rd": 0, "iops_wr": 0}
    execute(stream, {"execute": "block_set_io_throttle", "arguments": throttle})
    try:
        click(files_x + 80, 142 + 98 + names.index("README.TXT") * 24 + 10)
        await_frame("bounded disk timeout", lambda f: text_matches(
            f, b"Cannot open this file.", files_x + 22, preview_y + 32))
        if "VirtIO block: read timeout; retaining pending DMA request" not in serial_log.read_text(errors="replace"):
            raise RuntimeError("Delayed disk did not exercise pending-request retention")
    finally:
        throttle["iops"] = 0
        execute(stream, {"execute": "block_set_io_throttle", "arguments": throttle})
    click(files_x + 80, 142 + 98 + names.index("README.TXT") * 24 + 10)
    await_frame("disk recovery after late completion", lambda f: text_matches(
        f, b"CantayaOS boot disk", files_x + 22, preview_y + 32))
    key("f1")
    for command in ("clear", "info", "help"):
        for code in (*command, "ret"):
            key(code)
        if command == "info":
            await_frame("terminal output after window interaction", lambda f: text_matches(
                f, b"CantayaOS v0.1.0", 40, 142 + 48 + 16, 0x0C1B2A))
    await_frame("graphical terminal help", lambda f: text_matches(
        f, b"help          Show commands", 40, 142 + 48 + 3 * 16, 0x0C1B2A))
    # Paint owns a separate TTBR0 address space and copied surface. The desktop
    # remains interactive while it blocks on its own routed window input.
    app_x, app_y = (1024 - 336) // 2, 212
    content_x, content_y = app_x + 8, app_y + 42
    key("f3")
    await_frame("independent Paint launch", lambda f: f.color(app_x + 2, app_y + 2) == 0x23445A
                and f.color(content_x + 40, content_y + 80) == 0xE7F0FA)
    pointer(content_x + 40, content_y + 80, True)
    pointer(content_x + 120, content_y + 110, True)
    pointer(content_x + 120, content_y + 110, False)
    pointer(1006, 684)
    await_frame("Paint mouse stroke", lambda f: f.color(content_x + 80, content_y + 95) == 0x193B55)
    key("2")
    await_frame("Paint keyboard color selection", lambda f: f.color(content_x + 138, content_y + 8) == 0xE7F0FA)
    pointer(content_x + 68, content_y + 146, True)
    pointer(content_x + 188, content_y + 166, True)
    pointer(content_x + 188, content_y + 166, False)
    pointer(1006, 684)
    await_frame("Paint selected-color stroke", lambda f: f.color(content_x + 128, content_y + 156) == 0xD95F76)
    key("f1")
    await_frame("Terminal while Paint runs", lambda f: f.color(26, 144) == 0x23445A)
    for code in (*"clear", "ret", *"echo responsive", "ret"):
        key("spc" if code == " " else code)
    await_frame("responsive terminal during GUI application", lambda f: text_matches(
        f, b"responsive", 40, 142 + 48 + 16, 0x0C1B2A))
    key("f3")
    await_frame("Paint surface retained across focus", lambda f: f.color(content_x + 128, content_y + 156) == 0xD95F76)
    key("c")
    await_frame("Paint clear", lambda f: f.color(content_x + 128, content_y + 156) == 0xE7F0FA)
    pointer(app_x + 120, app_y + 15, True)
    pointer(app_x + 200, app_y + 45, True)
    pointer(app_x + 200, app_y + 45, False)
    await_frame("independent window dragging", lambda f: f.color(app_x + 82, app_y + 32) == 0x23445A)
    click(app_x + 80 + 336 - 18, app_y + 30 + 16)
    await_frame("independent window close", lambda f: f.color(app_x + 82, app_y + 32) != 0x23445A)

    # Launch the installed binary from Files, then let Esc end the application.
    key("f2")
    key("home")
    for _ in range(names.index("BIN")):
        key("down")
    key("ret")
    await_frame("application directory", lambda f: text_matches(f, b"HELLO.ELF", files_x + 44, 142 + 98 + 4, 0x24485B))
    key("down")
    key("ret")
    await_frame("Files launches PAINT.ELF", lambda f: f.color(app_x + 2, app_y + 2) == 0x23445A
                and f.color(content_x + 40, content_y + 80) == 0xE7F0FA)
    key("esc")
    await_frame("application-owned close", lambda f: f.color(app_x + 2, app_y + 2) != 0x23445A)
    key("f3")
    await_frame("fresh application surface after close", lambda f: f.color(app_x + 2, app_y + 2) == 0x23445A
                and f.color(content_x + 128, content_y + 156) == 0xE7F0FA)

    # A second GUI app launched through the terminal also runs asynchronously.
    key("f1")
    for code in (*"run BIN/PAINT.ELF", "ret"):
        # QEMU qcodes use lowercase keys; Shift supplies the path capitals.
        if code.isupper():
            execute(stream, {"execute": "input-send-event", "arguments": {"events": [
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "shift"}}}
            ]}})
        key({" ": "spc", "/": "slash", ".": "dot"}.get(code, code.lower()))
        if code.isupper():
            execute(stream, {"execute": "input-send-event", "arguments": {"events": [
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "shift"}}}
            ]}})
    await_frame("two independent graphical applications", lambda f: f.color(app_x + 20, app_y + 26) == 0x23445A
                and f.color(app_x + 2, app_y + 2) == 0x1D3248)
    key("f1")
    key("tab")
    key("tab")
    await_frame("Tab focuses first application", lambda f: f.color(app_x + 2, app_y + 2) == 0x23445A)
    key("tab")
    await_frame("Tab focuses second application", lambda f: f.color(app_x + 20, app_y + 26) == 0x23445A)
    key("esc")
    await_frame("second application exits", lambda f: f.color(26, 144) == 0x23445A)
    key("f3")
    await_frame("first application remains alive", lambda f: f.color(app_x + 2, app_y + 2) == 0x23445A)
    await_frame("asynchronous GUI completion", lambda f: "Program exited: 0x0" in serial_log.read_text(errors="replace"))
    output = serial_log.read_text(errors="replace")
    if output.count("Paint ready.") != 4:
        raise RuntimeError("Desktop did not launch four isolated Paint instances")
    if "Program exited: 0x0" not in output:
        raise RuntimeError("Asynchronous GUI completion was not reaped by the terminal")
    # Resizing changes the copied surface stride. Hidden drawing must survive
    # shrink/regrow, and subsequent pointer/key input must use the new geometry.
    key("1")
    click(content_x + 60, content_y + 90)
    click(content_x + 280, content_y + 190)
    pointer(1006, 684)
    await_frame("drawing before resize", lambda f: f.color(content_x + 280, content_y + 190) == 0x193B55)
    resize(app_x, app_y, 336, 290, -80, -60)
    await_frame("application surface shrink", lambda f:
                text_matches(f, b"1-4 color  C clear  Esc quit", content_x + 12, content_y + 162, 0x14263A)
                and f.color(content_x + 60, content_y + 90) == 0x193B55)
    key("2")
    click(content_x + 120, content_y + 110)
    pointer(1006, 684)
    await_frame("input after surface resize", lambda f: f.color(content_x + 120, content_y + 110) == 0xD95F76)
    resize(app_x, app_y, 256, 230, 80, 60)
    await_frame("hidden drawing restored after grow", lambda f:
                f.color(content_x + 280, content_y + 190) == 0x193B55
                and f.color(content_x + 120, content_y + 110) == 0xD95F76)
    resize(app_x, app_y, 336, 290, -400, -300)
    await_frame("application minimum size", lambda f:
                text_matches(f, b"1-4 C Esc", content_x + 12, content_y + 102, 0x14263A))
    resize(app_x, app_y, 176, 170, 400, 300)
    await_frame("application maximum size", lambda f:
                f.color(content_x + 280, content_y + 190) == 0x193B55)
    for iteration in range(3):
        resize(app_x, app_y, 336, 290, -80, -60)
        await_frame(f"repeated surface shrink {iteration}", lambda f:
                    text_matches(f, b"1-4 color  C clear  Esc quit", content_x + 12, content_y + 162, 0x14263A))
        resize(app_x, app_y, 256, 230, 80, 60)
        await_frame(f"repeated surface grow {iteration}", lambda f:
                    f.color(content_x + 280, content_y + 190) == 0x193B55)
    click(app_x + 336 - 54, app_y + 18)
    await_frame("minimized app focuses Terminal", lambda f: f.color(26, 144) == 0x23445A)
    key("2")
    key("backspace")
    key("f2")
    key("tab")
    await_frame("Tab restores minimized app", lambda f:
                f.color(app_x + 2, app_y + 2) == 0x23445A
                and f.color(content_x + 280, content_y + 190) == 0x193B55)
    click(app_x + 336 - 54, app_y + 18)
    await_frame("app minimized again", lambda f: f.color(26, 144) == 0x23445A)
    click(400, 740)
    await_frame("taskbar restores minimized app", lambda f:
                f.color(content_x + 120, content_y + 110) == 0xD95F76)
    if serial_log.read_text(errors="replace").count("Paint ready.") != 4:
        raise RuntimeError("Minimize/restore restarted Paint")

    # Reflow an unfinished command, then execute it after restoring the window.
    key("f1")
    for code in (*"clear", "ret"):
        key(code)
    pattern = "abcdefghijklmnopqrstuvwxyz" * 3 + "wxyz"
    line = b"cantaya> echo " + pattern.encode()
    for code in "echo " + pattern:
        key("spc" if code == " " else code)
    resize(24, 142, 573, 550, -173, -100)
    await_frame("terminal command reflows on shrink", lambda f:
                text_matches(f, line[:46], 40, 190, 0x0C1B2A)
                and text_matches(f, line[46:92], 40, 206, 0x0C1B2A)
                and text_matches(f, line[92:], 40, 222, 0x0C1B2A))
    resize(24, 142, 400, 450, 173, 100)
    await_frame("terminal command reflows on grow", lambda f:
                text_matches(f, line[:67], 40, 190, 0x0C1B2A)
                and text_matches(f, line[67:], 40, 206, 0x0C1B2A))
    key("ret")
    await_frame("resized terminal executes preserved input", lambda f:
                text_matches(f, pattern[:67].encode(), 40, 222, 0x0C1B2A))
    click(24 + 573 - 54, 160)
    await_frame("Terminal minimize", lambda f: f.color(26, 144) != 0x23445A)
    key("f1")
    await_frame("Terminal restore retains output", lambda f:
                text_matches(f, pattern[:67].encode(), 40, 222, 0x0C1B2A))

    key("f2")
    key("esc")
    key("home")
    for _ in range(names.index("README.TXT")):
        key("down")
    key("ret")
    await_frame("README after application lifecycle", lambda f: text_matches(f, b"CantayaOS boot disk", files_x + 22, preview_y + 32))
    resize(files_x, 142, 383, 550, -83, -170)
    await_frame("Files resized preview", lambda f:
                text_matches(f, b"CantayaOS boot disk", files_x + 22, 142 + 380 - 144 + 32))
    key("home")
    for _ in range(len(names) - 1):
        key("down")
    await_frame("resized Files keeps selection visible", lambda f:
                text_matches(f, names[-1].encode(), files_x + 44,
                             142 + 98 + min(len(names) - 1, 4) * 24 + 4, 0x24485B))
    key("home")
    for _ in range(names.index("README.TXT")):
        key("down")
    key("ret")
    resize(files_x, 142, 300, 380, 83, 170)
    await_frame("Files restored size and preview", lambda f:
                text_matches(f, b"CantayaOS boot disk", files_x + 22, preview_y + 32))
    click(files_x + 383 - 54, 160)
    await_frame("Files minimize", lambda f: f.color(files_x + 2, 144) != 0x23445A)
    key("f2")
    await_frame("Files restore retains preview", lambda f:
                text_matches(f, b"CantayaOS boot disk", files_x + 22, preview_y + 32))
    key("f1")
    for command in ("clear", "info", "help"):
        for code in (*command, "ret"):
            key(code)
    await_frame("terminal after asynchronous applications", lambda f: text_matches(f, b"help          Show commands", 40, 142 + 48 + 3 * 16, 0x0C1B2A))
    key("f3")
    key("1")
    key("c")
    pointer(content_x + 50, content_y + 142, True)
    pointer(content_x + 126, content_y + 74, True)
    pointer(content_x + 126, content_y + 74, False)
    pointer(content_x + 126, content_y + 74, True)
    pointer(content_x + 202, content_y + 142, True)
    pointer(content_x + 202, content_y + 142, False)
    key("3")
    pointer(content_x + 50, content_y + 158, True)
    pointer(content_x + 240, content_y + 158, True)
    pointer(content_x + 240, content_y + 158, False)
    pointer(1006, 684)
    await_frame("final retained Paint drawing", lambda f: f.color(content_x + 150, content_y + 158) == 0x168A7A)
    if artifact is not None:
        artifact.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, artifact)
    print("CantayaOS desktop input, browser, disk recovery, independent Paint, resizing, minimize, async launch and framebuffer checks passed.")
