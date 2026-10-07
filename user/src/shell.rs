//! Interactive EL0 shell over the bounded console pseudo-handles.

use core::{
    arch::asm,
    fmt::{self, Write},
};

const MAX_LINE: usize = 128;
const PROMPT: &str = "cantaya> ";
const STATUS_TIMEOUT: u64 = 0x102;
const STATUS_NO_MORE_FILES: u64 = 0x8000_0006;
const STATUS_INVALID_HANDLE: u64 = 0xC000_0008;
const STATUS_INVALID_PARAMETER: u64 = 0xC000_000D;
const STATUS_NO_SUCH_FILE: u64 = 0xC000_000F;
const STATUS_ACCESS_VIOLATION: u64 = 0xC000_0005;
const STATUS_INVALID_IMAGE_FORMAT: u64 = 0xC000_007B;

struct ShellWriter;

impl Write for ShellWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if text.is_empty() {
            return Ok(());
        }
        // Formatting may call us with a long slice; NtWriteFile's limit is
        // 1,024 bytes. All accepted shell text is ASCII, so chunks are UTF-8.
        for chunk in text.as_bytes().chunks(1024) {
            let status: u64;
            unsafe {
                asm!(
                    "svc #0",
                    inlateout("x0") u64::MAX => status,
                    in("x1") chunk.as_ptr(),
                    in("x2") chunk.len(),
                    in("x8") 0x8_u64,
                );
            }
            if status != 0 {
                return Err(fmt::Error);
            }
        }
        Ok(())
    }
}

pub(crate) fn output(args: fmt::Arguments<'_>) -> Result<(), ()> {
    ShellWriter.write_fmt(args).map_err(|_| ())
}

fn read_one() -> Result<Option<u8>, ()> {
    let mut byte = 0u8;
    let mut count = 0u64;
    let status: u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") (u64::MAX - 1) => status,
            in("x1") &mut byte,
            in("x2") 1_u64,
            in("x3") &mut count,
            in("x8") 0x6_u64,
        );
    }
    match status {
        0 if count == 1 => Ok(Some(byte)),
        STATUS_TIMEOUT => Ok(None),
        _ => Err(()),
    }
}

pub(crate) fn wait_for_input() -> Result<(), ()> {
    let status: u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") (u64::MAX - 1) => status,
            in("x8") 0x38_u64,
        );
    }
    (status == 0).then_some(()).ok_or(())
}

fn clear() -> Result<(), ()> {
    let status: u64;
    unsafe {
        asm!("svc #0", inlateout("x0") 0_u64 => status, in("x8") 0x37_u64);
    }
    (status == 0).then_some(()).ok_or(())
}

pub(crate) fn system_value(class: u64) -> Result<u64, ()> {
    let mut value = 0u64;
    let status: u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") class => status,
            in("x1") &mut value,
            in("x8") 0x36_u64,
        );
    }
    (status == 0).then_some(value).ok_or(())
}

pub(crate) fn open_file(name: &str) -> Result<u64, ()> {
    let mut handle = 0u64;
    let status: u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") &mut handle => status,
            in("x1") name.as_ptr(),
            in("x2") name.len(),
            in("x8") 0x55_u64,
        );
    }
    (status == 0).then_some(handle).ok_or(())
}

pub(crate) fn read_file(handle: u64, bytes: &mut [u8]) -> Result<usize, ()> {
    let mut count = 0u64;
    let status: u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") handle => status,
            in("x1") bytes.as_mut_ptr(),
            in("x2") bytes.len(),
            in("x3") &mut count,
            in("x8") 0x6_u64,
        );
    }
    if status == 0 && count <= bytes.len() as u64 {
        Ok(count as usize)
    } else {
        Err(())
    }
}

pub(crate) fn close_handle(handle: u64) {
    let status: u64;
    unsafe { asm!("svc #0", inlateout("x0") handle => status, in("x8") 0xf_u64) }
    let _ = status;
}

pub(crate) fn directory_entry(directory: Option<&str>, index: u64, entry: &mut [u8; 16]) -> u64 {
    let status: u64;
    unsafe {
        if let Some(name) = directory {
            asm!("svc #0", inlateout("x0") name.as_ptr() => status,
                in("x1") name.len(), in("x2") index, in("x3") entry.as_mut_ptr(), in("x8") 0x3a_u64);
        } else {
            asm!("svc #0", inlateout("x0") index => status,
                in("x1") entry.as_mut_ptr(), in("x8") 0x39_u64);
        }
    }
    status
}

