//! This crate provides a Bevy plugin, [`OutlinePlugin`], and associated components for
//! rendering outlines around meshes using the vertex extrusion method.
//!
//! Outlines are rendered in a seperate pass following the main 3D pass. The effect of this
//! pass is to present the outlines in depth sorted order according to the model translation
//! of each mesh. This ensures that outlines are not clipped by non-outline geometry.
//!
//! The [`OutlineVolume`] component will, by itself, cover the original object entirely with
//! the outline colour. The [`OutlineStencil`] component must also be added to prevent the body
//! of an object from being filled it. This must be added to any entity which needs to appear on
//! top of an outline.
//!
//! The [`OutlineMode`] component specifies the rendering method. Outlines may be flattened into
//! a plane in order to further avoid clipping, or left in real space.
//!
//! Outlines can be inherited from the parent via the [`InheritOutline`] component.
//!
//! Vertex extrusion works best with meshes that have smooth surfaces. To avoid visual
//! artefacts when outlining meshes with hard edges, see the
//! [`OutlineMeshExt::generate_outline_normals`] function and the
//! [`AutoGenerateOutlineNormalsPlugin`].

use bevy::asset::load_internal_asset;
use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::prelude::*;
use bevy::render::batching::no_gpu_preprocessing::{
    clear_batched_cpu_instance_buffers, write_batched_instance_buffer, BatchedInstanceBuffer,
};
use bevy::render::extract_component::{ExtractComponentPlugin, UniformComponentPlugin};
use bevy::render::mesh::MeshVertexAttribute;
use bevy::render::render_graph::{RenderGraphApp, RenderLabel, ViewNodeRunner};
use bevy::render::render_phase::{
    sort_phase_system, AddRenderCommand, DrawFunctions, SortedRenderPhasePlugin,
};
use bevy::render::render_resource::{SpecializedMeshPipelines, VertexFormat};
use bevy::render::renderer::RenderDevice;
use bevy::render::view::{RenderLayers, VisibilitySystems};
use bevy::render::{Render, RenderApp, RenderSet};
use bevy::transform::TransformSystem;
use render::{DrawOutline, DrawStencil};
use scene::AsyncSceneInheritOutlineSystems;

use crate::msaa::MsaaExtraWritebackNode;
use crate::node::{OpaqueOutline, OutlineNode, StencilOutline, TransparentOutline};
use crate::pipeline::{
    OutlinePipeline, COMMON_SHADER_HANDLE, FRAGMENT_SHADER_HANDLE, OUTLINE_SHADER_HANDLE,
};
use crate::queue::queue_outline_mesh;
use crate::uniforms::set_outline_visibility;
use crate::uniforms::{prepare_outline_instance_bind_group, OutlineInstanceUniform};
use crate::view_uniforms::{
    extract_outline_view_uniforms, prepare_outline_view_bind_group, OutlineViewUniform,
};

mod computed;
mod generate;
mod msaa;
mod node;
mod pipeline;
mod queue;
mod render;
mod uniforms;
mod view_uniforms;

pub use computed::*;
pub use generate::*;

#[cfg(feature = "scene")]
mod scene;
#[cfg(feature = "scene")]
pub use scene::*;

/// Legacy bundles.
#[deprecated(since = "0.9.0", note = "Use required components instead")]
pub mod bundles;

// See https://alexanderameye.github.io/notes/rendering-outlines/

/// The direction to extrude the vertex when rendering the outline.
pub const ATTRIBUTE_OUTLINE_NORMAL: MeshVertexAttribute =
    MeshVertexAttribute::new("Outline_Normal", 1585570526, VertexFormat::Float32x3);

/// Labels for render graph nodes which draw outlines.
#[derive(Copy, Clone, Debug, RenderLabel, Hash, PartialEq, Eq)]
#[non_exhaustive]
pub enum NodeOutline {
    /// This node writes back unsampled post-processing effects to the sampled attachment.
    MsaaExtraWritebackPass,
    /// This node runs after the main 3D passes and before the UI pass.
    OutlinePass,
}

