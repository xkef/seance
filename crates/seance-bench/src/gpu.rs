//! Headless wgpu instance for GPU-side submit timing.
//!
//! No surface, no swapchain — the bench harness is never on-screen.
//! Pipelines added later (once the renderer can run without a surface)
//! will share this device.

use std::time::{Duration, Instant};

use wgpu::{
    Backends, CommandEncoderDescriptor, Device, DeviceDescriptor, Instance, InstanceDescriptor,
    MemoryHints, PollType, PowerPreference, Queue, RequestAdapterOptions,
};

pub struct HeadlessGpu {
    pub adapter_name: String,
    device: Device,
    queue: Queue,
}

impl HeadlessGpu {
    pub async fn new() -> Option<Self> {
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::PRIMARY,
            ..InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok()?;
        let info = adapter.get_info();
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("seance-bench"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .ok()?;
        Some(Self {
            adapter_name: format!("{} ({:?})", info.name, info.backend),
            device,
            queue,
        })
    }

    /// Submit an empty command buffer and wait for the queue to drain.
    ///
    /// Proxies "time to round-trip the GPU from the submit thread" — useful
    /// as a baseline for M2 work that will layer cell uploads on top.
    pub fn submit_noop(&self) -> Duration {
        let encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor { label: None });
        let t0 = Instant::now();
        self.queue.submit(Some(encoder.finish()));
        let _ = self.device.poll(PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        t0.elapsed()
    }
}
