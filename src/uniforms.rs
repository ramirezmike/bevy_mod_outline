use bevy::{
    math::Affine3,
    prelude::*,
    render::{
        batching::{no_gpu_preprocessing::BatchedInstanceBuffer, NoAutomaticBatching},
        extract_component::ExtractComponent,
        render_resource::{BindGroup, BindGroupEntry, ShaderType},
        renderer::RenderDevice,
        view::RenderLayers,
    },
};

use crate::{pipeline::OutlinePipeline, ComputedOutline};

#[derive(Component)]
pub struct ExtractedOutline {
    pub(crate) stencil: bool,
    pub(crate) volume: bool,
    pub(crate) depth_mode: DepthMode,
    pub(crate) mesh_id: AssetId<Mesh>,
    pub(crate) automatic_batching: bool,
    pub(crate) instance_data: OutlineInstanceUniform,
    pub(crate) layers: RenderLayers,
}

#[derive(Clone, ShaderType)]
pub(crate) struct OutlineInstanceUniform {
    pub world_from_local: [Vec4; 3],
    pub origin_in_world: Vec3,
    pub volume_offset: f32,
    pub volume_colour: Vec4,
    pub stencil_offset: f32,
    pub first_vertex_index: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DepthMode {
    Flat = 1,
    Real = 2,
}

#[derive(Resource)]
pub(crate) struct OutlineInstanceBindGroup {
    pub bind_group: BindGroup,
}

pub(crate) fn set_outline_visibility(mut query: Query<(&mut ViewVisibility, &ComputedOutline)>) {
    for (mut visibility, computed) in query.iter_mut() {
        if let ComputedOutline(Some(computed)) = computed {
            if computed.volume.value.enabled || computed.stencil.value.enabled {
                visibility.set();
            }
        }
    }
}

impl ExtractComponent for ComputedOutline {
    type QueryData = (
        &'static ComputedOutline,
        &'static GlobalTransform,
        &'static Mesh3d,
        Has<NoAutomaticBatching>,
    );
    type QueryFilter = ();
    type Out = ExtractedOutline;

    fn extract_component(
        (computed, transform, mesh, no_automatic_batching): bevy::ecs::query::QueryItem<
            '_,
            Self::QueryData,
        >,
    ) -> Option<Self::Out> {
        let ComputedOutline(Some(computed)) = computed else {
            return None;
        };
        Some(ExtractedOutline {
            stencil: computed.stencil.value.enabled,
            volume: computed.volume.value.enabled,
            depth_mode: computed.mode.value.depth_mode,
            layers: computed.layers.value.clone(),
            mesh_id: mesh.id(),
            automatic_batching: !no_automatic_batching,
            instance_data: OutlineInstanceUniform {
                world_from_local: Affine3::from(&transform.affine()).to_transpose(),
                origin_in_world: computed.mode.value.world_origin,
                stencil_offset: computed.stencil.value.offset,
                volume_offset: computed.volume.value.offset,
                volume_colour: computed.volume.value.colour.to_vec4(),
                first_vertex_index: 0,
            },
        })
    }
}

pub(crate) fn prepare_outline_instance_bind_group(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    batched_instance_buffer: Res<BatchedInstanceBuffer<OutlineInstanceUniform>>,
    outline_pipeline: Res<OutlinePipeline>,
) {
    if let Some(instance_binding) = batched_instance_buffer.instance_data_binding() {
        let bind_group = render_device.create_bind_group(
            "outline_instance_mesh_bind_group",
            &outline_pipeline.outline_instance_bind_group_layout,
            &[BindGroupEntry {
                binding: 0,
                resource: instance_binding,
            }],
        );
        commands.insert_resource(OutlineInstanceBindGroup { bind_group });
    };
}
