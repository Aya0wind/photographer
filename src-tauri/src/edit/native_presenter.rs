//! Direct PhotoCraft compositor → GPU texture → display-LUT → swapchain.
//! There is no render_to_vec, CPU pixel readback, JPEG or pixel IPC here.
use super::native_canvas::View;
use photocraft_engine::doc::Document;
use photocraft_gpu::{Compositor, DeviceHealth};
use std::sync::Arc;

pub(super) struct Presenter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    compositor: Compositor,
    health: DeviceHealth,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    image: Option<wgpu::Texture>,
    dimensions: [u32; 2],
    lut: Option<wgpu::Texture>,
    profile: Option<Vec<u8>>,
}
const SHADER: &str = r#"
struct Uniforms { uv:vec4<f32>, flags:vec4<f32> }
@group(0) @binding(0) var image:texture_2d<f32>;
@group(0) @binding(1) var pixel_sampler:sampler;
@group(0) @binding(2) var display:texture_3d<f32>;
@group(0) @binding(3) var<uniform> view:Uniforms;
struct Vertex { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> }
@vertex fn vertex(@builtin(vertex_index) i:u32)->Vertex {
    let positions=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    var result:Vertex;result.position=vec4(positions[i],0.0,1.0);
    result.uv=(positions[i]*vec2(0.5,-0.5)+vec2(0.5))*view.uv.zw+view.uv.xy;return result;
}
@fragment fn fragment(input:Vertex)->@location(0) vec4<f32> {
    let pixel=textureSample(image,pixel_sampler,input.uv);
    let coord=(clamp(pixel.rgb,vec3(0.0),vec3(1.0))*32.0+vec3(0.5))/33.0;
    var color=textureSample(display,pixel_sampler,coord).rgb;
    // LUT values are encoded sRGB; avoid encoding twice on an sRGB swapchain.
    if view.flags.x>0.5 { color=select(color/12.92,pow((color+vec3(0.055))/1.055,vec3(2.4)),color>vec3(0.04045)); }
    return vec4(color,pixel.a);
}
"#;

impl Presenter {
    pub(super) fn new(window: Arc<tauri::Window>) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window).map_err(|e| e.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        if adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            return Err("无硬件 GPU，使用普通预览".into());
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| e.to_string())?;
        let health = DeviceHealth::watch(&device);
        let watch = health.clone();
        device.on_uncaptured_error(std::sync::Arc::new(move |error| {
            eprintln!("[editor native GPU] {error:?}");
            watch.mark(photocraft_gpu::Fault::Error(format!("{error:?}")));
        }));
        let mut config = surface
            .get_default_config(&adapter, 1, 1)
            .ok_or("无法配置原生画布")?;
        if let Some(format) = surface
            .get_capabilities(&adapter)
            .formats
            .iter()
            .find(|f| !f.is_srgb())
        {
            config.format = *format;
        }
        let mut compositor =
            Compositor::try_new_with_format(&device, wgpu::TextureFormat::Rgba16Float)
                .map_err(|e| e.0)?;
        compositor.set_health(health.clone());
        compositor.set_memory_budget(256 * 1024 * 1024);
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::VERTEX,
            ty,
            count: None,
        };
        let texture = |dimension| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dimension,
            multisampled: false,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native viewport"),
            entries: &[
                entry(0, texture(wgpu::TextureViewDimension::D2)),
                entry(
                    1,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(2, texture(wgpu::TextureViewDimension::D3)),
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("PhotoCraft display"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("PhotoCraft direct canvas"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        eprintln!(
            "[editor native] {}: direct GPU display, no pixel readback",
            adapter.get_info().name
        );
        Ok(Self {
            device,
            queue,
            surface,
            config,
            compositor,
            health,
            pipeline,
            layout,
            sampler,
            uniform,
            image: None,
            dimensions: [0, 0],
            lut: None,
            profile: None,
        })
    }

    pub(super) fn draw(
        &mut self,
        doc: &Document,
        view: &View,
        recompose: bool,
    ) -> Result<usize, String> {
        if let Some(error) = self.health.fault() {
            return Err(error.to_string());
        }
        self.compositor.supports(doc).map_err(|e| e.0)?;
        let dimensions = [doc.size.width, doc.size.height];
        if dimensions != self.dimensions {
            self.image = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("canvas composite"),
                size: wgpu::Extent3d {
                    width: dimensions[0],
                    height: dimensions[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
            self.dimensions = dimensions;
        }
        let image = self.image.as_ref().ok_or("无 GPU 图片")?;
        let uploads = if recompose {
            self.compositor
                .render(
                    &self.device,
                    &self.queue,
                    doc,
                    doc.bounds(),
                    |encoder, chunk| {
                        encoder.copy_texture_to_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: chunk.texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            wgpu::TexelCopyTextureInfo {
                                texture: image,
                                mip_level: 0,
                                origin: wgpu::Origin3d {
                                    x: chunk.rect.x0 as u32,
                                    y: chunk.rect.y0 as u32,
                                    z: 0,
                                },
                                aspect: wgpu::TextureAspect::All,
                            },
                            wgpu::Extent3d {
                                width: chunk.rect.width(),
                                height: chunk.rect.height(),
                                depth_or_array_layers: 1,
                            },
                        );
                    },
                )
                .map_err(|e| e.0)?
                .bytes_uploaded
        } else {
            0
        };
        let profile = photocraft_engine::color_cmds::document_profile(doc);
        let bytes = profile.to_bytes().to_vec();
        if self.profile.as_ref() != Some(&bytes) {
            let transform = photocraft_cms::Transform::new(
                &profile,
                &photocraft_cms::Builtin::Srgb.profile(),
                photocraft_cms::Intent::RelativeColorimetric,
                true,
            )
            .map_err(|e| e.to_string())?;
            let data = photocraft_cms::Lut3d::from_transform(&transform, 33).to_rgba16f_bytes();
            let lut = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("display ICC LUT"),
                size: wgpu::Extent3d {
                    width: 33,
                    height: 33,
                    depth_or_array_layers: 33,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &lut,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(33 * 8),
                    rows_per_image: Some(33),
                },
                wgpu::Extent3d {
                    width: 33,
                    height: 33,
                    depth_or_array_layers: 33,
                },
            );
            self.lut = Some(lut);
            self.profile = Some(bytes);
        }
        let size = [view.rect[2] as u32, view.rect[3] as u32];
        if self.config.width != size[0] || self.config.height != size[1] {
            self.config.width = size[0];
            self.config.height = size[1];
            self.surface.configure(&self.device, &self.config);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            _ => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                    _ => return Err("GPU 显示表面暂不可用".into()),
                }
            }
        };
        let values = [
            view.uv[0],
            view.uv[1],
            view.uv[2],
            view.uv[3],
            if self.config.format.is_srgb() {
                1.0
            } else {
                0.0
            },
            0.0,
            0.0,
            0.0,
        ];
        let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.queue.write_buffer(&self.uniform, 0, &data);
        let image_view = image.create_view(&Default::default());
        let lut_view = self
            .lut
            .as_ref()
            .ok_or("无显示变换")?
            .create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&image_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        });
        let output = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        Ok(uploads)
    }
}
