//! Runtime probe for the Vulkan loader used by Whisper's Vulkan backend.
//!
//! Windows MSVC builds with the `vulkan` feature delay-load `vulkan-1.dll`
//! (see `build.rs`). The app therefore starts on PCs without the Vulkan
//! loader, but the first Vulkan call would raise a delay-load exception and
//! terminate the process. whisper.cpp touches Vulkan from every context load
//! (its backend registry enumerates Vulkan devices even for CPU-only
//! contexts), so Whisper must not be initialized at all when the loader is
//! missing. Every native Whisper entry point calls [`ensure_whisper_runtime`].

use std::sync::OnceLock;

use super::acceleration::WhisperCompiledBackend;

/// User-facing explanation when Whisper cannot run in this build.
pub const VULKAN_RUNTIME_MISSING_MESSAGE: &str =
    "Whisper in this GPU build needs the Vulkan runtime, which isn't installed on this PC. \
Install or update your graphics driver, or install the official Vulkan runtime, then restart \
ClawScribe. Parakeet transcription works without it.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VulkanRuntime {
    /// This build does not link the Vulkan backend.
    NotRequired,
    /// The Vulkan loader was found and stays loaded for the process lifetime.
    Available,
    /// The build needs the loader, but it could not be loaded.
    Missing,
}

/// Probe result, computed once per process.
pub fn vulkan_runtime() -> VulkanRuntime {
    static PROBE: OnceLock<VulkanRuntime> = OnceLock::new();
    *PROBE.get_or_init(|| {
        classify(
            WhisperCompiledBackend::current() == WhisperCompiledBackend::Vulkan,
            || {
                let available = probe_loader();
                if !available {
                    log::warn!(
                        "Vulkan loader (vulkan-1.dll) not found; Whisper is unavailable in this build"
                    );
                }
                available
            },
        )
    })
}

fn classify(requires_vulkan: bool, probe: impl FnOnce() -> bool) -> VulkanRuntime {
    if !requires_vulkan {
        VulkanRuntime::NotRequired
    } else if probe() {
        VulkanRuntime::Available
    } else {
        VulkanRuntime::Missing
    }
}

/// Fails closed before any whisper.cpp call when the Vulkan loader is absent.
pub fn ensure_whisper_runtime() -> Result<(), String> {
    runtime_result(vulkan_runtime())
}

fn runtime_result(runtime: VulkanRuntime) -> Result<(), String> {
    match runtime {
        VulkanRuntime::Missing => Err(VULKAN_RUNTIME_MISSING_MESSAGE.to_string()),
        VulkanRuntime::NotRequired | VulkanRuntime::Available => Ok(()),
    }
}

#[cfg(all(windows, feature = "vulkan"))]
fn probe_loader() -> bool {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_FLAGS};

    // Same default search order the MSVC delay-load helper uses. The handle is
    // intentionally never freed so the delay-load helper reuses this module.
    // SAFETY: the argument is a static NUL-terminated wide string.
    unsafe { LoadLibraryExW(w!("vulkan-1.dll"), None, LOAD_LIBRARY_FLAGS(0)) }.is_ok()
}

#[cfg(not(all(windows, feature = "vulkan")))]
fn probe_loader() -> bool {
    // Non-Windows Vulkan builds keep a normal link-time dependency: the
    // dynamic loader has already resolved it if the process is running.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_without_vulkan_never_probe() {
        let runtime = classify(false, || panic!("must not probe"));
        assert_eq!(runtime, VulkanRuntime::NotRequired);
        assert!(runtime_result(runtime).is_ok());
    }

    #[test]
    fn missing_loader_fails_closed_with_guidance() {
        let runtime = classify(true, || false);
        assert_eq!(runtime, VulkanRuntime::Missing);
        let err = runtime_result(runtime).unwrap_err();
        assert!(err.contains("Vulkan runtime"));
        assert!(err.contains("Parakeet"));
    }

    #[test]
    fn available_loader_allows_whisper() {
        let runtime = classify(true, || true);
        assert_eq!(runtime, VulkanRuntime::Available);
        assert!(runtime_result(runtime).is_ok());
    }
}
