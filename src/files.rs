//! Draws every file in one instanced draw call.
//!
//! One entity per file made Bevy's renderer do per-entity work (extraction, visibility,
//! batching) for thousands of identical spheres: ~0.5 µs per file per frame. Here the whole
//! file layer is a single entity carrying an instance list (position, scale, color, glow) that
//! is rebuilt each frame, copied into a reused render-world list, and uploaded into a reused
//! GPU buffer: no per-frame allocation. Adapted from Bevy's `custom_shader_instancing` example.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::core_pipeline::core_3d::{Transparent3d, TransparentSortingInfo3d};
use bevy::ecs::query::QueryItem;
use bevy::ecs::system::SystemParamItem;
use bevy::ecs::system::lifetimeless::SRes;
use bevy::mesh::{MeshVertexBufferLayoutRef, VertexBufferLayout};
use bevy::pbr::{
    MeshPipeline, MeshPipelineKey, MeshPipelineSystems, RenderMeshInstances, SetMeshBindGroup,
    SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup, ViewKeyCache,
};
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::mesh::allocator::MeshAllocator;
use bevy::render::mesh::{RenderMesh, RenderMeshBufferInfo};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
    RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_component::SyncComponent;
use bevy::render::sync_world::MainEntity;
use bevy::render::view::ExtractedView;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bytemuck::{Pod, Zeroable};

/// One file as the GPU sees it.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct FileInstance {
    pub position: Vec3,
    pub scale: f32,
    /// Linear RGB, and the emissive strength in alpha (high values bloom).
    pub color: [f32; 4],
}

/// The instance list of the file layer, filled by the app every frame.
#[derive(Component, Default)]
pub struct FileInstances(pub Vec<FileInstance>);

/// Marks the file layer's entity, in both worlds.
#[derive(Component, Clone, Copy, Default)]
struct FileLayer;

impl SyncComponent<RenderApp> for FileLayer {
    type Target = Self;
}

impl ExtractComponent<RenderApp> for FileLayer {
    type QueryData = ();
    type QueryFilter = With<FileLayer>;
    type Out = Self;

    fn extract_component(_: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        Some(FileLayer)
    }
}

/// The render world's copy of the instances, reused across frames.
#[derive(Resource, Default)]
struct ExtractedInstances(Vec<FileInstance>);

pub struct FilesPlugin;

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wesl(
                include_str!("files.wesl"),
                "eridu/files.wesl",
            ));
        app.add_plugins(ExtractComponentPlugin::<FileLayer>::default());
        app.sub_app_mut(RenderApp)
            .insert_resource(FileShader(shader))
            .init_resource::<ExtractedInstances>()
            .init_resource::<FileBuffer>()
            .add_systems(ExtractSchedule, extract_instances)
            .add_render_command::<Transparent3d, DrawFiles>()
            .init_resource::<SpecializedMeshPipelines<FilePipeline>>()
            .add_systems(RenderStartup, init_pipeline.after(MeshPipelineSystems))
            .add_systems(
                Render,
                (
                    queue_files.in_set(RenderSystems::QueueMeshes),
                    prepare_buffer.in_set(RenderSystems::PrepareResources),
                ),
            );
    }
}

/// Spawns the file layer: one entity, drawn with `mesh` once per instance.
pub fn spawn_layer(commands: &mut Commands, mesh: Handle<Mesh>) {
    commands.spawn((
        Mesh3d(mesh),
        FileInstances::default(),
        FileLayer,
        // Instances are placed in the shader, so the mesh's own bounds mean nothing.
        NoFrustumCulling,
    ));
}

#[derive(Resource)]
struct FileShader(Handle<Shader>);

/// The instance buffer, reused across frames and only reallocated when it must grow.
#[derive(Resource, Default)]
struct FileBuffer {
    buffer: Option<Buffer>,
    capacity: usize,
    length: usize,
}

fn extract_instances(
    layers: Extract<Query<&FileInstances>>,
    mut extracted: ResMut<ExtractedInstances>,
) {
    extracted.0.clear();
    for layer in &layers {
        extracted.0.extend_from_slice(&layer.0);
    }
}