/// A component for stenciling meshes during outline rendering.
#[derive(Clone, Component)]
#[cfg_attr(feature = "reflect", derive(Reflect))]
#[cfg_attr(feature = "reflect", reflect(Component, Default))]
pub struct OutlineStencil {
    /// Enable rendering of the stencil
    pub enabled: bool,
    /// Offset of the stencil in logical pixels
    pub offset: f32,
}

impl Default for OutlineStencil {
    fn default() -> Self {
        OutlineStencil {
            enabled: true,
            offset: 0.0,
        }
    }
}

fn lerp_bool(this: bool, other: bool, scalar: f32) -> bool {
    if scalar <= 0.0 {
        this
    } else if scalar >= 1.0 {
        other
    } else {
        this | other
    }
}

macro_rules! impl_lerp {
    ($t:ty, $e:expr) => {
        impl Ease for $t {
            fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
                FunctionCurve::new(Interval::UNIT, move |t| $e(&start, &end, t))
            }
        }

        #[cfg(feature = "interpolation")]
        impl interpolation::Lerp for $t {
            type Scalar = f32;

            fn lerp(&self, other: &Self, scalar: &Self::Scalar) -> Self {
                $e(self, other, *scalar)
            }
        }
    };
}

fn lerp_stencil(start: &OutlineStencil, end: &OutlineStencil, t: f32) -> OutlineStencil {
    OutlineStencil {
        enabled: lerp_bool(start.enabled, end.enabled, t),
        offset: start.offset.lerp(end.offset, t),
    }
}

impl_lerp!(OutlineStencil, lerp_stencil);

/// A component for rendering outlines around meshes.
#[derive(Clone, Component, Default)]
#[cfg_attr(feature = "reflect", derive(Reflect))]
#[require(OutlineStencil)]
#[cfg_attr(feature = "reflect", reflect(Component, Default))]
pub struct OutlineVolume {
    /// Enable rendering of the outline
    pub visible: bool,
    /// Width of the outline in logical pixels
    pub width: f32,
    /// Colour of the outline
    pub colour: Color,
}

fn lerp_volume(start: &OutlineVolume, end: &OutlineVolume, t: f32) -> OutlineVolume {
    OutlineVolume {
        visible: lerp_bool(start.visible, end.visible, t),
        width: start.width.lerp(end.width, t),
        colour: start.colour.mix(&end.colour, t),
    }
}

impl_lerp!(OutlineVolume, lerp_volume);

/// A component for specifying what layer(s) the outline should be rendered for.
#[derive(Component, Clone, PartialEq, Eq, PartialOrd, Ord, Deref, DerefMut, Default)]
#[cfg_attr(feature = "reflect", derive(Reflect))]
#[cfg_attr(feature = "reflect", reflect(Component, Default))]
pub struct OutlineRenderLayers(pub RenderLayers);

/// A component which specifies how the outline should be rendered.
#[derive(Clone, Component)]
#[cfg_attr(feature = "reflect", derive(Reflect))]
#[cfg_attr(feature = "reflect", reflect(Component, Default))]
#[non_exhaustive]
pub enum OutlineMode {
    /// Vertex extrusion flattened into a plane facing the camera and intersecting the specified
    /// point in model-space.
    FlatVertex { model_origin: Vec3 },
    /// Vertex extrusion in real model-space.
    RealVertex,
}

impl Default for OutlineMode {
    fn default() -> Self {
        OutlineMode::FlatVertex {
            model_origin: Vec3::ZERO,
        }
    }
}

/// A component for inheriting outlines from the parent entity.
#[derive(Clone, Component, Default)]
#[cfg_attr(feature = "reflect", derive(Reflect))]
#[cfg_attr(feature = "reflect", reflect(Component, Default))]
pub struct InheritOutline;

/// Adds support for rendering outlines.
pub struct OutlinePlugin;

