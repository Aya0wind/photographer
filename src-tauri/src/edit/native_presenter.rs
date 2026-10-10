//! Direct PhotoCraft compositor → GPU texture → display-LUT → swapchain.
//! There is no render_to_vec, CPU pixel readback, JPEG or pixel IPC here.
use super::native_canvas::View;
use photocraft_engine::doc::Document;
use photocraft_gpu::{Compositor, DeviceHealth};
use std::sync::Arc;
use tauri::Window;
use windows::Win32::UI::WindowsAndMessaging::*;

pub(super) struct Presenter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    configured: bool,
    compositor: Compositor,
    health: DeviceHealth,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    image: Option<wgpu::Texture>,
    dimensions: [u32; 2],
    annotation:wgpu::Texture,
    annotation_dimensions:[u32;2],
    lut: Option<wgpu::Texture>,
    profile: Option<(photocraft_engine::doc::ColorMode, Option<Arc<Vec<u8>>>)>,
}
const SHADER: &str = r#"
struct Uniforms { uv:vec4<f32>, flags:vec4<f32>, image_rect:vec4<f32>, map_x:vec4<f32>, map_y:vec4<f32>, guide:vec4<f32>, metrics:vec4<f32> }
@group(0) @binding(0) var image:texture_2d<f32>;
@group(0) @binding(1) var pixel_sampler:sampler;
@group(0) @binding(2) var display:texture_3d<f32>;
@group(0) @binding(3) var<uniform> view:Uniforms;
@group(0) @binding(4) var annotations:texture_2d<f32>;
struct Vertex { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> }
@vertex fn vertex(@builtin(vertex_index) i:u32)->Vertex {
    let positions=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    var result:Vertex;result.position=vec4(positions[i],0.0,1.0);
    result.uv=positions[i]*vec2(0.5,-0.5)+vec2(0.5);return result;
}
fn output(encoded:vec3<f32>)->vec4<f32> {
    var color=encoded;
    if view.flags.x>0.5 {color=select(color/12.92,pow((color+vec3(0.055))/1.055,vec3(2.4)),color>vec3(0.04045));}
    return vec4(color,1.0);
}
@fragment fn fragment(input:Vertex)->@location(0) vec4<f32> {
    let image_uv=(input.uv-view.image_rect.xy)/view.image_rect.zw;
    if any(image_uv<vec2(0.0)) || any(image_uv>vec2(1.0)) {return output(vec3(0.12549));}
    let source_uv=vec2(dot(view.map_x.xyz,vec3(image_uv,1.0)),dot(view.map_y.xyz,vec3(image_uv,1.0)));
    var working=vec3(1.0);
    if all(source_uv>=vec2(0.0)) && all(source_uv<=vec2(1.0)) {
        let pixel=textureSampleLevel(image,pixel_sampler,source_uv*view.uv.zw+view.uv.xy,0.0);
        working=mix(vec3(1.0),pixel.rgb,pixel.a);
    }
    if view.flags.z>0.5 {let annotation=textureSampleLevel(annotations,pixel_sampler,image_uv,0.0);working=mix(working,annotation.rgb,annotation.a);}
    let coord=(clamp(working,vec3(0.0),vec3(1.0))*32.0+vec3(0.5))/33.0;
    var color=textureSampleLevel(display,pixel_sampler,coord,0.0).rgb;
    if view.flags.y>0.5 {
        let lo=view.guide.xy;let hi=lo+view.guide.zw;
        let pixels=view.image_rect.zw*view.metrics.xy;
        let edge=vec2(1.25*view.metrics.z)/pixels;
        let inside=all(image_uv>=lo)&&all(image_uv<=hi);
        if !inside {color*=0.5;}
        let border=any(abs(image_uv-lo)<edge)||any(abs(image_uv-hi)<edge);
        if border && all(image_uv>=lo-edge)&&all(image_uv<=hi+edge) {color=vec3(1.0);}
        if inside {
            let third=lo+view.guide.zw/3.0;let two_thirds=lo+2.0*view.guide.zw/3.0;
            if any(abs(image_uv-third)<edge*0.5)||any(abs(image_uv-two_thirds)<edge*0.5){color=mix(color,vec3(1.0),0.3);}
        }
        let middle=(lo+hi)/2.0;
        let handles=array<vec2<f32>,8>(lo,vec2(middle.x,lo.y),vec2(hi.x,lo.y),vec2(hi.x,middle.y),hi,vec2(middle.x,hi.y),vec2(lo.x,hi.y),vec2(lo.x,middle.y));
        for(var n=0;n<8;n++) {let delta=abs(image_uv-handles[n])*pixels;if all(delta<vec2(5.0*view.metrics.z)){color=vec3(0.1);if all(delta<vec2(3.5*view.metrics.z)){color=vec3(1.0);}}}
    }
    return output(color);
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
        // Keep at most one queued display frame; prefer replacing pending
        // frames over showing a FIFO of older slider values.
        config.desired_maximum_frame_latency = 1;
        if surface
            .get_capabilities(&adapter)
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            config.present_mode = wgpu::PresentMode::Mailbox;
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
                entry(4,texture(wgpu::TextureViewDimension::D2)),
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
            size: 112,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let annotation=device.create_texture(&wgpu::TextureDescriptor {
            label:Some("annotation placeholder"),size:wgpu::Extent3d{width:1,height:1,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba16Float,usage:wgpu::TextureUsages::COPY_DST|wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]
        });
        queue.write_texture(wgpu::TexelCopyTextureInfo{texture:&annotation,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},&[0u8;8],wgpu::TexelCopyBufferLayout{offset:0,bytes_per_row:Some(8),rows_per_image:Some(1)},wgpu::Extent3d{width:1,height:1,depth_or_array_layers:1});
        eprintln!(
            "[editor native] {}: direct GPU display, no pixel readback",
            adapter.get_info().name
        );
        Ok(Self {
            device,
            queue,
            surface,
            config,
            configured: false,
            compositor,
            health,
            pipeline,
            layout,
            sampler,
            uniform,
            image: None,
            dimensions: [0, 0],
            annotation,annotation_dimensions:[1,1],
            lut: None,
            profile: None,
        })
    }

    pub(super) fn wait_ready(&self) -> Result<(), String> {
        if self.health.wait(&self.device, None) {
            Ok(())
        } else {
            Err("GPU 首帧未完成".into())
        }
    }

    pub(super) fn draw(
        &mut self,
        doc: &Document,
        view: &View,
        recompose: bool,
    ) -> Result<usize, String> {self.draw_with_annotations(doc,view,recompose,None,false)}

    pub(super) fn draw_with_annotations(&mut self,doc:&Document,view:&View,recompose:bool,annotation:Option<&Document>,annotation_changed:bool)->Result<usize,String> {
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
        let mut uploads = if recompose {
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
        if let Some(overlay)=annotation {
            let dimensions=[overlay.size.width,overlay.size.height];
            let resized=dimensions!=self.annotation_dimensions;
            if resized {
                self.annotation=self.device.create_texture(&wgpu::TextureDescriptor{label:Some("post-geometry annotations"),size:wgpu::Extent3d{width:dimensions[0],height:dimensions[1],depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba16Float,usage:wgpu::TextureUsages::COPY_DST|wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]});
                self.annotation_dimensions=dimensions;
            }
            if resized||annotation_changed {
                let texture=&self.annotation;
                uploads+=self.compositor.render(&self.device,&self.queue,overlay,overlay.bounds(),|encoder,chunk|{
                    encoder.copy_texture_to_texture(wgpu::TexelCopyTextureInfo{texture:chunk.texture,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},wgpu::TexelCopyTextureInfo{texture,mip_level:0,origin:wgpu::Origin3d{x:chunk.rect.x0 as u32,y:chunk.rect.y0 as u32,z:0},aspect:wgpu::TextureAspect::All},wgpu::Extent3d{width:chunk.rect.width(),height:chunk.rect.height(),depth_or_array_layers:1});
                }).map_err(|e|e.0)?.bytes_uploaded;
            }
        }
        let profile_key = (doc.mode, doc.icc_profile.clone());
        if self.profile.as_ref() != Some(&profile_key) {
            let profile = photocraft_engine::color_cmds::document_profile(doc);
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
            self.profile = Some(profile_key);
        }
        let size = [view.rect[2] as u32, view.rect[3] as u32];
        if !self.configured || self.config.width != size[0] || self.config.height != size[1] {
            self.config.width = size[0];
            self.config.height = size[1];
            self.surface.configure(&self.device, &self.config);
            self.configured = true;
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
            if view.crop_guide.is_some(){1.0}else{0.0},
            if annotation.is_some(){1.0}else{0.0},
            0.0,
        ];
        let values:Vec<f32>=values.into_iter().chain(view.image).chain(view.mapping).chain(view.crop_guide.unwrap_or([0.0,0.0,1.0,1.0])).chain([view.rect[2] as f32,view.rect[3] as f32,view.scale,0.0]).collect();
        let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.queue.write_buffer(&self.uniform, 0, &data);
        let image_view = image.create_view(&Default::default());
        let annotation_view=self.annotation.create_view(&Default::default());
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
                wgpu::BindGroupEntry{binding:4,resource:wgpu::BindingResource::TextureView(&annotation_view)},
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

// SetParent changes the HWND style behind Tao's cached top-level flags.
// Hide the actual child window and acknowledge completion on the UI thread.
pub(super) fn hide_window(window: &Window) -> Result<(), String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    unsafe {
        SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA)
            .map_err(|e| e.to_string())?;
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW,
        )
        .map_err(|e| e.to_string())?;
        if IsWindowVisible(hwnd).as_bool() {
            return Err("Native canvas remains visible".into());
        }
    }
    Ok(())
}

