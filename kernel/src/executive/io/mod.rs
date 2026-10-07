//! Io — I/O Manager.
//!
//! NT-style I/O model:
//!   IRP (I/O Request Packet) → DriverObject dispatch table → DeviceObject stack
//!
//! Sub-modules:
//!   irp    — IRP structure
//!   driver — DriverObject + DeviceObject

use alloc::vec::Vec;

use crate::{
    drivers::{
        data_fat::DataFatVolume,
        fat::{BlockDevice, LiveFatVolume},
        virtio_blk::{DATA_DISK, MAX_DATA_VOLUME_SECTORS},
    },
};

pub mod driver;
pub mod image;
pub mod irp;
pub mod root;

/// One-time initialisation.
pub fn init(
    storage_create_probe: bool,
    storage_verify_probe: bool,
    storage_failure_probe: bool,
    storage_corruption_probe: bool,
    storage_capacity_probe: bool,
    storage_interrupt_create_probe: Option<u8>,
    storage_interrupt_verify_probe: Option<u8>,
) {
    let data_volume = match DataFatVolume::open_bounded(&DATA_DISK, MAX_DATA_VOLUME_SECTORS) {
        Ok(volume) => volume,
        Err(error) => {
            log::warn!(
                "Io: data volume unavailable or invalid ({:?}); continuing with read-only boot volume",
                error,
            );
            log::debug!("Io: I/O Manager ready");
            return;
        }
    };
    if let Err(error) = data_volume.recover() {
        log::warn!(
            "Io: data volume recovery failed ({:?}); continuing with read-only boot volume",
            error,
        );
    } else if DATA_DISK.flush() {
        log::info!("Io: data volume FAT32 mount metadata and flush validated");
        if [
            storage_create_probe,
            storage_verify_probe,
            storage_failure_probe,
            storage_corruption_probe,
            storage_capacity_probe,
            storage_interrupt_create_probe.is_some(),
            storage_interrupt_verify_probe.is_some(),
        ]
            .into_iter()
            .filter(|probe| *probe)
            .count()
            > 1
        {
            log::warn!("Io: conflicting data-volume probe flags ignored");
        } else if storage_create_probe {
            run_storage_create_probe(&data_volume);
        } else if storage_verify_probe {
            run_storage_verify_probe(&data_volume);
        } else if storage_failure_probe {
            run_storage_failure_probe(&data_volume);
        } else if storage_corruption_probe {
            run_storage_corruption_probe(&data_volume);
        } else if storage_capacity_probe {
            run_storage_capacity_probe(&data_volume);
        } else if let Some(checkpoint) = storage_interrupt_create_probe {
            run_storage_interrupt_create_probe(&data_volume, checkpoint);
        } else if let Some(checkpoint) = storage_interrupt_verify_probe {
            run_storage_interrupt_verify_probe(&data_volume, checkpoint);
        }
    } else {
        log::warn!("Io: data volume lacks a usable cache flush; writes remain unavailable");
    }
    log::debug!("Io: I/O Manager ready");
}

fn run_storage_create_probe(volume: &DataFatVolume<'_>) {
    const NAME: [u8; 11] = *b"PROBE   TXT";
    const CONTENTS: &[u8] = b"CantayaOS data-volume transaction probe\n";

    volume
        .create_file(&NAME, CONTENTS)
        .expect("data-volume transaction create failed");
    let reader = LiveFatVolume::open_bounded(&DATA_DISK, MAX_DATA_VOLUME_SECTORS)
        .expect("data-volume reader remount failed");
    let mut readback = Vec::new();
    assert!(
        reader.read_file_to_buf_bounded(&NAME, &mut readback, CONTENTS.len()),
        "data-volume readback failed",
    );
    assert_eq!(readback.as_slice(), CONTENTS, "data-volume readback mismatch");
    DataFatVolume::open_bounded(&DATA_DISK, MAX_DATA_VOLUME_SECTORS)
        .and_then(|volume| volume.recover())
        .expect("data-volume recovery remount failed");
    log::info!("Io: data-volume transaction create/readback/recovery validated");
}