fn list_files(directory: Option<&str>) -> Result<(), ()> {
    for index in 0..1024u64 {
        let mut entry = [0u8; 16];
        let status = directory_entry(directory, index, &mut entry);
        if status == STATUS_NO_MORE_FILES {
            return Ok(());
        }
        if status == STATUS_NO_SUCH_FILE {
            return output(format_args!("Cannot list directory\n"));
        }
        if status != 0 {
            return output(format_args!("Directory read failed\n"));
        }
        let length = entry[..12].iter().position(|&byte| byte == 0).unwrap_or(12);
        let name = core::str::from_utf8(&entry[..length]).map_err(|_| ())?;
        let size = u32::from_le_bytes(entry[12..16].try_into().map_err(|_| ())?);
        if size == u32::MAX {
            output(format_args!("{name}/\n"))?;
        } else {
            output(format_args!("{name}  {size} bytes\n"))?;
        }
    }
    output(format_args!("Directory listing limit reached\n"))
}

fn show_file(name: &str) -> Result<(), ()> {
    let Ok(handle) = open_file(name) else {
        return output(format_args!("Cannot open file: {name}\n"));
    };
    let result = (|| {
        let mut bytes = [0u8; 128];
        loop {
            let Ok(count) = read_file(handle, &mut bytes) else {
                return output(format_args!("File read failed\n"));
            };
            if count == 0 {
                return Ok(());
            }
            let chunk = &bytes[..count];
            if !chunk
                .iter()
                .all(|&byte| matches!(byte, b'\x07' | b'\x08' | b'\r' | b'\n' | b' '..=b'~'))
            {
                return output(format_args!("File contains unsupported bytes\n"));
            }
            let text = core::str::from_utf8(chunk).map_err(|_| ())?;
            output(format_args!("{text}"))?;
        }
    })();
    close_handle(handle);
    result
}

fn run_program(specification: &str, pending: Option<&mut [u64; 4]>) -> Result<(), ()> {
    if pending.as_ref().is_some_and(|p| p.iter().all(|h| *h != 0)) {
        return output(format_args!("Four programs are already running.\n"));
    }
    let (name, arguments) = specification
        .split_once(' ')
        .map_or((specification, ""), |(name, rest)| (name, rest.trim()));
    if name.is_empty() || arguments.len() > 64 {
        return output(format_args!("Cannot run program: {name}\n"));
    }
    let mut handle = 0u64;
    let mut status: u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") &mut handle => status,
            in("x1") 2_u64,
            in("x2") if arguments.is_empty() { 0 } else { arguments.as_ptr() as u64 },
            in("x3") name.as_ptr(),
            in("x4") name.len(),
            in("x5") arguments.len(),
            in("x8") 0x4c_u64,
        );
    }
    if status != 0 {
        return output(format_args!("Cannot run program: {name}\n"));
    }
    if let Some(pending) = pending {
        *pending.iter_mut().find(|h| **h == 0).unwrap() = handle;
        return output(format_args!("Program started: {name}\n"));
    }
    let mut completion = 0i32;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") handle => status,
            in("x1") 0_u64,
            in("x2") 0_u64,
            in("x3") &mut completion,
            in("x8") 0x4_u64,
        );
    }
    close_handle(handle);
    if status != 0 {
        output(format_args!("Program wait failed: {name}\n"))
    } else {
        output(format_args!("Program exited: {:#x}\n", completion as u32))
    }
}

fn file_contract_probe() -> bool {
    let name = "README.TXT";
    let mut status: u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") 0_u64 => status,
            in("x1") name.as_ptr(),
            in("x2") name.len(),
            in("x8") 0x55_u64,
        );
    }
    if status != STATUS_INVALID_PARAMETER {
        return false;
    }
    let invalid_name = "BAD/TOO/LONG";
    let mut rejected_handle = 0x55aa_55aa_u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") &mut rejected_handle => status,
            in("x1") invalid_name.as_ptr(),
            in("x2") invalid_name.len(),
            in("x8") 0x55_u64,
        );
    }
    if status != STATUS_INVALID_PARAMETER || rejected_handle != 0x55aa_55aa {
        return false;
    }
    let Ok(handle) = open_file(name) else {
        let _ = output(format_args!("[user-shell] file probe open failed\n"));
        return false;
    };
    let mut bytes = [0u8; 128];
    let mut count = 0x55aa_55aa_u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") handle => status,
            in("x1") 0x1234_5678_u64,
            in("x2") 1_u64,
            in("x3") &mut count,
            in("x8") 0x6_u64,
        );
    }
    if status != STATUS_ACCESS_VIOLATION || count != 0x55aa_55aa {
        let _ = output(format_args!(
            "[user-shell] file probe invalid read status={status:#x} count={count}\n"
        ));
        close_handle(handle);
        return false;
    }
    let first = read_file(handle, &mut bytes);
    let end = read_file(handle, &mut bytes);
    close_handle(handle);
    if first != Ok(84) || end != Ok(0) {
        return false;
    }
    let Ok(replacement) = open_file(name) else {
        return false;
    };
    if replacement == handle {
        close_handle(replacement);
        return false;
    }
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") handle => status,
            in("x1") bytes.as_mut_ptr(),
            in("x2") 1_u64,
            in("x3") &mut count,
            in("x8") 0x6_u64,
        );
    }
    close_handle(replacement);
    if status != STATUS_INVALID_HANDLE || count != 0x55aa_55aa {
        return false;
    }
    let mut entry = [0x55u8; 16];
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") 1023_u64 => status,
            in("x1") entry.as_mut_ptr(),
            in("x8") 0x39_u64,
        );
    }
    if status != STATUS_NO_MORE_FILES || entry != [0x55u8; 16] {
        return false;
    }
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") 0_u64 => status,
            in("x1") 0_u64,
            in("x8") 0x39_u64,
        );
    }
    status == STATUS_INVALID_PARAMETER
}

