
use anyhow::{anyhow, Result};

use winit::window::{Window};
use vulkanalia::Version;

use std::collections::HashSet;
use std::ffi::CStr;
use std::os::raw::c_void;

// Note: This trait was called `ExtDebugUtilsExtension` in versions of `vulkanalia` prior to `v0.31.0`.
use vulkanalia::vk::{ExtDebugUtilsExtensionInstanceCommands, PhysicalDevice, PhysicalDeviceType};

use log::*;
use thiserror::Error;
use vulkanalia::loader::{LibloadingLoader, LIBRARY};
use vulkanalia::window as vk_window;
use vulkanalia::prelude::v1_0::*;

const PORTABILITY_MACOS_VERSION: Version = Version::new(1, 3, 216);
const VALIDATION_ENABLED: bool = cfg!(debug_assertions);
const VALIDATION_LAYER: vk::ExtensionName = vk::ExtensionName::from_bytes(b"VK_LAYER_KHRONOS_validation");



extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    type_: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _: *mut c_void,
) -> vk::Bool32 {
    let data = unsafe { *data };
    let message = unsafe { CStr::from_ptr(data.message) }.to_string_lossy();

    if severity >= vk::DebugUtilsMessageSeverityFlagsEXT::ERROR {
        error!("({:?}) {}", type_, message);
    } else if severity >= vk::DebugUtilsMessageSeverityFlagsEXT::WARNING {
        warn!("({:?}) {}", type_, message);
    } else if severity >= vk::DebugUtilsMessageSeverityFlagsEXT::INFO {
        debug!("({:?}) {}", type_, message);
    } else {
        trace!("({:?}) {}", type_, message);
    }

    vk::FALSE
}
unsafe fn create_instance(window: &Window, entry: &Entry, data: &mut AppData) -> Result<Instance> {
    let application_info = vk::ApplicationInfo::builder()
        .application_name(b"Vulkan Tutorial\0")
        .application_version(vk::make_version(1, 0, 0))
        .engine_name(b"No Engine\0")
        .engine_version(vk::make_version(1, 0, 0))
        .api_version(vk::make_version(1, 0, 0));
    let mut extensions = vk_window::get_required_instance_extensions(window)
        .iter()
        .map(|e| e.as_ptr())
        .collect::<Vec<_>>();
    if VALIDATION_ENABLED {
        extensions.push(vk::EXT_DEBUG_UTILS_EXTENSION.name.as_ptr());
    }

    // Required by Vulkan SDK on macOS since 1.3.216.
    let flags = if cfg!(target_os = "macos") && entry.version()? >= PORTABILITY_MACOS_VERSION {
        info!("Enabling extensions for macOS portability.");
        extensions.push(vk::KHR_GET_PHYSICAL_DEVICE_PROPERTIES2_EXTENSION.name.as_ptr());
        extensions.push(vk::KHR_PORTABILITY_ENUMERATION_EXTENSION.name.as_ptr());
        vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR
    } else {
        vk::InstanceCreateFlags::empty()
    };
    let available_layers = entry
        .enumerate_instance_layer_properties()?
        .iter()
        .map(|l| l.layer_name)
        .collect::<HashSet<_>>();

    if VALIDATION_ENABLED && !available_layers.contains(&VALIDATION_LAYER) {
        return Err(anyhow!("Validation layer requested but not supported."));
    }

    let layers = if VALIDATION_ENABLED {
        vec![VALIDATION_LAYER.as_ptr()]
    } else {
        Vec::new()
    };
    let mut info = vk::InstanceCreateInfo::builder()
        .application_info(&application_info)
        .enabled_layer_names(&layers)
        .enabled_extension_names(&extensions)
        .flags(flags);

    let mut debug_info = vk::DebugUtilsMessengerCreateInfoEXT::builder()
        .message_severity(vk::DebugUtilsMessageSeverityFlagsEXT::all())
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .user_callback(Some(debug_callback));

    if VALIDATION_ENABLED {
        info = info.push_next(&mut debug_info);
    }

    let instance = entry.create_instance(&info, None)?;

    if VALIDATION_ENABLED {
        let debug_info = vk::DebugUtilsMessengerCreateInfoEXT::builder()
            .message_severity(vk::DebugUtilsMessageSeverityFlagsEXT::all())
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                    | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                    | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
            )
            .user_callback(Some(debug_callback));

        data.messenger = instance.create_debug_utils_messenger_ext(&debug_info, None)?;
    }

    Ok(instance)

}
#[derive(Copy, Clone, Debug)]
struct QueueFamilyIndices {
    graphics: u32,
}

