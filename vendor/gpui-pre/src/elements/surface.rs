use crate::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    ObjectFit, Pixels, Style, StyleRefinement, Styled, Window,
};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use crate::{DevicePixels, Size};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::sync::Arc;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use objc2_core_foundation::CFRetained;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use objc2_core_video::{CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth};
use refineable::Refineable;

/// A source of a surface's content.
#[cfg_attr(any(target_os = "macos", target_os = "ios"), derive(Clone, Debug, PartialEq, Eq))]
pub enum SurfaceSource {
    /// A CoreVideo image buffer
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    Surface(CFRetained<CVPixelBuffer>),
    /// A GPU texture (type-erased `Arc<wgpu::Texture>`) living on the
    /// window's device.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    Texture {
        /// The texture.
        texture: Arc<dyn std::any::Any + Send + Sync>,
        /// Its dimensions in device pixels.
        size: Size<DevicePixels>,
    },
}

/// A texture source for [`surface`] (Linux/FreeBSD): wraps the type-erased
/// texture plus its size.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub struct TextureSource {
    /// The texture (type-erased `Arc<wgpu::Texture>`).
    pub texture: Arc<dyn std::any::Any + Send + Sync>,
    /// Its dimensions in device pixels.
    pub size: Size<DevicePixels>,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl Clone for SurfaceSource {
    fn clone(&self) -> Self {
        match self {
            SurfaceSource::Texture { texture, size } => SurfaceSource::Texture {
                texture: texture.clone(),
                size: *size,
            },
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl std::fmt::Debug for SurfaceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SurfaceSource::Texture { size, .. } => f
                .debug_struct("Texture")
                .field("size", size)
                .finish_non_exhaustive(),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl PartialEq for SurfaceSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                SurfaceSource::Texture { texture: a, size: sa },
                SurfaceSource::Texture { texture: b, size: sb },
            ) => sa == sb && Arc::ptr_eq(a, b),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl Eq for SurfaceSource {}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl From<TextureSource> for SurfaceSource {
    fn from(value: TextureSource) -> Self {
        SurfaceSource::Texture {
            texture: value.texture,
            size: value.size,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl From<CFRetained<CVPixelBuffer>> for SurfaceSource {
    fn from(value: CFRetained<CVPixelBuffer>) -> Self {
        SurfaceSource::Surface(value)
    }
}

/// A surface element.
pub struct Surface {
    source: SurfaceSource,
    object_fit: ObjectFit,
    style: StyleRefinement,
}

/// Create a new surface element.
pub fn surface(source: impl Into<SurfaceSource>) -> Surface {
    Surface {
        source: source.into(),
        object_fit: ObjectFit::Contain,
        style: Default::default(),
    }
}

impl Surface {
    /// Set the object fit for the image.
    pub fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.object_fit = object_fit;
        self
    }
}

impl Element for Surface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);
        let layout_id = window.request_layout(style, [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        // On platforms with no surface source (Windows) the enum is
        // empty; `bounds`/`window` are then unused, and the reference
        // match below still needs a (wildcard) arm to be exhaustive.
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        let _ = (bounds, window);
        match &self.source {
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            SurfaceSource::Surface(surface) => {
                let size = crate::size(
                    CVPixelBufferGetWidth(surface).into(),
                    CVPixelBufferGetHeight(surface).into(),
                );
                let new_bounds = self.object_fit.get_bounds(bounds, size);
                // TODO: Add support for corner_radii
                window.paint_surface(new_bounds, surface.clone());
            }
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            SurfaceSource::Texture { texture, size } => {
                let new_bounds = self.object_fit.get_bounds(bounds, *size);
                window.paint_surface(new_bounds, Arc::clone(texture), *size);
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "linux",
                target_os = "freebsd"
            )))]
            _ => {}
        }
    }
}

impl IntoElement for Surface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for Surface {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