fn prepare_buffer(
    extracted: Res<ExtractedInstances>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut file_buffer: ResMut<FileBuffer>,
) {
    let instances = extracted.0.as_slice();
    file_buffer.length = instances.len();
    if instances.is_empty() {
        return;
    }
    if file_buffer.capacity < instances.len() {
        // Grow ahead, so a growing tree doesn't reallocate every frame.
        let capacity = instances.len().next_power_of_two();
        file_buffer.buffer = Some(device.create_buffer(&BufferDescriptor {
            label: Some("file instances"),
            size: (capacity * size_of::<FileInstance>()) as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        file_buffer.capacity = capacity;
    }
    if let Some(buffer) = &file_buffer.buffer {
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(instances));
    }
}

#[allow(clippy::too_many_arguments)]
fn queue_files(
    draw_functions: Res<DrawFunctions<Transparent3d>>,
    pipeline: Res<FilePipeline>,
    mut pipelines: ResMut<SpecializedMeshPipelines<FilePipeline>>,
    pipeline_cache: Res<PipelineCache>,
    meshes: Res<RenderAssets<RenderMesh>>,
    mesh_instances: Res<RenderMeshInstances>,
    layers: Query<(Entity, &MainEntity), With<FileLayer>>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    views: Query<&ExtractedView>,
    view_keys: Res<ViewKeyCache>,
) {
    let draw_files = draw_functions.read().id::<DrawFiles>();
    for view in &views {
        let (Some(phase), Some(&view_key)) = (
            phases.get_mut(&view.retained_view_entity),
            view_keys.get(&view.retained_view_entity),
        ) else {
            continue;
        };
        for (entity, main_entity) in &layers {
            let Some(mesh_instance) = mesh_instances.render_mesh_queue_data(*main_entity) else {
                continue;
            };
            let Some(mesh) = meshes.get(mesh_instance.mesh_asset_id()) else {
                continue;
            };
            let key = view_key
                | MeshPipelineKey::from_primitive_topology_and_strip_index(
                    mesh.primitive_topology(),
                    mesh.index_format(),
                );
            let Ok(pipeline) = pipelines.specialize(&pipeline_cache, &pipeline, key, &mesh.layout)
            else {
                continue;
            };
            phase.add_retained(Transparent3d {
                sorting_info: TransparentSortingInfo3d::Sorted {
                    mesh_center: Vec3::ZERO,
                    depth_bias: 0.0,
                },
                entity: (entity, *main_entity),
                pipeline,
                draw_function: draw_files,
                distance: 0.0,
                batch_range: 0..1,
                extra_index: PhaseItemExtraIndex::None,
                indexed: true,
            });
        }
    }
}

#[derive(Resource)]
struct FilePipeline {
    shader: Handle<Shader>,
    mesh_pipeline: MeshPipeline,
}

fn init_pipeline(
    mut commands: Commands,
    shader: Res<FileShader>,
    mesh_pipeline: Res<MeshPipeline>,
) {
    commands.insert_resource(FilePipeline {
        shader: shader.0.clone(),
        mesh_pipeline: mesh_pipeline.clone(),
    });
}

impl SpecializedMeshPipeline for FilePipeline {
    type Key = MeshPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = self.mesh_pipeline.specialize(key, layout)?;
        descriptor.vertex.shader = self.shader.clone();
        descriptor.vertex.buffers.push(VertexBufferLayout {
            array_stride: size_of::<FileInstance>() as u64,
            step_mode: VertexStepMode::Instance,
            attributes: vec![
                // Locations 0–2 are the mesh's position, normal and uv.
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 3,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: VertexFormat::Float32x4.size(),
                    shader_location: 4,
                },
            ],
        });
        descriptor
            .fragment
            .as_mut()
            .expect("mesh pipelines have a fragment stage")
            .shader = self.shader.clone();
        Ok(descriptor)
    }
}

type DrawFiles = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    DrawInstanced,
);

struct DrawInstanced;

impl<P: PhaseItem> RenderCommand<P> for DrawInstanced {
    type Param = (
        SRes<RenderAssets<RenderMesh>>,
        SRes<RenderMeshInstances>,
        SRes<MeshAllocator>,
        SRes<FileBuffer>,
    );
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _entity: Option<()>,
        (meshes, mesh_instances, mesh_allocator, file_buffer): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (mesh_allocator, file_buffer) = (mesh_allocator.into_inner(), file_buffer.into_inner());
        let (Some(buffer), length) = (&file_buffer.buffer, file_buffer.length as u32) else {
            return RenderCommandResult::Skip;
        };
        let Some(mesh_instance) = mesh_instances.render_mesh_queue_data(item.main_entity()) else {
            return RenderCommandResult::Skip;
        };
        let Some(gpu_mesh) = meshes.into_inner().get(mesh_instance.mesh_asset_id()) else {
            return RenderCommandResult::Skip;
        };
        let Some(vertices) = mesh_allocator.mesh_vertex_slice(&mesh_instance.mesh_asset_id())
        else {
            return RenderCommandResult::Skip;
        };
        if length == 0 {
            return RenderCommandResult::Skip;
        }
        pass.set_vertex_buffer(0, vertices.buffer.slice(..));
        pass.set_vertex_buffer(1, buffer.slice(..));
        match &gpu_mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format,
                count,
            } => {
                let Some(indices) = mesh_allocator.mesh_index_slice(&mesh_instance.mesh_asset_id())
                else {
                    return RenderCommandResult::Skip;
                };
                pass.set_index_buffer(indices.buffer.slice(..), *index_format);
                pass.draw_indexed(
                    indices.range.start..indices.range.start + count,
                    vertices.range.start as i32,
                    0..length,
                );
            }
            RenderMeshBufferInfo::NonIndexed => pass.draw(vertices.range, 0..length),
        }
        RenderCommandResult::Success
    }
}
