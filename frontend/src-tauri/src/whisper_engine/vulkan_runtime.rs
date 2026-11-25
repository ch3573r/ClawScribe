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
        // Match build.rs exactly: every `vulkan` build delay-loads the loader,
        // whichever backend a combined feature set reports first.
        classify(cfg!(feature = "vulkan"), || {
            let available = probe_loader();
            if !available {
                log::warn!(
                    "Vulkan loader (vulkan-1.dll) not found; Whisper is unavailable in this build"
                );
            }
            available
        })
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

/// ggml-vulkan creates its instance with Vulkan API 1.2.
const REQUIRED_API_VERSION: u32 = (1 << 22) | (2 << 12);

/// A loader older than 1.1 lacks exports that ggml-vulkan imports; the
/// delay-load helper would then fail at the first such call.
fn loader_supports(api_version: Option<u32>) -> bool {
    api_version.is_some_and(|version| version >= REQUIRED_API_VERSION)
}

#[cfg(all(windows, feature = "vulkan"))]
fn probe_loader() -> bool {
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_FLAGS,
    };

    // Same default search order the MSVC delay-load helper uses. The handle is
    // intentionally never freed so the delay-load helper reuses this module.
    // SAFETY: the argument is a static NUL-terminated wide string.
    let Ok(module) = (unsafe { LoadLibraryExW(w!("vulkan-1.dll"), None, LOAD_LIBRARY_FLAGS(0)) })
    else {
        return false;
    };
    // SAFETY: valid module handle and static NUL-terminated export names.
    let enumerate = unsafe { GetProcAddress(module, s!("vkEnumerateInstanceVersion")) };
    let features2 = unsafe { GetProcAddress(module, s!("vkGetPhysicalDeviceFeatures2")) };
    let (Some(enumerate), Some(_)) = (enumerate, features2) else {
        return false;
    };
    type EnumerateInstanceVersion = unsafe extern "system" fn(*mut u32) -> i32;
    // SAFETY: the export has the documented vkEnumerateInstanceVersion
    // signature; it writes one u32 and returns a VkResult (0 = VK_SUCCESS).
    let enumerate: EnumerateInstanceVersion = unsafe { std::mem::transmute(enumerate) };
    let mut version = 0u32;
    let result = unsafe { enumerate(&mut version) };
    loader_supports((result == 0).then_some(version))
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
    fn loaders_older_than_vulkan_1_2_are_rejected() {
        assert!(!loader_supports(None));
        assert!(!loader_supports(Some(1 << 22)));
        assert!(!loader_supports(Some((1 << 22) | (1 << 12) | 200)));
        assert!(loader_supports(Some(REQUIRED_API_VERSION)));
        assert!(loader_supports(Some((1 << 22) | (3 << 12) | 250)));
    }

    #[test]
    fn available_loader_allows_whisper() {
        let runtime = classify(true, || true);
        assert_eq!(runtime, VulkanRuntime::Available);
        assert!(runtime_result(runtime).is_ok());
    }
}
