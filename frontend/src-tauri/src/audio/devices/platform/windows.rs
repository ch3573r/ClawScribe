use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use log::{debug, warn};

use crate::audio::devices::configuration::{AudioDevice, DeviceType};

/// Configure Windows audio devices using WASAPI
pub fn configure_windows_audio(host: &cpal::Host) -> Result<Vec<AudioDevice>> {
    let mut devices = Vec::new();

    // Get WASAPI devices
    if let Ok(wasapi_host) = cpal::host_from_id(cpal::HostId::Wasapi) {
        debug!("Using WASAPI host for Windows audio device enumeration");

        // Add output devices (including loopback)
        if let Ok(output_devices) = wasapi_host.output_devices() {
            for device in output_devices {
                if let Ok(name) = device.name() {
                    // For Windows, we need to mark output devices specifically for loopback
                    // info!("Found Windows output device: {}", name);
                    devices.push(AudioDevice::new(name.clone(), DeviceType::Output));
                }
            }
        } else {
            warn!("Failed to enumerate WASAPI output devices");
        }

        // Add input devices from WASAPI
        if let Ok(input_devices) = wasapi_host.input_devices() {
            for device in input_devices {
                if let Ok(name) = device.name() {
                    // info!("Found Windows input device: {}", name);
                    devices.push(AudioDevice::new(name.clone(), DeviceType::Input));
                }
            }
        } else {
            warn!("Failed to enumerate WASAPI input devices");
        }
    } else {
        warn!("Failed to create WASAPI host, falling back to default host");
    }

    // If WASAPI failed or returned no devices, try default host as fallback
    if devices.is_empty() {
        debug!(
            "WASAPI device enumeration failed or returned no devices, falling back to default host"
        );
        // Add regular input devices
        if let Ok(input_devices) = host.input_devices() {
            for device in input_devices {
                if let Ok(name) = device.name() {
                    // info!("Found fallback input device: {}", name);
                    devices.push(AudioDevice::new(name.clone(), DeviceType::Input));
                }
            }
        } else {
            warn!("Failed to enumerate input devices from default host");
        }

        // Add output devices
        if let Ok(output_devices) = host.output_devices() {
            for device in output_devices {
                if let Ok(name) = device.name() {
                    // info!("Found fallback output device: {}", name);
                    devices.push(AudioDevice::new(name.clone(), DeviceType::Output));
                }
            }
        } else {
            warn!("Failed to enumerate output devices from default host");
        }
    }

    // If we still have no devices, add default devices
    if devices.is_empty() {
        warn!("No audio devices found, adding default devices only");

        // Try to add default input device
        if let Some(device) = host.default_input_device() {
            if let Ok(name) = device.name() {
                // info!("Adding default input device: {}", name);
                devices.push(AudioDevice::new(name, DeviceType::Input));
            }
        }

        // Try to add default output device
        if let Some(device) = host.default_output_device() {
            if let Ok(name) = device.name() {
                // info!("Adding default output device: {}", name);
                devices.push(AudioDevice::new(name, DeviceType::Output));
            }
        }
    }

    debug!("Found {} Windows audio devices", devices.len());
    Ok(devices)
}

/// Get Windows device and configuration using WASAPI
pub fn get_windows_device(
    audio_device: &AudioDevice,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let wasapi_host = cpal::host_from_id(cpal::HostId::Wasapi)
        .map_err(|e| anyhow!("Failed to create WASAPI host: {}", e))?;

    // The device type suffix is removed at the IPC boundary. A default choice
    // made by the caller also arrives here as a concrete endpoint name.
    let device = match audio_device.device_type {
        DeviceType::Input => {
            named_endpoint(wasapi_host.input_devices()?, &audio_device.name, |d| {
                d.name().ok()
            })?
        }
        DeviceType::Output => {
            named_endpoint(wasapi_host.output_devices()?, &audio_device.name, |d| {
                d.name().ok()
            })?
        }
    };
    let config = match audio_device.device_type {
        DeviceType::Input => device
            .default_input_config()
            .ok()
            .or_else(|| preferred_config(device.supported_input_configs().ok()?)),
        DeviceType::Output => device
            .supported_output_configs()
            .ok()
            .and_then(preferred_config)
            .or_else(|| device.default_output_config().ok()),
    }
    .ok_or_else(|| anyhow!("The selected audio device has no supported format."))?;
    Ok((device, config))
}

fn named_endpoint<T>(
    endpoints: impl Iterator<Item = T>,
    name: &str,
    endpoint_name: impl Fn(&T) -> Option<String>,
) -> Result<T> {
    let mut matching = endpoints.filter(|device| endpoint_name(device).as_deref() == Some(name));
    let selected = matching
        .next()
        .ok_or_else(|| anyhow!("The selected audio device is no longer available."))?;
    if matching.next().is_some() {
        return Err(anyhow!("More than one audio device has the selected name. Give the devices distinct names in Windows Sound settings."));
    }
    Ok(selected)
}

fn preferred_config(
    configs: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
) -> Option<cpal::SupportedStreamConfig> {
    configs
        .min_by_key(|config| match (config.sample_format(), config.channels()) {
            (cpal::SampleFormat::F32, 2) => 0,
            (cpal::SampleFormat::F32, _) => 1,
            _ => 2,
        })
        .map(|config| config.with_max_sample_rate())
}

#[cfg(test)]
mod tests {
    use super::named_endpoint;

    #[test]
    fn a_named_source_cannot_switch_to_another_endpoint() {
        let names = [
            "Virtual microphone 2",
            "Default input",
            "Virtual microphone",
        ];
        let find =
            |name: &str| named_endpoint(names.into_iter(), name, |value| Some((*value).to_owned()));
        assert_eq!(find("Virtual microphone").unwrap(), "Virtual microphone");
        assert!(find("Virtual").is_err());
        assert!(find("Disconnected microphone").is_err());
        assert!(
            named_endpoint(["Output", "Output"].into_iter(), "Output", |value| Some(
                (*value).to_owned()
            ))
            .is_err()
        );
    }
}