fn malformed_fixture_probe() -> bool {
    let broken = "BROKEN.TXT";
    let mut rejected_handle = 0x55aa_55aa_u64;
    let mut status: u64;
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") &mut rejected_handle => status,
            in("x1") broken.as_ptr(),
            in("x2") broken.len(),
            in("x8") 0x55_u64,
        );
    }
    if status != STATUS_NO_SUCH_FILE || rejected_handle != 0x55aa_55aa {
        let _ = output(format_args!(
            "[user-shell] malformed FAT status={status:#x} handle={rejected_handle:#x}\n"
        ));
        return false;
    }
    let Ok(good_handle) = open_file("README.TXT") else {
        return false;
    };
    close_handle(good_handle);

    for name in ["BAD.ELF", "BADWX.ELF"] {
        let mut rejected_process = 0x55aa_55aa_u64;
        unsafe {
            asm!(
                "svc #0",
                inlateout("x0") &mut rejected_process => status,
                in("x1") 2_u64,
                in("x2") 0_u64,
                in("x3") name.as_ptr(),
                in("x4") name.len(),
                in("x5") 0_u64,
                in("x8") 0x4c_u64,
            );
        }
        if status != STATUS_INVALID_IMAGE_FORMAT || rejected_process != 0x55aa_55aa {
            let _ = output(format_args!(
                "[user-shell] malformed ELF {name} status={status:#x} handle={rejected_process:#x}\n"
            ));
            if status == 0 {
                close_handle(rejected_process);
            }
            return false;
        }
    }
    true
}

fn exit(status: u64) -> ! {
    unsafe {
        asm!(
            "svc #0",
            in("x0") u64::MAX,
            in("x1") status,
            in("x8") 0x29_u64,
        );
    }
    loop {
        core::hint::spin_loop();
    }
}

pub(crate) struct Shell {
    line: [u8; MAX_LINE],
    len: usize,
    after_cr: bool,
    graphical: bool,
    pending: [u64; 4],
}

impl Shell {
    pub(crate) fn new() -> Self {
        Self {
            line: [0; MAX_LINE],
            len: 0,
            after_cr: false,
            graphical: false,
            pending: [0; 4],
        }
    }

    pub(crate) fn poll_programs(&mut self) -> Result<(), ()> {
        for handle in &mut self.pending {
            if *handle == 0 {
                continue;
            }
            let timeout = 0u64;
            let mut completion = 0i32;
            let status: u64;
            unsafe {
                asm!("svc #0", inlateout("x0") *handle => status,
                in("x1") 0u64, in("x2") &timeout, in("x3") &mut completion, in("x8") 4u64);
            }
            if status == STATUS_TIMEOUT {
                continue;
            }
            close_handle(*handle);
            *handle = 0;
            if status != 0 {
                return Err(());
            }
            output(format_args!(
                "\nProgram exited: {:#x}\n{PROMPT}",
                completion as u32
            ))?;
            if self.len != 0 {
                output(format_args!(
                    "{}",
                    core::str::from_utf8(&self.line[..self.len]).map_err(|_| ())?
                ))?;
            }
        }
        Ok(())
    }