impl QueueFamilyIndices {
    unsafe fn get(
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
    ) -> Result<Self> {
        let properties = instance
            .get_physical_device_queue_family_properties(physical_device);

        let graphics = properties
            .iter()
            .position(|p| p.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .map(|i| i as u32);

        if let Some(graphics) = graphics {
            Ok(Self { graphics })
        } else {
            Err(anyhow!(SuitabilityError("Missing required queue families.")))
        }
    }
}
unsafe fn check_physical_device(
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<i32> {
    let properties = instance
        .get_physical_device_properties(physical_device);
    let features = instance
        .get_physical_device_features(physical_device);
    let properties = instance.get_physical_device_properties(physical_device);
    let  score:i32 = match properties.device_type {
        vk::PhysicalDeviceType::DISCRETE_GPU => 10,
        vk::PhysicalDeviceType::CPU => -1,
        vk::PhysicalDeviceType::INTEGRATED_GPU => 5,
        vk::PhysicalDeviceType::VIRTUAL_GPU => 4,
        vk::PhysicalDeviceType::OTHER => -1,
        _ => -1
    };
    QueueFamilyIndices::get(instance, physical_device)?;
    let features = instance.get_physical_device_features(physical_device);
    // if features.geometry_shader != vk::TRUE {
    //     return Err(anyhow!(SuitabilityError("Missing geometry shader support.")));
    // }
    Ok(score)
}

#[derive(Debug, Error)]
#[error("Missing {0}.")]
pub struct SuitabilityError(pub &'static str);

unsafe fn pick_physical_device(instance: &Instance) -> Result<PhysicalDevice> {
    let mut max_score:i32 = -100;
    let mut best_physical_device = None;
    let devs = instance.enumerate_physical_devices()?;
    for physical_device in devs {
        if let Ok(score) = check_physical_device(instance, physical_device) {
            if score > max_score {
                best_physical_device = Some(physical_device);
                max_score = score;
            }
        }
    }
    best_physical_device.ok_or(anyhow!("Failed to find suitable physical device."))
}
/// Our Vulkan app.
#[derive(Clone, Debug)]
pub struct App {
    entry: Entry,
    instance: Instance,
    data: AppData,
}

impl App {
    /// Creates our Vulkan app.
    pub  fn create(window: &Window) -> Result<Self> {
        unsafe {
            let mut data = AppData::default();
            let loader = LibloadingLoader::new(LIBRARY)?;
            let entry = Entry::new(loader).map_err(|b| anyhow!("{}", b))?;
            let instance = create_instance(window, &entry, &mut data)?;
            data.physical_device = pick_physical_device(&instance)?;
            let properties = instance.get_physical_device_properties(data.physical_device);
            info!("Selected physical device (`{}`).", properties.device_name);

            Ok(Self { entry, instance , data})
        }
    }

    /// Renders a frame for our Vulkan app.
    pub fn render(&mut self, window: &Window) -> Result<()> {
        Ok(())
    }

}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            if VALIDATION_ENABLED {
                self.instance.destroy_debug_utils_messenger_ext(self.data.messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// The Vulkan handles and associated properties used by our Vulkan app.
#[derive(Clone, Debug, Default)]
struct AppData {
    messenger: vk::DebugUtilsMessengerEXT,
    physical_device: vk::PhysicalDevice,
}