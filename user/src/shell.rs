//! Interactive EL0 shell over the bounded console pseudo-handles.

use core::{
    arch::asm,
    fmt::{self, Write},
};

const MAX_LINE: usize = 128;
const PROMPT: &str = "cantaya> ";
const STATUS_TIMEOUT: u64 = 0x102;

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

fn output(args: fmt::Arguments<'_>) -> Result<(), ()> {
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

fn wait_for_input() -> Result<(), ()> {
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

fn system_value(class: u64) -> Result<u64, ()> {
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

struct Shell {
    line: [u8; MAX_LINE],
    len: usize,
    after_cr: bool,
}

impl Shell {
    fn new() -> Self {
        Self {
            line: [0; MAX_LINE],
            len: 0,
            after_cr: false,
        }
    }

    fn accept(&mut self, byte: u8) -> Result<(), ()> {
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

    fn run_command(&self) -> Result<(), ()> {
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
            "echo" => output(format_args!("\n")),
            _ => {
                if let Some(text) = command.strip_prefix("echo ") {
                    output(format_args!("{text}\n"))
                } else {
                    output(format_args!("Unknown command: {command}\n"))
                }
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn el0_shell_main(_stack_top: u64, _mode: u64) -> ! {
    let first_byte = match read_one() {
        Ok(value) => value,
        Err(()) => exit(1),
    };
    if output(format_args!(
        "[user-shell] EL0 command loop ready\n\nCantayaOS terminal. Type 'help' for commands.\n{PROMPT}"
    )).is_err() {
        exit(2);
    }

    let mut shell = Shell::new();
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