    pub(crate) fn accept(&mut self, byte: u8) -> Result<(), ()> {
        if byte == b'\n' && self.after_cr {
            self.after_cr = false;
            return Ok(());
        }
        self.after_cr = byte == b'\r';

        match byte {
            b'\r' | b'\n' => {
                output(format_args!("\n"))?;
                self.run_command()?;
                self.len = 0;
                output(format_args!("{PROMPT}"))?;
            }
            8 | 127 => {
                if self.len > 0 {
                    self.len -= 1;
                    output(format_args!("\x08 \x08"))?;
                }
            }
            0x15 => {
                while self.len > 0 {
                    self.len -= 1;
                    output(format_args!("\x08 \x08"))?;
                }
            }
            0x0c => {
                clear()?;
                output(format_args!(
                    "{PROMPT}{}",
                    core::str::from_utf8(&self.line[..self.len]).map_err(|_| ())?
                ))?;
            }
            0x20..=0x7e => {
                if self.len < MAX_LINE {
                    self.line[self.len] = byte;
                    self.len += 1;
                    output(format_args!("{}", byte as char))?;
                } else {
                    output(format_args!("\x07"))?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn run_command(&mut self) -> Result<(), ()> {
        let command = core::str::from_utf8(&self.line[..self.len])
            .map_err(|_| ())?
            .trim();
        match command {
            "" => Ok(()),
            "help" | "?" => output(format_args!(
                "help          Show commands\n\
                 info          Show OS version\n\
                 uptime        Show time since boot\n\
                 mem           Show free physical memory\n\
                 echo <text>   Print text\n\
                 clear         Clear the terminal screen\n\
                 ls [DIR]      List files\n\
                 cat PATH      Show a text file\n\
                 run PATH      Run a program\n\
                 Ctrl-U clears input; Ctrl-L clears the screen.\n"
            )),
            "info" => output(format_args!(
                "CantayaOS v{} (AArch64, QEMU virt)\n",
                env!("CARGO_PKG_VERSION")
            )),
            "uptime" => {
                let ticks = system_value(1)?;
                output(format_args!(
                    "Uptime: {}.{:02} seconds\n",
                    ticks / 100,
                    ticks % 100
                ))
            }
            "mem" => {
                let pages = system_value(0)?;
                output(format_args!(
                    "Free physical memory: {} MiB ({} pages)\n",
                    pages / 256,
                    pages
                ))
            }
            "clear" => clear(),
            "ls" => list_files(None),
            "echo" => output(format_args!("\n")),
            _ => {
                if let Some(text) = command.strip_prefix("echo ") {
                    output(format_args!("{text}\n"))
                } else if let Some(name) = command.strip_prefix("cat ") {
                    show_file(name.trim())
                } else if let Some(directory) = command.strip_prefix("ls ") {
                    list_files(Some(directory.trim()))
                } else if let Some(name) = command.strip_prefix("run ") {
                    run_program(
                        name.trim(),
                        if self.graphical {
                            Some(&mut self.pending)
                        } else {
                            None
                        },
                    )
                } else {
                    output(format_args!("Unknown command: {command}\n"))
                }
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn el0_shell_main(_stack_top: u64, mode: u64) -> ! {
    let first_byte = match read_one() {
        Ok(value) => value,
        Err(()) => exit(1),
    };
    if mode == 0x7e {
        if !file_contract_probe() || !malformed_fixture_probe() {
            exit(5);
        }
        if output(format_args!(
            "[user-shell] read-only file contract validated\n\
             [user-shell] malformed file and ELF fixtures rejected\n"
        ))
        .is_err()
        {
            exit(5);
        }
    }
    let mut desktop = crate::desktop::Desktop::start();
    if mode == 0x7e && desktop.as_mut().is_some_and(|d| !d.contract_probe()) {
        exit(6);
    }
    if output(format_args!("[user-shell] EL0 command loop ready\n")).is_err() {
        exit(2);
    }
    if let Some(ref mut desktop) = desktop {
        desktop.reset_transcript();
    }
    if output(format_args!(
        "\nCantayaOS terminal. Type 'help' for commands.\n{PROMPT}"
    ))
    .is_err()
    {
        exit(2);
    }
    if mode == 0x7d {
        if output(format_args!(
            "[user-shell] fallback probe releasing input\n"
        ))
        .is_err()
        {
            exit(2);
        }
        exit(0x7d);
    }

    let mut shell = Shell::new();
    if let Some(ref mut desktop) = desktop {
        shell.graphical = true;
        if let Some(byte) = first_byte {
            if shell.accept(byte).is_err() {
                exit(3);
            }
        }
        desktop.run(&mut shell);
    }
    if let Some(byte) = first_byte {
        if shell.accept(byte).is_err() {
            exit(3);
        }
    }
    loop {
        match read_one() {
            Ok(Some(byte)) => {
                if shell.accept(byte).is_err() {
                    exit(3);
                }
            }
            Ok(None) => {
                if wait_for_input().is_err() {
                    exit(4);
                }
            }
            Err(()) => exit(4),
        }
    }
}