/// Clip only opaque floating DOM controls out of the foreground native window.
/// Ownership of a successfully applied region transfers to Windows.
pub(super) fn clip_overlays(window:&Window,overlays:&[[i32;5]],width:i32,height:i32)->Result<(),String> {
    use windows::Win32::Graphics::Gdi::*;
    let hwnd=window.hwnd().map_err(|e|e.to_string())?;
    unsafe {
        if overlays.is_empty() {
            if SetWindowRgn(hwnd,None,true)==0 {return Err("清理画布区域失败".into());}
            return Ok(());
        }
        let region=CreateRectRgn(0,0,width,height);
        if region.0.is_null(){return Err("创建画布区域失败".into());}
        for r in overlays {
            let hole=CreateRoundRectRgn(r[0],r[1],r[0]+r[2],r[1]+r[3],r[4]*2,r[4]*2);
            if hole.0.is_null() {let _=DeleteObject(HGDIOBJ(region.0));return Err("创建浮动控件区域失败".into());}
            let result=CombineRgn(Some(region),Some(region),Some(hole),RGN_DIFF);
            let _=DeleteObject(HGDIOBJ(hole.0));
            if result.0==0 {let _=DeleteObject(HGDIOBJ(region.0));return Err("合并画布区域失败".into());}
        }
        if SetWindowRgn(hwnd,Some(region),true)==0 {let _=DeleteObject(HGDIOBJ(region.0));return Err("更新画布区域失败".into());}
    }
    Ok(())
}
