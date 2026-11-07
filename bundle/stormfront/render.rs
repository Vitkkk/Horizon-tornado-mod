// Stormfront's particle pass, added to the renderer as `stormfront_render`
// (crates/horizon-client-render/src/lib.rs, public). It also re-exports the simulation crate as
// `engine`, which the game's glue reaches through `horizon_client_render::stormfront_render`.
//
// On the `world_drawn` hook (after sky, terrain, entities, Horizon's own particles and weather,
// before the hand and the HUD) it draws the instances the game thread last published
// (`engine::render_bridge`) as instanced quads over the frame's colour target, faded against
// the world's depth, which it asks Horizon to keep (`render_needs`). Particles are extrapolated
// along their velocity between the game's 20 Hz updates.

pub use stormfront as engine;

use crate::gfx::native_render::{RenderNeeds, WorldDrawn};
use engine::render_bridge::{Frame, Instance, PARTICLE_WGSL};
use std::sync::Mutex;

struct Pass {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    instances: Option<(wgpu::Buffer, u64)>,
    /// A 1x1 depth texture bound when the world's depth is not the right kind of view.
    dummy_depth: wgpu::TextureView,
}

struct State {
    pass: Option<Pass>,
    frame: Option<Frame>,
    seen: u64,
    bytes: Vec<u8>,
}

static STATE: Mutex<State> = Mutex::new(State { pass: None, frame: None, seen: 0, bytes: Vec::new() });

/// The uniform block, as the shader declares it: a mat4 and five vec4s.
const UNIFORM_SIZE: u64 = 64 + 5 * 16;

/// `render_needs`: keep the world's depth while there is something to draw.
pub fn needs(needs: &mut RenderNeeds) {
    let Ok(mut s) = STATE.lock() else { return };
    refresh(&mut s);
    if s.frame.as_ref().is_some_and(|f| !f.instances.is_empty()) {
        needs.world_depth = true;
    }
}

fn refresh(s: &mut State) {
    match engine::render_bridge::latest(s.seen) {
        Some(f) => {
            s.seen = f.version;
            s.frame = Some(f);
        }
        None => {
            // cleared (left the world, mod off): the bridge holds nothing
            if engine::render_bridge::latest(0).is_none() {
                s.frame = None;
            }
        }
    }
}

fn create_pass(device: &wgpu::Device, format: wgpu::TextureFormat) -> Pass {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("stormfront particles"),
        source: wgpu::ShaderSource::Wgsl(PARTICLE_WGSL.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("stormfront particles"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE) },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("stormfront particles"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let attributes = [
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 12, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 16, shader_location: 2 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 28, shader_location: 3 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 32, shader_location: 4 },
    ];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("stormfront particles"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: Instance::SIZE as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &attributes,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("stormfront particles"),
        size: UNIFORM_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let dummy = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("stormfront no depth"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let dummy_depth = dummy.create_view(&wgpu::TextureViewDescriptor::default());
    Pass { format, pipeline, layout, uniform, instances: None, dummy_depth }
}

/// `world_drawn`: draw the latest particles over the world.
pub fn draw(world: &mut WorldDrawn<'_>) {
    let Ok(mut guard) = STATE.lock() else { return };
    let s = &mut *guard;
    refresh(s);
    let Some(frame) = &s.frame else { return };
    if frame.instances.is_empty() || world.size[0] == 0 || world.size[1] == 0 {
        return;
    }
    if s.pass.as_ref().is_none_or(|p| p.format != world.color_format) {
        s.pass = Some(create_pass(world.device, world.color_format));
    }
    let pass = s.pass.as_mut().unwrap();
    // instances
    s.bytes.clear();
    for i in &frame.instances {
        i.write(&mut s.bytes);
    }
    let needed = s.bytes.len() as u64;
    if pass.instances.as_ref().is_none_or(|(_, cap)| *cap < needed) {
        let cap = needed.next_power_of_two().max(64 * 1024);
        let buffer = world.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stormfront particle instances"),
            size: cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        pass.instances = Some((buffer, cap));
    }
    let (instance_buffer, _) = pass.instances.as_ref().unwrap();
    world.queue.write_buffer(instance_buffer, 0, &s.bytes);
    // uniforms
    let vp = world.view_proj;
    let cols = vp.to_cols_array();
    // camera axes: the view-projection's first two rows point along the screen's x and y
    let row = |r: usize| glam::Vec3::new(cols[r], cols[4 + r], cols[8 + r]).normalize_or_zero();
    let right = row(0);
    let up = row(1);
    // with reversed depth, a camera-relative point at the eye projects to z = near
    let near = vp.w_axis.z.abs().max(1e-4);
    let anchor_rel = [
        (frame.anchor[0] - world.camera.x) as f32,
        (frame.anchor[1] - world.camera.y) as f32,
        (frame.anchor[2] - world.camera.z) as f32,
    ];
    let age = (engine::render_bridge::now() - frame.built_at).clamp(0.0, 0.25) as f32;
    let depth_ok = world.depth_size[0] > 0 && world.depth_size[1] > 0;
    let mut u: Vec<f32> = Vec::with_capacity((UNIFORM_SIZE / 4) as usize);
    u.extend_from_slice(&cols);
    u.extend_from_slice(&[anchor_rel[0], anchor_rel[1], anchor_rel[2], age]);
    u.extend_from_slice(&[right.x, right.y, right.z, 0.0]);
    u.extend_from_slice(&[up.x, up.y, up.z, 0.0]);
    u.extend_from_slice(&[
        frame.light,
        near,
        world.depth_size[0] as f32 / world.size[0] as f32,
        world.depth_size[1] as f32 / world.size[1] as f32,
    ]);
    u.extend_from_slice(&[world.size[0] as f32, world.size[1] as f32, if depth_ok { 1.0 } else { 0.0 }, 1.5]);
    let bytes: Vec<u8> = u.iter().flat_map(|f| f.to_le_bytes()).collect();
    world.queue.write_buffer(&pass.uniform, 0, &bytes);
    let depth_view = if depth_ok { world.depth } else { &pass.dummy_depth };
    let bind = world.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("stormfront particles"),
        layout: &pass.layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: pass.uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth_view) },
        ],
    });
    let mut rp = world.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("stormfront particles"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: world.color_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        ..Default::default()
    });
    rp.set_pipeline(&pass.pipeline);
    rp.set_bind_group(0, &bind, &[]);
    rp.set_vertex_buffer(0, instance_buffer.slice(..needed));
    rp.draw(0..6, 0..frame.instances.len() as u32);
}