fn run_storage_verify_probe(volume: &DataFatVolume<'_>) {
    const NAME: [u8; 11] = *b"PROBE   TXT";
    const CONTENTS: &[u8] = b"CantayaOS data-volume transaction probe\n";

    volume
        .recover()
        .expect("data-volume reboot recovery failed");
    let reader = LiveFatVolume::open_bounded(&DATA_DISK, MAX_DATA_VOLUME_SECTORS)
        .expect("data-volume reboot reader remount failed");
    let mut readback = Vec::new();
    assert!(
        reader.read_file_to_buf_bounded(&NAME, &mut readback, CONTENTS.len()),
        "data-volume reboot readback failed",
    );
    assert_eq!(
        readback.as_slice(),
        CONTENTS,
        "data-volume reboot readback mismatch",
    );
    log::info!("Io: data-volume transaction reboot persistence validated");
}

fn run_storage_failure_probe(volume: &DataFatVolume<'_>) {
    volume
        .probe_failure_recovery()
        .expect("data-volume transaction failure recovery failed");
    log::info!("Io: data-volume transaction failure recovery validated");
}

fn run_storage_corruption_probe(volume: &DataFatVolume<'_>) {
    volume
        .probe_corruption_rejection()
        .expect("data-volume corruption rejection failed");
    log::info!("Io: data-volume corruption rejection validated");
}

fn run_storage_capacity_probe(volume: &DataFatVolume<'_>) {
    volume
        .probe_capacity_rejection()
        .expect("data-volume capacity rejection failed");
    log::info!("Io: data-volume capacity and directory exhaustion validated");
}

fn run_storage_interrupt_create_probe(volume: &DataFatVolume<'_>, checkpoint: u8) {
    const CONTENTS: &[u8] = b"CantayaOS data-volume interruption probe\n";
    let name = interrupted_probe_name(checkpoint);

    volume
        .create_file_interrupted(&name, CONTENTS, checkpoint, stop_at_storage_checkpoint)
        .expect("data-volume interruption checkpoint was not reached");
    panic!("data-volume interruption probe returned without stopping");
}

fn run_storage_interrupt_verify_probe(volume: &DataFatVolume<'_>, checkpoint: u8) {
    const CONTENTS: &[u8] = b"CantayaOS data-volume interruption probe\n";
    let name = interrupted_probe_name(checkpoint);

    volume
        .recover()
        .expect("data-volume interruption recovery failed");
    volume
        .validate_fat_copies()
        .expect("data-volume FAT copies diverged after interruption recovery");
    let reader = LiveFatVolume::open_bounded(&DATA_DISK, MAX_DATA_VOLUME_SECTORS)
        .expect("data-volume interruption reader remount failed");
    let mut readback = Vec::new();
    let found = reader.read_file_to_buf_bounded(&name, &mut readback, CONTENTS.len());
    if checkpoint == 8 {
        assert!(found, "committed interrupted transaction disappeared");
        assert_eq!(
            readback.as_slice(),
            CONTENTS,
            "committed interrupted transaction contents changed",
        );
    } else {
        assert!(!found, "uncommitted interrupted transaction was published");
    }
    log::info!(
        "Io: data-volume transaction interruption recovery validated at checkpoint {}",
        checkpoint,
    );
}

fn interrupted_probe_name(checkpoint: u8) -> [u8; 11] {
    let mut name = *b"CRASH000TXT";
    name[7] = b'0' + checkpoint;
    name
}

fn stop_at_storage_checkpoint(checkpoint: u8) {
    log::info!(
        "Io: data-volume transaction interruption checkpoint {} durable",
        checkpoint,
    );
    loop {
        core::hint::spin_loop();
    }
}

/// Route an IRP to the top of a device stack.
///
/// # Safety
///
/// `device` and `irp` must remain valid for the complete synchronous dispatch.
pub unsafe fn io_call_driver(device: *mut driver::DeviceObject, irp: *mut irp::Irp) {
    let Some(request) = (unsafe { irp.as_mut() }) else {
        return;
    };
    let Some(dev) = (unsafe { device.as_ref() }) else {
        request.complete(irp::STATUS_INVALID_PARAMETER);
        return;
    };
    let Some(driver) = (unsafe { dev.driver.as_ref() }) else {
        request.complete(irp::STATUS_INVALID_PARAMETER);
        return;
    };
    let Some(location) = request.current_location() else {
        request.complete(irp::STATUS_INVALID_PARAMETER);
        return;
    };
    let function = irp::IrpMajorFunction::from_u8(location.major_function);
    if function == irp::IrpMajorFunction::Unknown {
        request.complete(irp::STATUS_NOT_SUPPORTED);
        return;
    }
    if let Some(dispatch) = driver.dispatch_table[function as usize] {
        dispatch(device, irp);
    } else {
        request.complete(irp::STATUS_NOT_SUPPORTED);
    }
}
