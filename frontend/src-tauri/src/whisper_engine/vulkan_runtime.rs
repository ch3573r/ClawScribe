//! Runtime probe for the Vulkan backend used by Whisper.
//!
//! Windows MSVC builds with the `vulkan` feature delay-load `vulkan-1.dll`
//! (see `build.rs`). The app therefore starts on PCs without the Vulkan
//! loader, but the first Vulkan call would raise a delay-load exception and
//! terminate the process. whisper.cpp touches Vulkan from every context load
//! (its backend registry enumerates Vulkan devices even for CPU-only
//! contexts), so Whisper must not be initialized at all when the loader is
//! missing or ggml cannot enumerate devices. Every native Whisper entry point
//! calls [`ensure_whisper_runtime`].

#[cfg(all(windows, feature = "vulkan"))]
use std::sync::OnceLock;

/// User-facing explanation when Whisper cannot run in this build.
pub const VULKAN_RUNTIME_MISSING_MESSAGE: &str =
    "Whisper in this GPU build needs a working Vulkan runtime and a compatible device. \
Install or update your graphics driver, then restart ClawScribe. \
Parakeet transcription works without Vulkan.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VulkanRuntime {
    /// This build does not link the Vulkan backend.
    NotRequired,
    /// The Vulkan runtime passed its preflight.
    Available,
    /// The build needs Vulkan, but the runtime could not be used.
    Missing,
}

/// On Windows, the first check can wait for a driver running in a child.
pub async fn vulkan_runtime() -> VulkanRuntime {
    #[cfg(all(windows, feature = "vulkan"))]
    {
        static PROBE: OnceLock<VulkanRuntime> = OnceLock::new();
        if let Some(result) = PROBE.get() {
            return *result;
        }
        tokio::task::spawn_blocking(|| {
            *PROBE.get_or_init(|| {
                classify(true, || {
                    if !crate::parakeet_engine::worker::vulkan_device_available() {
                        log::warn!("Vulkan preflight failed; Whisper is unavailable");
                        return false;
                    }
                    let Some(loader) = load_compatible_loader() else {
                        log::warn!(
                            "System Vulkan loader could not be loaded; Whisper is unavailable"
                        );
                        return false;
                    };
                    // MSVC's delayhlp.cpp calls LoadLibraryEx with flags 0 and the
                    // bare DLL name. Windows consults loaded modules by base name
                    // before searching directories, so retain the System32 module
                    // for the lifetime of this process.
                    std::mem::forget(loader);
                    true
                })
            })
        })
        .await
        .unwrap_or(VulkanRuntime::Missing)
    }
    #[cfg(not(all(windows, feature = "vulkan")))]
    {
        // Other Vulkan builds resolve the loader at link time.
        classify(cfg!(feature = "vulkan"), || true)
    }
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

/// Fails closed before native Whisper initialization if the preflight failed.
pub async fn ensure_whisper_runtime() -> Result<(), String> {
    runtime_result(vulkan_runtime().await)
}

fn runtime_result(runtime: VulkanRuntime) -> Result<(), String> {
    match runtime {
        VulkanRuntime::Missing => Err(VULKAN_RUNTIME_MISSING_MESSAGE.to_string()),
        VulkanRuntime::NotRequired | VulkanRuntime::Available => Ok(()),
    }
}

/// ggml-vulkan creates its instance with Vulkan API 1.2.
#[cfg(any(test, all(windows, feature = "vulkan")))]
const REQUIRED_API_VERSION: u32 = (1 << 22) | (2 << 12);

/// A loader older than 1.1 lacks exports that ggml-vulkan imports; the
/// delay-load helper would then fail at the first such call.
#[cfg(any(test, all(windows, feature = "vulkan")))]
fn loader_supports(api_version: Option<u32>) -> bool {
    api_version.is_some_and(|version| version >= REQUIRED_API_VERSION)
}

#[cfg(all(windows, feature = "vulkan"))]
struct VulkanLoader(windows::Win32::Foundation::HMODULE);

#[cfg(all(windows, feature = "vulkan"))]
impl Drop for VulkanLoader {
    fn drop(&mut self) {
        // SAFETY: this guard owns one reference obtained with LoadLibraryExW.
        let _ = unsafe { windows::Win32::Foundation::FreeLibrary(self.0) };
    }
}

#[cfg(all(windows, feature = "vulkan"))]
fn load_compatible_loader() -> Option<VulkanLoader> {
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    // SAFETY: the argument is a static NUL-terminated wide string.
    let loader = VulkanLoader(
        unsafe { LoadLibraryExW(w!("vulkan-1.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.ok()?,
    );
    // SAFETY: the guard owns a live module and the export names are static.
    let enumerate = unsafe { GetProcAddress(loader.0, s!("vkEnumerateInstanceVersion")) }?;
    unsafe { GetProcAddress(loader.0, s!("vkGetPhysicalDeviceFeatures2")) }?;
    type EnumerateInstanceVersion = unsafe extern "system" fn(*mut u32) -> i32;
    // SAFETY: the export has the documented vkEnumerateInstanceVersion
    // signature; it writes one u32 and returns a VkResult (0 = VK_SUCCESS).
    let enumerate: EnumerateInstanceVersion = unsafe { std::mem::transmute(enumerate) };
    let mut version = 0u32;
    let result = unsafe { enumerate(&mut version) };
    loader_supports((result == 0).then_some(version)).then_some(loader)
}

#[cfg(all(windows, feature = "vulkan"))]
pub(crate) fn probe_device_in_child() -> bool {
    extern "C" {
        // ggml-vulkan.h from the same whisper-rs-sys static library that
        // Whisper uses. This enters the actual backend's device enumeration.
        fn ggml_backend_vk_get_device_count() -> std::ffi::c_int;
    }
    let Some(_loader) = load_compatible_loader() else {
        return false;
    };
    // SAFETY: the loader is retained through the call, which runs only in the
    // supervised child. The delay-load table keeps its own reference.
    unsafe { ggml_backend_vk_get_device_count() > 0 }
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
