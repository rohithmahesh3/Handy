use ort::execution_providers::{
    CPUExecutionProvider, CUDAExecutionProvider, ExecutionProvider, ExecutionProviderDispatch,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnnxExecutionDevice {
    #[default]
    Auto,
    Cpu,
    Gpu,
}

#[derive(Debug, Clone)]
pub struct OnnxExecutionParams {
    pub device: OnnxExecutionDevice,
    pub gpu_device_id: i32,
    pub allow_cpu_fallback: bool,
}

impl Default for OnnxExecutionParams {
    fn default() -> Self {
        Self {
            device: OnnxExecutionDevice::Auto,
            gpu_device_id: 0,
            allow_cpu_fallback: true,
        }
    }
}

impl OnnxExecutionParams {
    pub fn cpu() -> Self {
        Self {
            device: OnnxExecutionDevice::Cpu,
            ..Self::default()
        }
    }

    pub fn gpu(gpu_device_id: i32) -> Self {
        Self {
            device: OnnxExecutionDevice::Gpu,
            gpu_device_id,
            allow_cpu_fallback: true,
        }
    }

    pub fn auto() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OnnxExecutionCapabilities {
    pub cuda_available: bool,
}

pub fn detect_onnx_execution_capabilities() -> OnnxExecutionCapabilities {
    let cuda_available = CUDAExecutionProvider::default()
        .is_available()
        .unwrap_or(false);
    OnnxExecutionCapabilities { cuda_available }
}

#[derive(Debug, Clone)]
pub struct OnnxExecutionResolution {
    pub providers: Vec<ExecutionProviderDispatch>,
    pub effective_device: OnnxExecutionDevice,
    pub reason: String,
    pub capabilities: OnnxExecutionCapabilities,
}

pub fn resolve_onnx_execution(params: &OnnxExecutionParams) -> OnnxExecutionResolution {
    let caps = detect_onnx_execution_capabilities();
    let cpu_provider = CPUExecutionProvider::default().build();

    match params.device {
        OnnxExecutionDevice::Cpu => OnnxExecutionResolution {
            providers: vec![cpu_provider],
            effective_device: OnnxExecutionDevice::Cpu,
            reason: "CPU mode requested".to_string(),
            capabilities: caps,
        },
        OnnxExecutionDevice::Auto => {
            if caps.cuda_available {
                let mut providers = vec![CUDAExecutionProvider::default()
                    .with_device_id(params.gpu_device_id)
                    .build()
                    .fail_silently()];
                if params.allow_cpu_fallback {
                    providers.push(cpu_provider);
                }
                OnnxExecutionResolution {
                    providers,
                    effective_device: OnnxExecutionDevice::Gpu,
                    reason: format!(
                        "CUDA provider available, preferring GPU device {}",
                        params.gpu_device_id
                    ),
                    capabilities: caps,
                }
            } else {
                OnnxExecutionResolution {
                    providers: vec![cpu_provider],
                    effective_device: OnnxExecutionDevice::Cpu,
                    reason: "CUDA provider unavailable, falling back to CPU".to_string(),
                    capabilities: caps,
                }
            }
        }
        OnnxExecutionDevice::Gpu => {
            if caps.cuda_available {
                let mut providers = vec![CUDAExecutionProvider::default()
                    .with_device_id(params.gpu_device_id)
                    .build()
                    .fail_silently()];
                if params.allow_cpu_fallback {
                    providers.push(cpu_provider);
                }
                OnnxExecutionResolution {
                    providers,
                    effective_device: OnnxExecutionDevice::Gpu,
                    reason: format!(
                        "GPU mode requested, using CUDA device {}",
                        params.gpu_device_id
                    ),
                    capabilities: caps,
                }
            } else {
                OnnxExecutionResolution {
                    providers: vec![cpu_provider],
                    effective_device: OnnxExecutionDevice::Cpu,
                    reason: "GPU mode requested but CUDA provider unavailable; using CPU"
                        .to_string(),
                    capabilities: caps,
                }
            }
        }
    }
}
