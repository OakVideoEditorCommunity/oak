// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! M5: importing platform hardware surfaces as wgpu textures.
//!
//! The video decoder produces hardware surfaces (VAAPI/NVDEC/D3D11VA/
//! VideoToolbox). Instead of downloading each frame to system memory and
//! re-uploading it, the surface's platform handle is wrapped into a
//! `wgpu::Texture` on the engine's own device:
//!
//! - **Linux**: the VAAPI surface's DMA-BUF plane (fd/offset/pitch/
//!   modifier) is imported into a raw Vulkan image
//!   (`VK_EXT_external_memory_dma_buf` + `VK_EXT_image_drm_format_modifier`)
//!   and wrapped through `wgpu_hal::vulkan::Device::texture_from_raw`.
//! - **Windows**: the D3D11VA texture's shared NT handle is wrapped via
//!   `wgpu_hal::vulkan::Device::texture_from_d3d11_shared_handle`.
//! - **macOS**: the VideoToolbox CVPixelBuffer's IOSurface is wrapped in a
//!   Metal texture (`newTextureWithDescriptor:iosurface:plane:`) and
//!   handed to `wgpu_hal::metal::Device::texture_from_raw`.
//!
//! Everything here is `unsafe` platform glue; the *only* consumer is the
//! decode import in `oak-codec::gpuinterop`, and any failure must be
//! treated as "fall back to the staging path for this frame" — never as a
//! hard error that poisons later frames.

use std::sync::Arc;

use super::*;

/// A DMA-BUF plane of a hardware frame (Linux/Vulkan import).
///
/// The `fd` is borrowed: the import duplicates it before handing it to
/// Vulkan, so the caller keeps ownership of the original.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug)]
pub struct DmaBufPlane {
	/// DMA-BUF file descriptor.
	pub fd: i32,
	/// Byte offset of the plane inside the buffer object.
	pub offset: u64,
	/// Row pitch in bytes.
	pub pitch: u64,
	/// DRM format modifier (`DRM_FORMAT_MOD_*`).
	pub modifier: u64,
}

/// The Metal pixel format for an IOSurface plane (macOS).
#[cfg(target_os = "macos")]
fn iosurface_pixel_format(format: wgpu::TextureFormat) -> Option<objc2_metal::MTLPixelFormat> {
	use objc2_metal::MTLPixelFormat;
	Some(match format {
		wgpu::TextureFormat::R8Unorm => MTLPixelFormat::R8Unorm,
		wgpu::TextureFormat::Rg8Unorm => MTLPixelFormat::RG8Unorm,
		wgpu::TextureFormat::R16Unorm => MTLPixelFormat::R16Unorm,
		wgpu::TextureFormat::Rg16Unorm => MTLPixelFormat::RG16Unorm,
		_ => return None,
	})
}

/// Registration parameters for one imported texture alias: dimensions,
/// array size, layout, plane aspect and the selected array slice (M5).
#[derive(Clone, Copy)]
struct ImportedTextureDesc {
	width: u32,
	height: u32,
	array_layers: u32,
	format: wgpu::TextureFormat,
	aspect: wgpu::TextureAspect,
	layer: Option<(u32, u32)>,
}

/// Caller-owned resource tied to an imported texture's lifetime (M5):
/// the decoder's hardware `AVFrame` stays alive until the GPU texture is
/// destroyed, so the decoder's surface pool cannot recycle the memory
/// while the GPU still samples it. The closure runs exactly once, right
/// before the platform resources are released.
pub type ImportGuard = Box<dyn FnOnce() + Send + Sync>;

/// The plane formats the import accepts. Kept small on purpose: luma and
/// interleaved chroma for 8-bit NV12 and 16-bit-container P010.
#[cfg(target_os = "linux")]
fn import_vk_format(format: wgpu::TextureFormat) -> Option<ash::vk::Format> {
	use ash::vk::Format;
	Some(match format {
		wgpu::TextureFormat::R8Unorm => Format::R8_UNORM,
		wgpu::TextureFormat::Rg8Unorm => Format::R8G8_UNORM,
		wgpu::TextureFormat::R16Unorm => Format::R16_UNORM,
		wgpu::TextureFormat::Rg16Unorm => Format::R16G16_UNORM,
		_ => return None,
	})
}

