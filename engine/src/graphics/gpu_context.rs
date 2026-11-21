use std::sync::Arc;
use vulkanalia::prelude::v1_0::*;
use vulkanalia::vk::{ExtDebugUtilsExtensionInstanceCommands, KhrSurfaceExtensionInstanceCommands};

pub(crate) struct GpuContextInner {
    _entry: Entry,
    pub instance: Instance,
    pub device: Device,
    pub physical_device: vk::PhysicalDevice,
    pub(crate) queue: vk::Queue,
    pub(crate) queue_family: u32,
    surface: vk::SurfaceKHR,
    debug_messenger: vk::DebugUtilsMessengerEXT,
}

impl Drop for GpuContextInner {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.instance.destroy_surface_khr(self.surface, None);
            if !self.debug_messenger.is_null() {
                self.instance
                    .destroy_debug_utils_messenger_ext(self.debug_messenger, None);
            }
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

#[derive(Clone)]
pub struct GpuContext(pub(crate) Arc<GpuContextInner>);

impl GpuContext {
    pub(crate) fn new(
        entry: Entry,
        instance: Instance,
        device: Device,
        physical_device: vk::PhysicalDevice,
        queue: vk::Queue,
        queue_family: u32,
        surface: vk::SurfaceKHR,
        debug_messenger: vk::DebugUtilsMessengerEXT,
    ) -> Self {
        Self(Arc::new(GpuContextInner {
            _entry: entry,
            instance,
            device,
            physical_device,
            queue,
            queue_family,
            surface,
            debug_messenger,
        }))
    }

    pub(crate) fn device(&self) -> &Device {
        &self.0.device
    }

    pub(crate) fn physical_device(&self) -> vk::PhysicalDevice {
        self.0.physical_device
    }

    pub(crate) fn queue(&self) -> vk::Queue {
        self.0.queue
    }

    pub(crate) fn queue_family(&self) -> u32 {
        self.0.queue_family
    }
}
