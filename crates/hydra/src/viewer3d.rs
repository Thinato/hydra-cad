//! Orbit viewer that draws meshes inside egui through a wgpu paint callback.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu, wgpu::util::DeviceExt};
use glam::{Mat4, Vec3};
use hydra_mesh::Mesh;

use crate::theme::LIGHT;

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
    color: vec4<f32>,
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
    // Two-sided, so faces a CAD export got inside out still show.
    let lambert = abs(dot(normalize(in.normal), u.light_dir.xyz));
    return vec4<f32>(u.color.rgb * (0.25 + 0.75 * lambert), 1.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    light_dir: [f32; 4],
    color: [f32; 4],
}

/// One mesh to draw, placed in the scene. `key` names the mesh for the GPU cache, e.g. its hash.
#[derive(Clone)]
pub struct Instance {
    pub key: String,
    pub mesh: Arc<Mesh>,
    pub transform: Mat4,
    /// Shaded from this colour, a palette token.
    pub color: egui::Color32,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// GPU objects, stored once in egui-wgpu's `callback_resources`.
// ponytail: keyed by type, so one Viewer3d at a time; key by id when several views are needed.
// Meshes stay uploaded until the app exits, which is fine for a catalogue's worth of models.
struct Resources {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    meshes: HashMap<String, GpuMesh>,
    /// One [`Uniforms`] per instance, `stride` bytes apart, bound with a dynamic offset.
    uniforms: wgpu::Buffer,
    stride: u64,
    capacity: usize,
    bind_group: wgpu::BindGroup,
}

/// Orbit camera, refitted whenever the scene changes.
pub struct Viewer3d {
    center: Vec3,
    radius: f32,
    yaw: f32,
    pitch: f32,
    /// Camera distance relative to the fitted distance; 1.0 shows the whole scene.
    zoom: f32,
    /// The scene the camera was last fitted to.
    fitted: Vec<(String, [f32; 16])>,
}

fn uniform_buffer(device: &wgpu::Device, stride: u64, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("viewer3d_uniforms"),
        size: stride * capacity as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewer3d"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: uniforms,
                offset: 0,
                size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64),
            }),
        }],
    })
}