impl GpuContext {
	/// Import one DMA-BUF plane as a GPU texture (Linux/Vulkan).
	///
	/// Returns the registry token. `keep_alive` (the decoder's mapped
	/// `AVFrame`) runs when the GPU texture is destroyed, after the GPU is
	/// done with it.
	#[cfg(target_os = "linux")]
	pub fn import_dmabuf_plane(
		&self,
		width: u32,
		height: u32,
		format: wgpu::TextureFormat,
		plane: DmaBufPlane,
		keep_alive: Option<ImportGuard>,
	) -> Result<u64> {
		use ash::vk;
		use wgpu::hal;

		if self.kind != BackendKind::Vulkan || width == 0 || height == 0 {
			return Err(Error::State);
		}
		let Some(vk_format) = import_vk_format(format) else {
			return Err(Error::Invalid);
		};
		let guard = unsafe { self.device.as_hal::<hal::api::Vulkan>() }.ok_or(Error::State)?;
		let hal_device: &hal::vulkan::Device = &guard;
		let raw = hal_device.raw_device().clone();
		let _ = hal_device.raw_physical_device();

		let layouts = [vk::SubresourceLayout {
			offset: plane.offset,
			size: 0,
			row_pitch: plane.pitch,
			array_pitch: 0,
			depth_pitch: 0,
		}];
		let external = vk::ExternalMemoryImageCreateInfo::default()
			.handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
		let mut modifier = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
			.drm_format_modifier(plane.modifier)
			.plane_layouts(&layouts);
		modifier.p_next = (&external as *const vk::ExternalMemoryImageCreateInfo).cast();

		let info = vk::ImageCreateInfo::default()
			.image_type(vk::ImageType::TYPE_2D)
			.format(vk_format)
			.extent(vk::Extent3D {
				width,
				height,
				depth: 1,
			})
			.mip_levels(1)
			.array_layers(1)
			.samples(vk::SampleCountFlags::TYPE_1)
			.tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
			.usage(vk::ImageUsageFlags::SAMPLED)
			.sharing_mode(vk::SharingMode::EXCLUSIVE)
			.initial_layout(vk::ImageLayout::UNDEFINED)
			.push_next(&mut modifier);
		let image = unsafe { raw.create_image(&info, None) }
			.map_err(|e| Error::Failed(format!("vkCreateImage(dmabuf): {e:?}")))?;

		let req = unsafe { raw.get_image_memory_requirements(image) };

		// Duplicate the fd: Vulkan takes ownership of the imported fd on a
		// successful `vkAllocateMemory` and closes it when the memory is
		// freed; the decoder keeps its own.
		let dup_fd = unsafe { libc::dup(plane.fd) };
		if dup_fd < 0 {
			unsafe { raw.destroy_image(image, None) };
			return Err(Error::Failed("dup(dmabuf fd) failed".into()));
		}
		// Pick the memory type by trying the compatible ones in order —
		// `vkGetPhysicalDeviceMemoryProperties` is instance-level and
		// wgpu-hal keeps its raw instance private. The compatible set is
		// usually tiny (often one DEVICE_LOCAL type on discrete GPUs).
		let mut import = vk::ImportMemoryFdInfoKHR::default()
			.handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
			.fd(dup_fd);
		let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
		let mut last_err = None;
		let mut memory = None;
		for type_index in 0..32u32 {
			if req.memory_type_bits & (1 << type_index) == 0 {
				continue;
			}
			let alloc = vk::MemoryAllocateInfo::default()
				.allocation_size(req.size)
				.memory_type_index(type_index)
				.push_next(&mut dedicated)
				.push_next(&mut import);
			match unsafe { raw.allocate_memory(&alloc, None) } {
				Ok(m) => {
					memory = Some(m);
					break;
				}
				Err(e) => last_err = Some((type_index, e)),
			}
		}
		let Some(memory) = memory else {
			unsafe {
				libc::close(dup_fd);
				raw.destroy_image(image, None);
			}
			return Err(Error::Failed(format!(
				"vkAllocateMemory(import) failed for every compatible type: {last_err:?}"
			)));
		};
		if let Err(e) = unsafe { raw.bind_image_memory(image, memory, 0) } {
			unsafe {
				raw.free_memory(memory, None);
				raw.destroy_image(image, None);
			}
			return Err(Error::Failed(format!("vkBindImageMemory: {e:?}")));
		}

		let hal_desc = hal::TextureDescriptor {
			label: Some("oakrender-imported-dmabuf"),
			size: wgpu::Extent3d {
				width,
				height,
				depth_or_array_layers: 1,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format,
			usage: wgpu_types::TextureUses::RESOURCE,
			memory_flags: hal::MemoryFlags::empty(),
			view_formats: Vec::new(),
		};
		// wgpu-hal does not destroy external images (it cannot know how);
		// the drop guard owns both the image and the imported memory, and
		// carries the decoder's frame-keep-alive with it.
		let raw_guard = raw.clone();
		let drop_guard: hal::DropCallback = Box::new(move || unsafe {
			if let Some(keep) = keep_alive {
				keep();
			}
			raw_guard.destroy_image(image, None);
			raw_guard.free_memory(memory, None);
		});
		let hal_texture = unsafe {
			hal_device.texture_from_raw(
				image,
				&hal_desc,
				Some(drop_guard),
				hal::vulkan::TextureMemory::External,
			)
		};
		self.register_imported_texture::<hal::api::Vulkan>(
			hal_texture,
			ImportedTextureDesc {
				width,
				height,
				array_layers: 1,
				format,
				aspect: wgpu::TextureAspect::All,
				layer: None,
			},
		)
	}

	/// Import one slice of a D3D11VA texture array shared as an NT handle
	/// (Windows/Vulkan): one multi-planar image (`NV12`/`P010`) with
	/// `array_size` layers, registered twice (planes 0/1) for `slice`.
	///
	/// D3D11VA frames cycle through the decoder's surface array
	/// (`AVFrame::data[1]` is the slice index), so the imported VkImage
	/// must carry the whole array and the tokens must select their slice —
	/// otherwise only every N-th frame would be importable (M5 audit).
	///
	/// `keep_alive` runs when the image is destroyed; the handle itself is
	/// owned by the caller (Vulkan takes a resource reference, not the
	/// handle).
	#[cfg(target_os = "windows")]
	pub fn import_d3d11_shared_texture(
		&self,
		shared_handle: isize,
		width: u32,
		height: u32,
		format: wgpu::TextureFormat,
		array_size: u32,
		slice: u32,
		keep_alive: Option<ImportGuard>,
	) -> Result<(u64, u64)> {
		use ash::vk;
		use wgpu::hal;

		if self.kind != BackendKind::Vulkan || width == 0 || height == 0 {
			return Err(Error::State);
		}
		let array_size = array_size.max(1);
		if slice >= array_size {
			return Err(Error::Invalid);
		}
		let (vk_format, required_feature) = match format {
			wgpu::TextureFormat::NV12 => (
				vk::Format::G8_B8R8_2PLANE_420_UNORM,
				wgpu::Features::TEXTURE_FORMAT_NV12,
			),
			wgpu::TextureFormat::P010 => (
				vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16,
				wgpu::Features::TEXTURE_FORMAT_P010,
			),
			_ => return Err(Error::Invalid),
		};
		// Multi-planar formats need their device feature; without it the
		// hal texture would be created but every view invalid.
		if !self.device.features().contains(required_feature) {
			return Err(Error::State);
		}
		let guard = unsafe { self.device.as_hal::<hal::api::Vulkan>() }.ok_or(Error::State)?;
		let hal_device: &hal::vulkan::Device = &guard;
		let raw = hal_device.raw_device().clone();

		let mut external = vk::ExternalMemoryImageCreateInfo::default()
			.handle_types(vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE);
		let info = vk::ImageCreateInfo::default()
			.image_type(vk::ImageType::TYPE_2D)
			.format(vk_format)
			.extent(vk::Extent3D {
				width,
				height,
				depth: 1,
			})
			.mip_levels(1)
			.array_layers(array_size)
			.samples(vk::SampleCountFlags::TYPE_1)
			.tiling(vk::ImageTiling::OPTIMAL)
			.usage(vk::ImageUsageFlags::SAMPLED)
			.sharing_mode(vk::SharingMode::EXCLUSIVE)
			.initial_layout(vk::ImageLayout::UNDEFINED)
			.push_next(&mut external);
		let image = unsafe { raw.create_image(&info, None) }
			.map_err(|e| Error::Failed(format!("vkCreateImage(d3d11): {e:?}")))?;

		let req = unsafe { raw.get_image_memory_requirements(image) };
		// Dedicated allocation is required for imported D3D11 textures.
		let mut import = vk::ImportMemoryWin32HandleInfoKHR::default()
			.handle_type(vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE)
			.handle(shared_handle);
		let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
		let mut last_err = None;
		let mut memory = None;
		for type_index in 0..32u32 {
			if req.memory_type_bits & (1 << type_index) == 0 {
				continue;
			}
			let alloc = vk::MemoryAllocateInfo::default()
				.allocation_size(req.size)
				.memory_type_index(type_index)
				.push_next(&mut dedicated)
				.push_next(&mut import);
			match unsafe { raw.allocate_memory(&alloc, None) } {
				Ok(m) => {
					memory = Some(m);
					break;
				}
				Err(e) => last_err = Some((type_index, e)),
			}
		}
		let Some(memory) = memory else {
			unsafe { raw.destroy_image(image, None) };
			return Err(Error::Failed(format!(
				"vkAllocateMemory(import d3d11) failed: {last_err:?}"
			)));
		};
		if let Err(e) = unsafe { raw.bind_image_memory(image, memory, 0) } {
			unsafe {
				raw.free_memory(memory, None);
				raw.destroy_image(image, None);
			}
			return Err(Error::Failed(format!("vkBindImageMemory: {e:?}")));
		}

		let hal_desc = hal::TextureDescriptor {
			label: Some("oakrender-imported-d3d11"),
			size: wgpu::Extent3d {
				width,
				height,
				depth_or_array_layers: array_size,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format,
			usage: wgpu_types::TextureUses::RESOURCE,
			memory_flags: hal::MemoryFlags::empty(),
			view_formats: Vec::new(),
		};
		let raw_guard = raw.clone();
		let drop_guard: hal::DropCallback = Box::new(move || unsafe {
			if let Some(keep) = keep_alive {
				keep();
			}
			raw_guard.destroy_image(image, None);
			raw_guard.free_memory(memory, None);
		});
		let hal_texture = unsafe {
			hal_device.texture_from_raw(
				image,
				&hal_desc,
				Some(drop_guard),
				hal::vulkan::TextureMemory::External,
			)
		};
		// Two plane views of the chosen slice of the multi-planar array:
		// Y at plane 0, interleaved chroma at plane 1.
		let desc = ImportedTextureDesc {
			width,
			height,
			array_layers: array_size,
			format,
			aspect: wgpu::TextureAspect::Plane0,
			layer: Some((slice, 1)),
		};
		let (y, texture) = self.register_imported_texture_shared::<hal::api::Vulkan>(
			hal_texture, desc,
		)?;
		let uv = self.register_texture_entry(
			texture,
			ImportedTextureDesc {
				// The chroma plane's view is half-size; the planar pass
				// derives the sampling scale from the registered sizes.
				width: width.div_ceil(2),
				height: height.div_ceil(2),
				aspect: wgpu::TextureAspect::Plane1,
				..desc
			},
		);
		Ok((y, uv))
	}

	/// Import one IOSurface plane as a Metal-backed texture (macOS): the
	/// VideoToolbox pixel buffer's surface stays owned by the decoder; the
	/// caller keeps it alive via the planar texture's guard.
	///
	/// # Safety
	///
	/// `iosurface` must be a live `IOSurfaceRef` that stays valid until
	/// the imported texture is destroyed (the decoder's frame reference is
	/// held by the planar texture's keep-alive guard).
	#[cfg(target_os = "macos")]
	pub unsafe fn import_iosurface_plane(
		&self,
		iosurface: *mut std::ffi::c_void,
		plane: u32,
		width: u32,
		height: u32,
		format: wgpu::TextureFormat,
	) -> Result<u64> {
		use objc2::rc::Retained;
		use objc2::runtime::ProtocolObject;
		use objc2_io_surface::IOSurfaceRef;
		use objc2_metal::{
			MTLDevice as _, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor,
			MTLTextureType, MTLTextureUsage,
		};
		use wgpu::hal;

		if self.kind != BackendKind::Metal || width == 0 || height == 0 || iosurface.is_null() {
			return Err(Error::State);
		}
		let Some(pixel_format) = iosurface_pixel_format(format) else {
			return Err(Error::Invalid);
		};
		let guard = unsafe { self.device.as_hal::<hal::api::Metal>() }.ok_or(Error::State)?;
		let hal_device: &hal::metal::Device = &guard;
		let device: &Retained<ProtocolObject<dyn objc2_metal::MTLDevice>> =
			hal_device.raw_device();

		let descriptor = MTLTextureDescriptor::new();
		descriptor.setTextureType(MTLTextureType::Type2D);
		descriptor.setPixelFormat(pixel_format);
		descriptor.setStorageMode(MTLStorageMode::Shared);
		descriptor.setUsage(MTLTextureUsage::ShaderRead);
		// SAFETY: plain descriptor field setters.
		unsafe {
			descriptor.setWidth(width as usize);
			descriptor.setHeight(height as usize);
			descriptor.setMipmapLevelCount(1);
		}
		// SAFETY: the caller guarantees a live IOSurface for the texture's
		// lifetime (the planar texture holds its keep-alive guard).
		let surface: &IOSurfaceRef = unsafe { &*(iosurface as *const IOSurfaceRef) };
		// SAFETY: `surface` is a live IOSurface for the texture's lifetime
		// (the caller's planar keep-alive holds the pixel buffer).
		let texture = device
			.newTextureWithDescriptor_iosurface_plane(&descriptor, surface, plane as usize)
			.ok_or_else(|| Error::Failed("newTextureWithDescriptor:iosurface: returned nil".into()))?;

		let raw_type = MTLTextureType::Type2D;
		let hal_texture = unsafe {
			hal::metal::Device::texture_from_raw(
				texture,
				format,
				raw_type,
				1,
				1,
				hal::CopyExtent {
					width,
					height,
					depth: 1,
				},
			)
		};
		self.register_imported_texture::<hal::api::Metal>(
			hal_texture,
			ImportedTextureDesc {
				width,
				height,
				array_layers: 1,
				format,
				aspect: wgpu::TextureAspect::All,
				layer: None,
			},
		)
	}

	/// Register a texture created through `create_texture_from_hal` in the
	/// registry (M5 external imports; also the shared tail of every
	/// platform import).
	fn register_imported_texture<A: wgpu::hal::Api>(
		&self,
		hal_texture: A::Texture,
		desc: ImportedTextureDesc,
	) -> Result<u64> {
		self.register_imported_texture_shared::<A>(hal_texture, desc)
			.map(|(token, _)| token)
	}

	/// The shared registration body; returns the token plus the owning
	/// `wgpu::Texture` so multi-plane/array imports (one texture, several
	/// plane aspects or slices) can register aliases without recreating
	/// the texture.
	fn register_imported_texture_shared<A: wgpu::hal::Api>(
		&self,
		hal_texture: A::Texture,
		desc: ImportedTextureDesc,
	) -> Result<(u64, Arc<wgpu::Texture>)> {
		let texture_desc = wgpu::TextureDescriptor {
			label: Some("oakrender-imported"),
			size: wgpu::Extent3d {
				width: desc.width,
				height: desc.height,
				depth_or_array_layers: desc.array_layers.max(1),
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: desc.format,
			usage: wgpu::TextureUsages::TEXTURE_BINDING,
			view_formats: &[],
		};
		let texture = unsafe {
			self.device
				.create_texture_from_hal::<A>(hal_texture, &texture_desc)
		};
		self.used.store(true, Ordering::Release);
		let texture = Arc::new(texture);
		let token = self.register_texture_entry(texture.clone(), desc);
		Ok((token, texture))
	}

	/// Insert one registry entry for `texture`.
	fn register_texture_entry(&self, texture: Arc<wgpu::Texture>, desc: ImportedTextureDesc) -> u64 {
		let token = self.next_token.fetch_add(1, Ordering::Relaxed);
		lock(&self.textures).insert(
			token,
			GpuTexture {
				texture,
				width: desc.width,
				height: desc.height,
				format: desc.format,
				aspect: desc.aspect,
				layer: desc.layer,
			},
		);
		token
	}

	/// The view for `token`, honoring plane aspects and array slices
	/// (M5 imports).
	pub(crate) fn texture_view(&self, token: u64) -> Result<wgpu::TextureView> {
		let entry = lock(&self.textures)
			.get(&token)
			.cloned()
			.ok_or(Error::NotFound)?;
		let (base_array_layer, array_layer_count) = match entry.layer {
			Some((base, count)) => (base, Some(count)),
			None => (0, None),
		};
		Ok(entry.texture.create_view(&wgpu::TextureViewDescriptor {
			aspect: entry.aspect,
			base_array_layer,
			array_layer_count,
			// wgpu-core derives `D2Array` from the TEXTURE's layer count,
			// not from the view range: an array-imported slice must ask
			// for `D2` explicitly or the planar pass's `D2` layout
			// validation rejects the bind group (M5 desktop review).
			dimension: (array_layer_count == Some(1)).then_some(wgpu::TextureViewDimension::D2),
			..Default::default()
		}))
	}
}

#[cfg(test)]
mod tests {
	//! Unit coverage for the safe parts of the import module. The unsafe
	//! platform bodies are deliberately **not** exercised here: a real
	//! import needs a hardware decoder surface (a VAAPI DMA-BUF, a D3D11VA
	//! shared NT handle, a VideoToolbox IOSurface), which the
	//! software-Vulkan CI runners do not have, and feeding them fake
	//! handles would only assert that a driver rejects garbage.
	//!
	//! What covers the hardware paths instead:
	//! - `import_dmabuf_plane` (Linux): a VAAPI machine running
	//!   `oak-render/tests/footage_import_test.rs` (DMA-BUF + DRM format
	//!   modifier import); the M5 real-hardware acceptance is recorded in
	//!   `docs/zh/plans/render-pipeline-threads-m5-branch-coverage.txt`.
	//! - `import_d3d11_shared_texture` (Windows): the Windows job on a host
	//!   with D3D11VA; a lavapipe-only runner has no shared texture to
	//!   import and the decode side falls back to staging.
	//! - `import_iosurface_plane` (macOS): the macOS job with VideoToolbox
	//!   active.
	//! - `register_imported_texture*`/`texture_view`: the alias registry
	//!   and view helper are unit-tested below on a locally created array
	//!   texture (no platform import); the `Plane0`/`Plane1` aspects only
	//!   become valid with the multi-planar formats of a real import.

	use super::*;
	use crate::backend::gpu_or_skip;
	use crate::error::Error;

	/// The Linux plane format map accepts exactly the luma/chroma planes
	/// the planar import feeds it and rejects everything else.
	#[cfg(target_os = "linux")]
	#[test]
	fn import_vk_format_maps_luma_and_chroma_planes() {
		use ash::vk::Format;
		assert_eq!(
			import_vk_format(wgpu::TextureFormat::R8Unorm),
			Some(Format::R8_UNORM)
		);
		assert_eq!(
			import_vk_format(wgpu::TextureFormat::Rg8Unorm),
			Some(Format::R8G8_UNORM)
		);
		assert_eq!(
			import_vk_format(wgpu::TextureFormat::R16Unorm),
			Some(Format::R16_UNORM)
		);
		assert_eq!(
			import_vk_format(wgpu::TextureFormat::Rg16Unorm),
			Some(Format::R16G16_UNORM)
		);
		for rejected in [
			wgpu::TextureFormat::Rgba8Unorm,
			wgpu::TextureFormat::Rgba32Float,
			wgpu::TextureFormat::NV12,
			wgpu::TextureFormat::P010,
		] {
			assert_eq!(
				import_vk_format(rejected),
				None,
				"{rejected:?} is not a single-plane sample format"
			);
		}
	}

	/// The macOS plane map accepts the interleaved 8/16-bit layouts the
	/// VideoToolbox import feeds and rejects everything else.
	#[cfg(target_os = "macos")]
	#[test]
	fn iosurface_pixel_format_maps_supported_planes() {
		use objc2_metal::MTLPixelFormat;
		assert_eq!(
			iosurface_pixel_format(wgpu::TextureFormat::R8Unorm),
			Some(MTLPixelFormat::R8Unorm)
		);
		assert_eq!(
			iosurface_pixel_format(wgpu::TextureFormat::Rg8Unorm),
			Some(MTLPixelFormat::RG8Unorm)
		);
		assert_eq!(
			iosurface_pixel_format(wgpu::TextureFormat::R16Unorm),
			Some(MTLPixelFormat::R16Unorm)
		);
		assert_eq!(
			iosurface_pixel_format(wgpu::TextureFormat::Rg16Unorm),
			Some(MTLPixelFormat::RG16Unorm)
		);
		assert_eq!(
			iosurface_pixel_format(wgpu::TextureFormat::Rgba8Unorm),
			None
		);
	}

	/// `import_dmabuf_plane`'s argument validation must reject zero-sized
	/// imports and non-plane formats *before* it touches Vulkan, so this
	/// runs without any DMA-BUF: the plane fd (here invalid on purpose)
	/// is never reached.
	#[cfg(target_os = "linux")]
	#[test]
	fn dmabuf_import_rejects_invalid_arguments_without_importing() {
		let Some(ctx) = gpu_or_skip("the DMA-BUF argument validation test") else {
			return;
		};
		if ctx.kind() != crate::backend::BackendKind::Vulkan {
			eprintln!("SKIP: the DMA-BUF validation test needs a Vulkan context");
			return;
		}
		let plane = DmaBufPlane {
			fd: -1,
			offset: 0,
			pitch: 64,
			modifier: 0,
		};
		let err = ctx
			.import_dmabuf_plane(0, 64, wgpu::TextureFormat::R8Unorm, plane, None)
			.unwrap_err();
		assert!(matches!(err, Error::State), "zero width: {err:?}");
		let err = ctx
			.import_dmabuf_plane(64, 0, wgpu::TextureFormat::R8Unorm, plane, None)
			.unwrap_err();
		assert!(matches!(err, Error::State), "zero height: {err:?}");
		let err = ctx
			.import_dmabuf_plane(64, 64, wgpu::TextureFormat::Bgra8Unorm, plane, None)
			.unwrap_err();
		assert!(matches!(err, Error::Invalid), "non-plane format: {err:?}");
	}

	/// The registry/view helper every import shares: one array texture,
	/// several aliases. A single-slice alias must produce the explicit `D2`
	/// view the M5 desktop review fix requests (wgpu-core would otherwise
	/// derive `D2Array` from the texture's layer count) **and bind into the
	/// planar pass's `D2` bind-group layout** — the actual failure point of
	/// that regression: `create_view` succeeds on a derived `D2Array`, the
	/// bind group does not. Broader aliases keep the array dimension (and
	/// are shown to be rejected by the `D2` layout, so the guard is
	/// falsifiable), and unknown tokens are `NotFound`.
	#[test]
	fn imported_texture_aliases_create_valid_views_and_bind() {
		let Some(ctx) = gpu_or_skip("the imported-texture alias test") else {
			return;
		};
		// The shape a D3D11VA import registers: one 4-layer D2 texture.
		let texture = Arc::new(ctx.device.create_texture(&wgpu::TextureDescriptor {
			label: Some("oakrender-imported-test"),
			size: wgpu::Extent3d {
				width: 8,
				height: 8,
				depth_or_array_layers: 4,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::Rgba8Unorm,
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		}));
		let desc = ImportedTextureDesc {
			width: 8,
			height: 8,
			array_layers: 4,
			format: wgpu::TextureFormat::Rgba8Unorm,
			aspect: wgpu::TextureAspect::All,
			layer: Some((2, 1)),
		};
		let single = ctx.register_texture_entry(texture.clone(), desc);
		let single_view = ctx.texture_view(single).expect("single-slice alias view");

		let pair = ctx.register_texture_entry(
			texture.clone(),
			ImportedTextureDesc {
				layer: Some((0, 2)),
				..desc
			},
		);
		let pair_view = ctx.texture_view(pair).expect("two-slice alias view");

		let whole = ctx.register_texture_entry(
			texture.clone(),
			ImportedTextureDesc {
				layer: None,
				..desc
			},
		);
		ctx.texture_view(whole).expect("whole-array alias view");

		assert_ne!(single, pair, "aliases get distinct tokens");
		assert_ne!(pair, whole, "aliases get distinct tokens");
		assert!(
			matches!(ctx.texture_view(u64::MAX), Err(Error::NotFound)),
			"an unregistered token must be NotFound"
		);

		// The regression guard: the planar pass creates its bind group
		// with a `D2` layout. Without `dimension: Some(D2)` the
		// single-slice alias is a derived `D2Array` and this bind group
		// fails validation, so the test must exercise bind-group creation
		// (not just `create_view`). Use the production planar layout so the
		// guard follows it if the layout changes.
		let pipeline = ctx
			.planar_yuv_pipeline()
			.expect("the planar pass pipeline builds");
		// The chroma slot must be a single-slice `D2` alias too: slice 0 of
		// the same texture serves.
		let chroma = ctx.register_texture_entry(
			texture,
			ImportedTextureDesc {
				layer: Some((0, 1)),
				..desc
			},
		);
		let chroma_view = ctx.texture_view(chroma).expect("chroma alias view");
		let uniform = ctx.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("oakrender-imported-test-planar-params"),
			size: 64,
			usage: wgpu::BufferUsages::UNIFORM,
			mapped_at_creation: false,
		});
		let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
		let _bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("oakrender-imported-test-planar-bg"),
			layout: &pipeline.layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&single_view),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::TextureView(&chroma_view),
				},
				wgpu::BindGroupEntry {
					binding: 2,
					resource: uniform.as_entire_binding(),
				},
			],
		});
		let error = pollster_block_on(scope.pop());
		assert!(
			error.is_none(),
			"single-slice aliases must bind into the planar pass's D2 layout \
			 (a D2Array view is rejected here): {error:?}"
		);

		// Negative control: the broader (D2Array) alias is rejected by the
		// same layout, proving mismatch in the assertion above is
		// observable through the error scope.
		let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
		let _rejected = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("oakrender-imported-test-planar-bg-rejected"),
			layout: &pipeline.layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&pair_view),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::TextureView(&chroma_view),
				},
				wgpu::BindGroupEntry {
					binding: 2,
					resource: uniform.as_entire_binding(),
				},
			],
		});
		let error = pollster_block_on(scope.pop());
		assert!(
			error.is_some(),
			"a D2Array alias must be rejected by the D2 planar layout"
		);
	}
}