impl Plugin for OutlinePlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, COMMON_SHADER_HANDLE, "common.wgsl", Shader::from_wgsl);
        load_internal_asset!(
            app,
            OUTLINE_SHADER_HANDLE,
            "outline.wgsl",
            Shader::from_wgsl
        );
        load_internal_asset!(
            app,
            FRAGMENT_SHADER_HANDLE,
            "fragment.wgsl",
            Shader::from_wgsl
        );

        app.add_plugins((
            ExtractComponentPlugin::<ComputedOutline>::default(),
            UniformComponentPlugin::<OutlineViewUniform>::default(),
            SortedRenderPhasePlugin::<StencilOutline, OutlinePipeline>::default(),
            SortedRenderPhasePlugin::<OpaqueOutline, OutlinePipeline>::default(),
            SortedRenderPhasePlugin::<TransparentOutline, OutlinePipeline>::default(),
        ))
        .register_required_components::<OutlineStencil, ComputedOutline>()
        .register_required_components::<OutlineVolume, ComputedOutline>()
        .register_required_components::<InheritOutline, ComputedOutline>()
        .add_systems(
            PostUpdate,
            (
                compute_outline
                    .after(TransformSystem::TransformPropagate)
                    .after(VisibilitySystems::VisibilityPropagate),
                set_outline_visibility.in_set(VisibilitySystems::CheckVisibility),
            ),
        )
        .sub_app_mut(RenderApp)
        .init_resource::<DrawFunctions<StencilOutline>>()
        .init_resource::<DrawFunctions<OpaqueOutline>>()
        .init_resource::<DrawFunctions<TransparentOutline>>()
        .init_resource::<SpecializedMeshPipelines<OutlinePipeline>>()
        .add_render_command::<StencilOutline, DrawStencil>()
        .add_render_command::<OpaqueOutline, DrawOutline>()
        .add_render_command::<TransparentOutline, DrawOutline>()
        .add_systems(ExtractSchedule, extract_outline_view_uniforms)
        .add_systems(
            Render,
            msaa::prepare_msaa_extra_writeback_pipelines.in_set(RenderSet::Prepare),
        )
        .add_systems(
            Render,
            (
                prepare_outline_view_bind_group,
                prepare_outline_instance_bind_group,
            )
                .in_set(RenderSet::PrepareBindGroups),
        )
        .add_systems(
            Render,
            write_batched_instance_buffer::<OutlinePipeline>
                .in_set(RenderSet::PrepareResourcesFlush),
        )
        .add_systems(Render, queue_outline_mesh.in_set(RenderSet::QueueMeshes))
        .add_systems(
            Render,
            (
                sort_phase_system::<StencilOutline>,
                sort_phase_system::<OpaqueOutline>,
                sort_phase_system::<TransparentOutline>,
            )
                .in_set(RenderSet::PhaseSort),
        )
        .add_systems(
            Render,
            clear_batched_cpu_instance_buffers::<OutlinePipeline>
                .in_set(RenderSet::Cleanup)
                .after(RenderSet::Render),
        )
        .add_render_graph_node::<ViewNodeRunner<MsaaExtraWritebackNode>>(
            Core3d,
            NodeOutline::MsaaExtraWritebackPass,
        )
        .add_render_graph_node::<ViewNodeRunner<OutlineNode>>(Core3d, NodeOutline::OutlinePass)
        // Outlining occurs after tone-mapping...
        .add_render_graph_edges(
            Core3d,
            (
                Node3d::Tonemapping,
                NodeOutline::MsaaExtraWritebackPass,
                NodeOutline::OutlinePass,
                Node3d::EndMainPassPostProcessing,
            ),
        )
        // ...and before any later anti-aliasing.
        .add_render_graph_edge(Core3d, NodeOutline::OutlinePass, Node3d::Fxaa)
        .add_render_graph_edge(Core3d, NodeOutline::OutlinePass, Node3d::Smaa);

        #[cfg(feature = "reflect")]
        app.register_type::<OutlineStencil>()
            .register_type::<OutlineVolume>()
            .register_type::<OutlineRenderLayers>()
            .register_type::<OutlineMode>()
            .register_type::<InheritOutline>();

        #[cfg(feature = "scene")]
        app.init_resource::<AsyncSceneInheritOutlineSystems>();
    }

    fn finish(&self, app: &mut App) {
        let render_app = app.sub_app_mut(RenderApp);
        let render_device = render_app.world().resource::<RenderDevice>();
        let instance_buffer = BatchedInstanceBuffer::<OutlineInstanceUniform>::new(render_device);
        render_app
            .init_resource::<OutlinePipeline>()
            .insert_resource(instance_buffer);
    }
}
