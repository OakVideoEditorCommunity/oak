use crate::{DevicePixels, Pixels, Result, SharedString, Size, size};
use smallvec::SmallVec;

use image::{Delay, Frame};
use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
};

/// A source of assets for this app to use.
pub trait AssetSource: 'static + Send + Sync {
    /// Load the given asset from the source path.
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;

    /// List the assets at the given path.
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}

impl AssetSource for () {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

/// A unique identifier for the image cache
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

static NEXT_RENDER_IMAGE_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(PartialEq, Eq, Hash, Clone)]
#[expect(missing_docs)]
pub struct RenderImageParams {
    pub image_id: ImageId,
    pub frame_index: usize,
}

/// A cached and processed image, in BGRA format
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    data: RenderImageData,
}

enum RenderImageData {
    Ready(SmallVec<[Frame; 1]>),
    Lazy {
        width: u32,
        height: u32,
        frames: OnceLock<SmallVec<[Frame; 1]>>,
        loader: Box<dyn Fn() -> SmallVec<[Frame; 1]> + Send + Sync>,
    },
}

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RenderImage {}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        Self {
            id: ImageId(NEXT_RENDER_IMAGE_ID.fetch_add(1, SeqCst)),
            scale_factor: 1.0,
            data: RenderImageData::Ready(data.into()),
        }
    }

    /// Create an image whose pixel data is produced on first CPU access.
    pub fn new_lazy(
        width: u32,
        height: u32,
        loader: impl Fn() -> SmallVec<[Frame; 1]> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: ImageId(NEXT_RENDER_IMAGE_ID.fetch_add(1, SeqCst)),
            scale_factor: 1.0,
            data: RenderImageData::Lazy {
                width,
                height,
                frames: OnceLock::new(),
                loader: Box::new(loader),
            },
        }
    }

    fn frames(&self) -> &SmallVec<[Frame; 1]> {
        match &self.data {
            RenderImageData::Ready(frames) => frames,
            RenderImageData::Lazy { frames, loader, .. } => frames.get_or_init(loader),
        }
    }

    /// Convert this image into a byte slice.
    pub fn as_bytes(&self, frame_index: usize) -> Option<&[u8]> {
        self.frames()
            .get(frame_index)
            .map(|frame| frame.buffer().as_raw().as_slice())
    }

    /// Get the size of this image, in pixels.
    pub fn size(&self, frame_index: usize) -> Size<DevicePixels> {
        match &self.data {
            RenderImageData::Lazy {
                width, height, ..
            } if frame_index == 0 => size((*width).into(), (*height).into()),
            RenderImageData::Lazy { .. } => Size::default(),
            RenderImageData::Ready(frames) => frames
                .get(frame_index)
                .map(|frame| {
                    let (width, height) = frame.buffer().dimensions();
                    size(width.into(), height.into())
                })
                .unwrap_or_default(),
        }
    }

    /// Get the size of this image, in pixels for display, adjusted for the scale factor.
    pub(crate) fn render_size(&self, frame_index: usize) -> Size<Pixels> {
        self.size(frame_index)
            .map(|v| (v.0 as f32 / self.scale_factor).into())
    }

    /// Get the delay of this frame from the previous
    pub fn delay(&self, frame_index: usize) -> Delay {
        self.frames()
            .get(frame_index)
            .map(|frame| frame.delay())
            .unwrap_or(Delay::from_numer_denom_ms(100, 1))
    }

    /// Get the number of frames for this image.
    pub fn frame_count(&self) -> usize {
        match &self.data {
            RenderImageData::Ready(frames) => frames.len(),
            RenderImageData::Lazy { .. } => 1,
        }
    }
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageData")
            .field("id", &self.id)
            .field("size", &self.size(0))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;

    #[test]
    fn empty_render_image_does_not_panic() {
        let image = RenderImage::new(SmallVec::new());
        assert_eq!(image.frame_count(), 0);
        assert_eq!(image.size(0), Size::default());
        assert_eq!(image.as_bytes(0), None);
        assert_eq!(image.render_size(0), Size::default());
        assert_eq!(image.delay(0), Delay::from_numer_denom_ms(100, 1));
        let _ = format!("{image:?}");
    }

    #[test]
    fn lazy_render_image_materializes_only_for_cpu_access() {
        let loads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let loads_in_loader = loads.clone();
        let image = RenderImage::new_lazy(2, 1, move || {
            loads_in_loader.fetch_add(1, SeqCst);
            SmallVec::from_elem(
                Frame::new(image::RgbaImage::from_raw(2, 1, vec![7; 8]).unwrap()),
                1,
            )
        });

        assert_eq!(image.size(0), size(2.into(), 1.into()));
        assert_eq!(image.frame_count(), 1);
        assert_eq!(loads.load(SeqCst), 0);
        assert_eq!(image.as_bytes(0).unwrap(), &[7; 8]);
        assert_eq!(image.as_bytes(0).unwrap(), &[7; 8]);
        assert_eq!(loads.load(SeqCst), 1);
        assert_ne!(image.id, RenderImage::new(SmallVec::new()).id);
    }
}