impl Viewer3d {
    /// Builds the render pipeline. Requires eframe's wgpu renderer.
    pub fn new(cc: &eframe::CreationContext) -> anyhow::Result<Self> {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .context("Viewer3d needs the wgpu renderer")?;
        let device = &rs.device;
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = (size_of::<Uniforms>() as u64).div_ceil(align) * align;
        let capacity = 16;
        let uniforms = uniform_buffer(device, stride, capacity);

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewer3d"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        let bind_group = bind_group(device, &layout, &uniforms);
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
            primitive: wgpu::PrimitiveState::default(),
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
            layout,
            meshes: HashMap::new(),
            uniforms,
            stride,
            capacity,
            bind_group,
        });
        Ok(Self::unfitted())
    }

    /// The camera before it has seen a scene.
    fn unfitted() -> Self {
        Self {
            center: Vec3::ZERO,
            radius: 1.0,
            // Seen from the front left, above: Hydra's model space is Z up.
            yaw: -1.1,
            pitch: 0.5,
            zoom: 1.0,
            fitted: Vec::new(),
        }
    }

    /// Fit the camera to the scene's bounding box when the scene changed.
    fn fit(&mut self, scene: &[Instance]) {
        let signature: Vec<_> = scene
            .iter()
            .map(|i| (i.key.clone(), i.transform.to_cols_array()))
            .collect();
        if signature == self.fitted {
            return;
        }
        self.fitted = signature;
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in scene {
            let (a, b) = i.mesh.bounds();
            for corner in 0..8 {
                let p = Vec3::new(
                    if corner & 1 == 0 { a[0] } else { b[0] },
                    if corner & 2 == 0 { a[1] } else { b[1] },
                    if corner & 4 == 0 { a[2] } else { b[2] },
                );
                let p = i.transform.transform_point3(p);
                (lo, hi) = (lo.min(p), hi.max(p));
            }
        }
        if lo.x <= hi.x {
            self.center = (lo + hi) / 2.0;
            self.radius = ((hi - lo).length() / 2.0).max(1e-3);
        }
    }

    /// Fills the available space with `scene`; drag to orbit, scroll or pinch to zoom,
    /// double-click to see everything again.
    pub fn ui(&mut self, ui: &mut egui::Ui, scene: &[Instance]) {
        self.fit(scene);
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if response.double_clicked() {
            self.zoom = 1.0;
        }

        let drag = response.drag_delta();
        self.yaw -= drag.x * 0.01;
        self.pitch = (self.pitch + drag.y * 0.01).clamp(-1.55, 1.55);
        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            self.zoom = (self.zoom * (-scroll * 0.002).exp() / zoom).clamp(0.1, 20.0);
        }

        let dir = Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
        );
        // Fit the bounding sphere in the narrower of the two fields of view.
        let aspect = rect.aspect_ratio().max(1e-3);
        let half_fov = ((FOV_Y / 2.0).tan() * aspect.min(1.0)).atan();
        let distance = self.radius / half_fov.sin() * 1.1 * self.zoom;
        let eye = self.center + dir * distance;
        let view = glam::camera::rh::view::look_at_mat4(eye, self.center, Vec3::Z);
        let proj = glam::camera::rh::proj::directx::perspective(
            FOV_Y,
            aspect,
            distance * 0.01,
            distance + self.radius * 4.0,
        );
        // Headlight, nudged up and to the right so faces facing the camera differ.
        let right = Vec3::Z.cross(dir).normalize_or_zero();
        let light = (dir + Vec3::Z * 0.4 + right * 0.3).normalize();

        ui.painter().rect_filled(rect, 0.0, LIGHT.viewport);
        let view_proj = (proj * view).to_cols_array_2d();
        let light_dir = light.extend(0.0).to_array();
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Paint(
                scene
                    .iter()
                    .map(|i| {
                        let uniforms = Uniforms {
                            view_proj,
                            model: i.transform.to_cols_array_2d(),
                            light_dir,
                            color: i.color.to_normalized_gamma_f32(),
                        };
                        (i.key.clone(), i.mesh.clone(), uniforms)
                    })
                    .collect(),
            ),
        ));
    }
}

/// One frame's draw: each instance's mesh and uniforms.
struct Paint(Vec<(String, Arc<Mesh>, Uniforms)>);

impl egui_wgpu::CallbackTrait for Paint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<Resources>() else {
            return Vec::new();
        };
        for (key, mesh, _) in &self.0 {
            r.meshes
                .entry(key.clone())
                .or_insert_with(|| upload(device, mesh));
        }
        if self.0.len() > r.capacity {
            r.capacity = self.0.len().next_power_of_two();
            r.uniforms = uniform_buffer(device, r.stride, r.capacity);
            r.bind_group = bind_group(device, &r.layout, &r.uniforms);
        }
        for (i, (_, _, uniforms)) in self.0.iter().enumerate() {
            queue.write_buffer(
                &r.uniforms,
                i as u64 * r.stride,
                bytemuck::bytes_of(uniforms),
            );
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
        for (i, (key, _, _)) in self.0.iter().enumerate() {
            let Some(m) = r.meshes.get(key) else { continue };
            pass.set_bind_group(0, &r.bind_group, &[(i as u64 * r.stride) as u32]);
            pass.set_vertex_buffer(0, m.vertices.slice(..));
            pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.index_count, 0, 0..1);
        }
    }
}

fn upload(device: &wgpu::Device, mesh: &Mesh) -> GpuMesh {
    let interleaved: Vec<[f32; 6]> = mesh
        .positions
        .iter()
        .zip(&mesh.normals)
        .map(|(p, n)| [p[0], p[1], p[2], n[0], n[1], n[2]])
        .collect();
    GpuMesh {
        vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewer3d_vertices"),
            contents: bytemuck::cast_slice(&interleaved),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewer3d_indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        }),
        index_count: mesh.indices.len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_is_drawn_on_the_viewport_colour() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |ui| Viewer3d::unfitted().ui(ui, &[]));
        out.textures_delta.clear();
        let ground = out.shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Rect(r) => Some(r.fill),
            _ => None,
        });
        assert_eq!(ground, Some(LIGHT.viewport));
    }
}
