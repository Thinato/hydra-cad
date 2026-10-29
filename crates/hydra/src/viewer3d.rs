//! Orbit viewer that draws a [`Mesh`] inside egui through a wgpu paint callback.

use crate::mesh::Mesh;
use anyhow::Context;
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu, wgpu::util::DeviceExt};
use glam::{Mat4, Vec3};

/// Set `NativeOptions::depth_buffer` to this; egui-wgpu maps 32 bits to
/// `Depth32Float` (`egui_wgpu::depth_format_from_bits`), which the pipeline must match.
pub const DEPTH_BUFFER_BITS: u8 = 32;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Set `NativeOptions::multisampling` to this; the pipeline sample count must match.
pub const MSAA_SAMPLES: u16 = 4;

const FOV_Y: f32 = 0.6;

const SHADER: &str = r"
struct Uniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    light_dir: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * u.model * vec4<f32>(pos, 1.0);
    out.normal = (u.model * vec4<f32>(normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let base = vec3<f32>(0.55, 0.6, 0.68);
    let lambert = max(dot(normalize(in.normal), u.light_dir.xyz), 0.0);
    return vec4<f32>(base * (0.25 + 0.75 * lambert), 1.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    light_dir: [f32; 4],
}

/// GPU objects, stored once in egui-wgpu's `callback_resources`.
// ponytail: keyed by type, so one Viewer3d at a time; key by id when several views are needed.
struct Resources {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Orbit camera state around the mesh centre.
pub struct Viewer3d {
    center: Vec3,
    radius: f32,
    yaw: f32,
    pitch: f32,
    /// Camera distance relative to the fitted distance; 1.0 shows the whole mesh.
    zoom: f32,
}

impl Viewer3d {
    /// Uploads `mesh` to the GPU. Requires eframe's wgpu renderer.
    pub fn new(cc: &eframe::CreationContext, mesh: &Mesh) -> anyhow::Result<Self> {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .context("Viewer3d needs the wgpu renderer")?;
        let device = &rs.device;

        let interleaved: Vec<[f32; 6]> = mesh
            .positions
            .iter()
            .zip(&mesh.normals)
            .map(|(p, n)| [p[0], p[1], p[2], n[0], n[1], n[2]])
            .collect();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewer3d_vertices"),
            contents: bytemuck::cast_slice(&interleaved),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewer3d_indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewer3d_uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewer3d"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewer3d"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewer3d"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewer3d"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewer3d"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 6 * 4,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                })],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES as u32,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(rs.target_format.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        rs.renderer.write().callback_resources.insert(Resources {
            pipeline,
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
            uniforms,
            bind_group,
        });

        let (lo, hi) = mesh.bounds();
        let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
        let radius = ((hi - lo).length() / 2.0).max(1e-6);
        Ok(Self {
            center: (lo + hi) / 2.0,
            radius,
            yaw: 0.6,
            pitch: 0.45,
            zoom: 1.0,
        })
    }

    /// Fills the available space with the 3D view; drag to orbit, scroll or pinch to zoom.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());

        let drag = response.drag_delta();
        self.yaw -= drag.x * 0.01;
        self.pitch = (self.pitch + drag.y * 0.01).clamp(-1.55, 1.55);
        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            self.zoom = (self.zoom * (-scroll * 0.002).exp() / zoom).clamp(0.1, 20.0);
        }

        let dir = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        // Fit the bounding sphere in the narrower of the two fields of view.
        let aspect = rect.aspect_ratio().max(1e-3);
        let half_fov = ((FOV_Y / 2.0).tan() * aspect.min(1.0)).atan();
        let distance = self.radius / half_fov.sin() * 1.1 * self.zoom;
        let eye = self.center + dir * distance;
        let view = glam::camera::rh::view::look_at_mat4(eye, self.center, Vec3::Y);
        let proj = glam::camera::rh::proj::directx::perspective(
            FOV_Y,
            aspect,
            distance * 0.01,
            distance + self.radius * 4.0,
        );
        // Headlight, nudged up and to the right so faces facing the camera differ.
        let right = Vec3::Y.cross(dir).normalize_or_zero();
        let light = (dir + Vec3::Y * 0.4 + right * 0.3).normalize();

        ui.painter()
            .rect_filled(rect, 0.0, egui::Color32::from_gray(40));
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Paint(Uniforms {
                view_proj: (proj * view).to_cols_array_2d(),
                model: Mat4::IDENTITY.to_cols_array_2d(),
                light_dir: light.extend(0.0).to_array(),
            }),
        ));
    }
}

/// One frame's draw of the mesh.
struct Paint(Uniforms);

impl egui_wgpu::CallbackTrait for Paint {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(r) = resources.get::<Resources>() {
            queue.write_buffer(&r.uniforms, 0, bytemuck::bytes_of(&self.0));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(r) = resources.get::<Resources>() else {
            return;
        };
        pass.set_pipeline(&r.pipeline);
        pass.set_bind_group(0, &r.bind_group, &[]);
        pass.set_vertex_buffer(0, r.vertices.slice(..));
        pass.set_index_buffer(r.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..r.index_count, 0, 0..1);
    }
}
