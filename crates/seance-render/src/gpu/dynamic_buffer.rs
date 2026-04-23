use wgpu::util::DeviceExt;
use wgpu::*;

/// A GPU buffer that grows on demand when the upload doesn't fit.
pub(super) struct DynamicBuffer {
    pub(super) buffer: Option<Buffer>,
    pub(super) bind_group: Option<BindGroup>,
    usage: BufferUsages,
    label: &'static str,
}

impl DynamicBuffer {
    pub(super) fn new(usage: BufferUsages, label: &'static str) -> Self {
        Self {
            buffer: None,
            bind_group: None,
            usage,
            label,
        }
    }

    /// Upload `data`, growing the buffer if needed. Returns whether a
    /// new buffer was allocated (callers must rebuild any bind group).
    pub(super) fn upload(&mut self, device: &Device, queue: &Queue, data: &[u8]) -> bool {
        let needs_new = self
            .buffer
            .as_ref()
            .is_none_or(|b| b.size() < data.len() as u64);
        if needs_new {
            self.buffer = Some(device.create_buffer_init(&util::BufferInitDescriptor {
                label: Some(self.label),
                contents: data,
                usage: self.usage,
            }));
            self.bind_group = None;
            true
        } else {
            queue.write_buffer(self.buffer.as_ref().unwrap(), 0, data);
            false
        }
    }
}
