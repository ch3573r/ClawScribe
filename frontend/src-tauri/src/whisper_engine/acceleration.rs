use crate::audio::{GpuType, PerformanceTier};
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicU8, Ordering};
use whisper_rs::{whisper_rs_sys, WhisperContext, WhisperError, WhisperState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperCompiledBackend {
    Metal,
    Cuda,
    Vulkan,
    HipBlas,
    Cpu,
}

// The pinned whisper-rs-sys bindings expose the backend choice and a subsequent GPU
// initialization failure only through native logs. Observe state creation under
// the native inference permit, ignoring every line unrelated to the decision.
const UNCONFIRMED: u8 = 0;
const VULKAN: u8 = 1;
const CUDA: u8 = 2;
const METAL: u8 = 3;
const FELL_BACK_TO_CPU: u8 = 4;
static OBSERVED_BACKEND: AtomicU8 = AtomicU8::new(UNCONFIRMED);

fn backend_observation(line: &str) -> Option<u8> {
    let message = line.strip_prefix("whisper_backend_init_gpu: ")?;
    match message.trim_end() {
        "using Vulkan backend" => Some(VULKAN),
        "using CUDA backend" => Some(CUDA),
        "using Metal backend" => Some(METAL),
        msg if msg.contains("init() failed") || msg.contains("falling back to CPU") => {
            Some(FELL_BACK_TO_CPU)
        }
        _ => None,
    }
}

unsafe extern "C" fn observe_backend(
    _: whisper_rs_sys::ggml_log_level,
    text: *const c_char,
    _: *mut c_void,
) {
    if !text.is_null() {
        // whisper.cpp supplies a NUL-terminated string for this callback.
        if let Ok(line) = unsafe { CStr::from_ptr(text) }.to_str() {
            if let Some(observation) = backend_observation(line) {
                OBSERVED_BACKEND.store(observation, Ordering::Relaxed);
            }
        }
    }
}

pub(crate) fn create_observed_state(
    ctx: &WhisperContext,
) -> Result<(WhisperState, Option<WhisperCompiledBackend>), WhisperError> {
    let compiled = WhisperCompiledBackend::current();
    if compiled == WhisperCompiledBackend::Cpu {
        return Ok((ctx.create_state()?, Some(compiled)));
    }
    struct RestoreLoggers;
    impl Drop for RestoreLoggers {
        fn drop(&mut self) {
            // whisper_backend_init_gpu also assigns whisper's callback to GGML.
            // Both libraries use their default stderr logger outside this scope.
            unsafe {
                whisper_rs::set_log_callback(None, std::ptr::null_mut());
                whisper_rs_sys::ggml_log_set(None, std::ptr::null_mut());
            }
        }
    }
    OBSERVED_BACKEND.store(UNCONFIRMED, Ordering::Relaxed);
    // The process-wide native permit serializes all Whisper state creation.
    // The callback retains no pointer to stack data, even on an error/unwind.
    unsafe { whisper_rs::set_log_callback(Some(observe_backend), std::ptr::null_mut()) };
    let _restore = RestoreLoggers;
    let state = ctx.create_state()?;
    let backend = match OBSERVED_BACKEND.load(Ordering::Relaxed) {
        VULKAN => Some(WhisperCompiledBackend::Vulkan),
        CUDA if compiled == WhisperCompiledBackend::HipBlas => {
            Some(WhisperCompiledBackend::HipBlas)
        }
        CUDA => Some(WhisperCompiledBackend::Cuda),
        METAL => Some(WhisperCompiledBackend::Metal),
        FELL_BACK_TO_CPU => Some(WhisperCompiledBackend::Cpu),
        _ => None,
    };
    Ok((state, backend))
}

impl WhisperCompiledBackend {
    pub fn current() -> Self {
        if cfg!(feature = "cuda") {
            Self::Cuda
        } else if cfg!(feature = "vulkan") {
            Self::Vulkan
        } else if cfg!(feature = "hipblas") {
            Self::HipBlas
        } else if cfg!(target_os = "macos") || cfg!(feature = "metal") {
            Self::Metal
        } else {
            Self::Cpu
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metal => "Metal",
            Self::Cuda => "Cuda",
            Self::Vulkan => "Vulkan",
            Self::HipBlas => "HipBlas",
            Self::Cpu => "Cpu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperContextAcceleration {
    pub compiled_backend: WhisperCompiledBackend,
    pub runtime_detected_gpu: GpuType,
    pub use_gpu: bool,
    pub flash_attn: bool,
    pub gpu_device: i32,
}

impl WhisperContextAcceleration {
    pub fn status_label(self) -> &'static str {
        match (self.compiled_backend, self.flash_attn) {
            (WhisperCompiledBackend::Metal, true) => "Metal GPU with Flash Attention (Ultra-Fast)",
            (WhisperCompiledBackend::Metal, false) => "Metal GPU acceleration",
            (WhisperCompiledBackend::Cuda, true) => "CUDA GPU with Flash Attention (Ultra-Fast)",
            (WhisperCompiledBackend::Cuda, false) => "CUDA GPU acceleration",
            (WhisperCompiledBackend::Vulkan, _) => "Vulkan GPU acceleration",
            (WhisperCompiledBackend::HipBlas, _) => "HIP BLAS GPU acceleration",
            (WhisperCompiledBackend::Cpu, _) => "CPU processing only",
        }
    }
}

pub fn whisper_context_acceleration_for(
    compiled_backend: WhisperCompiledBackend,
    runtime_detected_gpu: GpuType,
    performance_tier: PerformanceTier,
) -> WhisperContextAcceleration {
    let use_gpu = !matches!(compiled_backend, WhisperCompiledBackend::Cpu);
    let fast_tier = matches!(
        performance_tier,
        PerformanceTier::High | PerformanceTier::Ultra
    );
    let flash_attn = match compiled_backend {
        WhisperCompiledBackend::Metal | WhisperCompiledBackend::Cuda => fast_tier,
        WhisperCompiledBackend::Vulkan
        | WhisperCompiledBackend::HipBlas
        | WhisperCompiledBackend::Cpu => false,
    };

    WhisperContextAcceleration {
        compiled_backend,
        runtime_detected_gpu,
        use_gpu,
        flash_attn: use_gpu && flash_attn,
        gpu_device: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_observation_uses_whisper_runtime_events() {
        assert_eq!(
            backend_observation("whisper_backend_init_gpu: using Vulkan backend\n"),
            Some(VULKAN)
        );
        assert_eq!(
            backend_observation("whisper_backend_init_gpu: ggml_backend_vk_init() failed\n"),
            Some(FELL_BACK_TO_CPU)
        );
        assert_eq!(backend_observation("whisper_backend_init_gpu: Metal GPU does not support family 7 - falling back to CPU"), Some(FELL_BACK_TO_CPU));
        assert_eq!(backend_observation("compiled with Vulkan"), None);
    }

    #[test]
    fn acceleration_vulkan_backend_ignores_runtime_cuda_flash_attention() {
        let params = whisper_context_acceleration_for(
            WhisperCompiledBackend::Vulkan,
            GpuType::Cuda,
            PerformanceTier::High,
        );

        assert_eq!(params.compiled_backend, WhisperCompiledBackend::Vulkan);
        assert_eq!(params.runtime_detected_gpu, GpuType::Cuda);
        assert!(params.use_gpu);
        assert!(!params.flash_attn);
    }

    #[test]
    fn acceleration_vulkan_backend_keeps_gpu_without_runtime_gpu_detection() {
        let params = whisper_context_acceleration_for(
            WhisperCompiledBackend::Vulkan,
            GpuType::None,
            PerformanceTier::Low,
        );

        assert!(params.use_gpu);
        assert!(!params.flash_attn);
    }

    #[test]
    fn acceleration_cuda_backend_enables_flash_attention_for_fast_tiers() {
        let high = whisper_context_acceleration_for(
            WhisperCompiledBackend::Cuda,
            GpuType::Cuda,
            PerformanceTier::High,
        );
        let ultra = whisper_context_acceleration_for(
            WhisperCompiledBackend::Cuda,
            GpuType::Cuda,
            PerformanceTier::Ultra,
        );

        assert!(high.use_gpu);
        assert!(high.flash_attn);
        assert!(ultra.use_gpu);
        assert!(ultra.flash_attn);
    }

    #[test]
    fn acceleration_cpu_backend_disables_gpu_and_flash_attention() {
        for runtime_gpu in [GpuType::None, GpuType::Cuda, GpuType::Vulkan] {
            let params = whisper_context_acceleration_for(
                WhisperCompiledBackend::Cpu,
                runtime_gpu,
                PerformanceTier::Ultra,
            );

            assert!(!params.use_gpu);
            assert!(!params.flash_attn);
        }
    }
}
