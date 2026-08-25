//! A texture owned by a renderer outside of GPUI.
//!
//! This is deliberately a small transport type.  GPUI owns the scene
//! primitive and the platform renderer owns the resource import.  In
//! particular, constructing an [`ExternalTexture`] does not copy pixels into
//! the sprite atlas.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::{DevicePixels, Size, size};

/// A process-local identifier for an external texture.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalTextureId(pub u64);

/// The native resource exported by the producer renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalTextureSource {
    /// A shared NT handle exported by a D3D12 producer and imported by the
    /// GPUI D3D11 renderer.
    #[cfg(windows)]
    D3D11SharedHandle {
        /// The NT handle value. The producer retains ownership for the
        /// lifetime of the [`ExternalTexture`].
        handle: isize,
        /// Whether the resource is protected by a keyed mutex.
        keyed_mutex: bool,
    },
    /// A token for an `MTLTexture` registered with the Metal renderer.
    ///
    /// The producer builds its wgpu device *on GPUI's own `MTLDevice`*, so the
    /// texture needs no import and no IOSurface round-trip — but `gpui` cannot
    /// name a Metal type, so the identity crosses as a token exactly like the
    /// wgpu one.
    #[cfg(target_os = "macos")]
    MetalTexture(usize),
    /// An opaque wgpu texture token. The Linux renderer replaces this token
    /// with the device-local `wgpu::Texture` before drawing.
    #[cfg(target_os = "linux")]
    Wgpu(usize),
    /// Explicitly unsupported on platforms without an importer.
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    Unsupported,
}

/// Metadata and native source for a texture that can be painted by GPUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalTexture {
    /// Process-local identity used to associate scene entries with a source.
    pub id: ExternalTextureId,
    /// Size of the producer texture in device pixels.
    pub size: Size<DevicePixels>,
    /// Native producer resource.
    pub source: ExternalTextureSource,
}

impl ExternalTextureSource {
    /// The wgpu registry token, when this source is one.
    ///
    /// Returning `None` elsewhere lets the wgpu renderer keep a single code
    /// path that still type-checks on every host, instead of a `cfg` body that
    /// only ever compiles on Linux.
    pub fn wgpu_token(&self) -> Option<usize> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Wgpu(token) => Some(*token),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// The Metal registry token, when this source is one. Same single-code-path
    /// reason as [`Self::wgpu_token`].
    pub fn metal_token(&self) -> Option<usize> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MetalTexture(token) => Some(*token),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }
}

impl ExternalTexture {
    /// Creates an external texture and assigns a fresh process-local id.
    pub fn new(source: ExternalTextureSource, size: Size<DevicePixels>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: ExternalTextureId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            size,
            source,
        }
    }

    /// Convenience constructor for the Windows shared-handle path.
    #[cfg(windows)]
    pub fn d3d11_shared_handle(handle: isize, keyed_mutex: bool, width: i32, height: i32) -> Self {
        Self::new(
            ExternalTextureSource::D3D11SharedHandle {
                handle,
                keyed_mutex,
            },
            size(DevicePixels(width), DevicePixels(height)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_size_is_preserved() {
        #[cfg(windows)]
        let source = ExternalTextureSource::D3D11SharedHandle {
            handle: 17,
            keyed_mutex: true,
        };
        #[cfg(target_os = "macos")]
        let source = ExternalTextureSource::MetalTexture(17);
        #[cfg(target_os = "linux")]
        let source = ExternalTextureSource::Wgpu(17);
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        let source = ExternalTextureSource::Unsupported;

        let first =
            ExternalTexture::new(source.clone(), size(DevicePixels(640), DevicePixels(480)));
        let second = ExternalTexture::new(source, size(DevicePixels(1), DevicePixels(2)));
        assert_ne!(first.id, second.id);
        assert_eq!(first.size, size(DevicePixels(640), DevicePixels(480)));
    }

    #[cfg(windows)]
    #[test]
    fn scene_keeps_external_texture_order_and_clip() {
        let source = ExternalTextureSource::D3D11SharedHandle {
            handle: 17,
            keyed_mutex: true,
        };
        let texture = ExternalTexture::new(source, size(DevicePixels(16), DevicePixels(8)));
        let bounds = crate::Bounds {
            origin: crate::point(crate::ScaledPixels(4.0), crate::ScaledPixels(6.0)),
            size: crate::size(crate::ScaledPixels(16.0), crate::ScaledPixels(8.0)),
        };
        let clip = crate::Bounds {
            origin: crate::point(crate::ScaledPixels(8.0), crate::ScaledPixels(6.0)),
            size: crate::size(crate::ScaledPixels(12.0), crate::ScaledPixels(8.0)),
        };
        let mut scene = crate::Scene::default();
        scene.insert_primitive(crate::PaintExternalTexture {
            order: 0,
            bounds,
            tex_bounds: bounds,
            content_mask: crate::ContentMask { bounds: clip },
            corner_radii: crate::Corners::default(),
            opacity: 1.0,
            texture,
        });
        scene.finish();
        let Some(crate::PrimitiveBatch::ExternalTextures(range)) = scene.batches().next() else {
            panic!("external texture batch missing");
        };
        assert_eq!(range, 0..1);
        let primitive =
            crate::Primitive::ExternalTexture(scene.external_textures.first().unwrap().clone());
        assert_eq!(primitive.bounds(), &bounds);
        assert_eq!(primitive.content_mask().bounds, clip);
    }

    /// A scene with several external textures batches them into ranges that do
    /// NOT start at zero. A renderer must therefore index
    /// `scene.external_textures` by `range.start + i`, not by the offset inside
    /// the batch: getting that wrong draws one primitive with another's bounds,
    /// which is invisible while a single external texture exists.
    #[cfg(windows)]
    #[test]
    fn external_texture_batches_do_not_all_start_at_zero() {
        let source = ExternalTextureSource::D3D11SharedHandle {
            handle: 17,
            keyed_mutex: false,
        };
        let mask = crate::ContentMask {
            bounds: crate::Bounds {
                origin: crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
                size: crate::size(crate::ScaledPixels(1000.0), crate::ScaledPixels(1000.0)),
            },
        };
        let mut scene = crate::Scene::default();
        for i in 0..3 {
            let offset = crate::ScaledPixels(i as f32 * 100.0);
            scene.insert_primitive(crate::PaintExternalTexture {
                order: 0,
                bounds: crate::Bounds {
                    origin: crate::point(offset, offset),
                    size: crate::size(crate::ScaledPixels(50.0), crate::ScaledPixels(50.0)),
                },
                tex_bounds: mask.bounds,
                content_mask: mask,
                corner_radii: crate::Corners::default(),
                opacity: 1.0,
                texture: ExternalTexture::new(
                    source.clone(),
                    size(DevicePixels(16), DevicePixels(8)),
                ),
            });
        }
        scene.finish();

        let covered: Vec<_> = scene
            .batches()
            .filter_map(|batch| match batch {
                crate::PrimitiveBatch::ExternalTextures(range) => Some(range),
                _ => None,
            })
            .collect();
        let total: usize = covered.iter().map(|r| r.len()).sum();
        assert_eq!(total, 3, "every external texture must be drawn exactly once");
        assert_eq!(
            covered.last().expect("a batch").end,
            3,
            "the last batch must reach the end of scene.external_textures"
        );
    }
}
