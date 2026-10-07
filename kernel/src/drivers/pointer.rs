//! QEMU VirtIO absolute tablet: report complete pointer state at SYN_REPORT.
use super::virtio_input::VirtioInput;
use cantaya_shared::desktop::{DesktopEvent, POINTER};
use spin::Mutex;

struct Pointer {
    device: VirtioInput,
    x: u32,
    y: u32,
    buttons: u32,
    changed: bool,
}
static POINTER_DEVICE: Mutex<Option<Pointer>> = Mutex::new(None);

pub fn init() {
    match VirtioInput::new(true) {
        Ok(device) => {
            let irq = device.irq();
            log::info!("VirtIO pointer: absolute input ready at {:#x}", device.base);
            *POINTER_DEVICE.lock() = Some(Pointer {
                device,
                x: 16384,
                y: 16384,
                buttons: 0,
                changed: false,
            });
            crate::hal::gic::register_handler(irq, on_interrupt);
        }
        Err(error) => log::warn!("VirtIO pointer unavailable: {:?}", error),
    }
}
impl Pointer {
    fn drain(&mut self) {
        for _ in 0..self.device.queue_size {
            let Some(event) = self.device.poll() else {
                break;
            };
            let kind = u16::from_le(event.event_type);
            let code = u16::from_le(event.code);
            let value = u32::from_le(event.value);
            match (kind, code) {
                (3, 0) => {
                    self.x = value.min(32767);
                    self.changed = true;
                }
                (3, 1) => {
                    self.y = value.min(32767);
                    self.changed = true;
                }
                (1, 0x110..=0x112) => {
                    let mask = 1 << (code - 0x110);
                    if value == 0 {
                        self.buttons &= !mask;
                    } else {
                        self.buttons |= mask;
                    }
                    self.changed = true;
                }
                (0, 0) if self.changed => {
                    crate::desktop::queue_event(DesktopEvent {
                        kind: POINTER,
                        code: self.buttons,
                        value: self.x as i32,
                        text: self.y,
                    });
                    self.changed = false;
                }
                _ => {}
            }
        }
    }
}
fn on_interrupt() {
    let irq = crate::executive::ke::spinlock::IrqState::disable();
    if let Some(pointer) = POINTER_DEVICE.lock().as_mut() {
        pointer.drain();
        pointer.device.acknowledge();
        pointer.drain();
    }
    crate::console::wake_ready_waiter();
    irq.restore();
}
