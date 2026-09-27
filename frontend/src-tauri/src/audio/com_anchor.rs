/// Keep CPAL's process-wide Windows device enumerator in a live COM apartment.
/// Call before any thread first touches audio devices.
#[cfg(target_os = "windows")]
pub fn ensure_device_enumerator() {
    use cpal::traits::HostTrait;
    use std::sync::{mpsc, Once};
    use std::time::Duration;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    static START: Once = Once::new();
    START.call_once(|| {
        let (ready_tx, ready_rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("audio-com-anchor".into())
            .spawn(move || {
                // This apartment deliberately lives until process termination.
                // CPAL accepts RPC_E_CHANGED_MODE from its own STA initializer
                // and must never lose the apartment owning its static enumerator.
                if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
                    log::warn!("Could not initialize the audio COM anchor");
                    return;
                }
                if let Ok(host) = cpal::host_from_id(cpal::HostId::Wasapi) {
                    let _ = host.output_devices();
                    let _ = host.default_output_device();
                }
                let _ = ready_tx.send(());
                loop {
                    std::thread::park();
                }
            });
        if spawned.is_err() {
            log::warn!("Could not start the audio COM anchor thread");
            return;
        }
        if ready_rx.recv_timeout(Duration::from_secs(5)).is_err() {
            log::warn!("Audio COM anchor was not ready within five seconds");
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn ensure_device_enumerator() {}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::ensure_device_enumerator;
    use cpal::traits::HostTrait;

    #[test]
    fn enumerator_survives_short_lived_device_threads() {
        ensure_device_enumerator();
        for _ in 0..2 {
            std::thread::spawn(|| {
                let host = cpal::host_from_id(cpal::HostId::Wasapi).unwrap();
                // Missing audio devices in a service session are valid.
                let _ = host.output_devices();
                let _ = host.default_output_device();
            })
            .join()
            .expect("Device enumeration thread must finish without crashing");
        }
    }
}
