//! EL1 fallback prompt if the controlled EL0 shell exits or cannot start.

use core::sync::atomic::Ordering;

const MAX_LINE: usize = 128;
const INPUT_BATCH: usize = 32;
const PROMPT: &str = "cantaya> ";

pub struct Shell {
    line: [u8; MAX_LINE],
    len: usize,
    after_cr: bool,
}

impl Shell {
    pub fn start() -> Self {
        crate::hal::framebuffer::show_shell_screen();
        output(format_args!(
            "\nCantayaOS terminal. Type 'help' for commands.\n{PROMPT}"
        ));
        Self {
            line: [0; MAX_LINE],
            len: 0,
            after_cr: false,
        }
    }

    /// Poll a bounded amount of input, then return to the scheduler.
    pub fn poll(&mut self) {
        for _ in 0..INPUT_BATCH {
            let Some(byte) = crate::console::try_read_for_shell() else {
                break;
            };
            self.accept(byte);
        }
    }

    fn accept(&mut self, byte: u8) {
        if byte == b'\n' && self.after_cr {
            self.after_cr = false;
            return;
        }
        self.after_cr = byte == b'\r';

        match byte {
            b'\r' | b'\n' => {
                output(format_args!("\n"));
                self.run_command();
                self.len = 0;
                output(format_args!("{PROMPT}"));
            }
            8 | 127 => {
                if self.len > 0 {
                    self.len -= 1;
                    output(format_args!("\x08 \x08"));
                }
            }
            0x15 => {
                // Ctrl-U clears the current input without running it.
                while self.len > 0 {
                    self.len -= 1;
                    output(format_args!("\x08 \x08"));
                }
            }
            0x0c => {
                // Ctrl-L clears the screen and restores the unfinished line.
                let line = core::str::from_utf8(&self.line[..self.len]).unwrap_or("");
                clear_output();
                output(format_args!("{PROMPT}{line}"));
            }
            0x20..=0x7e => {
                if self.len < MAX_LINE {
                    self.line[self.len] = byte;
                    self.len += 1;
                    output(format_args!("{}", byte as char));
                } else {
                    output(format_args!("\x07"));
                }
            }
            _ => {}
        }
    }

    fn run_command(&self) {
        // Only printable ASCII is stored in the line buffer.
        let command = core::str::from_utf8(&self.line[..self.len])
            .expect("terminal accepted a non-ASCII byte")
            .trim();
        match command {
            "" => {}
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
                let ticks = crate::executive::ps::scheduler::TICK_COUNT.load(Ordering::Relaxed);
                output(format_args!(
                    "Uptime: {}.{:02} seconds\n",
                    ticks / 100,
                    ticks % 100
                ));
            }
            "mem" => {
                let pages = crate::executive::mm::phys::free_pages();
                output(format_args!(
                    "Free physical memory: {} MiB ({} pages)\n",
                    pages / 256,
                    pages
                ));
            }
            "clear" => clear_output(),
            "echo" => output(format_args!("\n")),
            _ => {
                if let Some(text) = command.strip_prefix("echo ") {
                    output(format_args!("{text}\n"));
                } else {
                    output(format_args!("Unknown command: {command}\n"));
                }
            }
        }
    }
}

fn output(args: core::fmt::Arguments<'_>) {
    crate::console::write(args);
}

fn clear_output() {
    crate::console::clear();
}
