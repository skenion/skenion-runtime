use std::{borrow::Cow, error::Error, num::NonZeroU64, path::PathBuf, sync::Arc, time::Instant};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use crate::{
    PreviewFrameLimit,
    render::{PreviewDocument, RenderScene, render_scene_from_preview_document},
    telemetry::PreviewTelemetryWriter,
};

pub fn run_render_preview_window(
    document: PreviewDocument,
    frame_limit: PreviewFrameLimit,
    telemetry_path: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    let (scene, scene_error) = match render_scene_from_preview_document(&document) {
        Ok(scene) => (scene, None),
        Err(error) => (RenderScene::default(), Some(error.to_string())),
    };
    let mut app = NativePreviewApp::new(document, scene, scene_error, frame_limit, telemetry_path);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct NativePreviewApp {
    document: PreviewDocument,
    scene: RenderScene,
    window: Option<Arc<Window>>,
    renderer: Option<WgpuPreviewRenderer>,
    telemetry: Option<PreviewTelemetryWriter>,
    frame_index: usize,
    frame_limit: PreviewFrameLimit,
    started_at: Instant,
    last_redraw: Instant,
}

impl NativePreviewApp {
    fn new(
        document: PreviewDocument,
        scene: RenderScene,
        scene_error: Option<String>,
        frame_limit: PreviewFrameLimit,
        telemetry_path: Option<PathBuf>,
    ) -> Self {
        let mut telemetry = telemetry_path.map(|path| {
            PreviewTelemetryWriter::new(
                path,
                document.graph.id.clone(),
                document.graph.revision.clone(),
                document.session_revision,
                scene.renderer_label(),
                "wgpu",
                scene.source_node_id(),
            )
        });
        if let (Some(error), Some(telemetry)) = (scene_error, telemetry.as_mut()) {
            telemetry.record_error(error);
        }
        Self {
            document,
            scene,
            window: None,
            renderer: None,
            telemetry,
            frame_index: 0,
            frame_limit,
            started_at: Instant::now(),
            last_redraw: Instant::now(),
        }
    }

    fn title(&self) -> String {
        let source_node_id = self.scene.source_node_id();
        let source = source_node_id.as_deref().unwrap_or("default-clear");
        format!(
            "Skenion Preview - {} rev {} session {} source {}",
            self.document.graph.id,
            self.document.graph.revision,
            self.document.session_revision,
            source
        )
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn should_exit_after_redraw(&self) -> bool {
        match self.frame_limit {
            PreviewFrameLimit::Frames(frame_count) => self.frame_index >= frame_count.max(1),
            PreviewFrameLimit::UntilClose => false,
        }
    }
}

impl ApplicationHandler for NativePreviewApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = WindowAttributes::default()
            .with_title(self.title())
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create preview window: {error}");
                if let Some(telemetry) = &mut self.telemetry {
                    telemetry.record_error(format!("failed to create preview window: {error}"));
                }
                event_loop.exit();
                return;
            }
        };

        match WgpuPreviewRenderer::new(Arc::clone(&window), &self.scene) {
            Ok(renderer) => {
                self.renderer = Some(renderer);
                self.window = Some(window);
                self.request_redraw();
            }
            Err(error) if matches!(self.scene, RenderScene::FullscreenShader(_)) => {
                eprintln!("failed to initialize fullscreen shader renderer: {error}");
                if let Some(telemetry) = &mut self.telemetry {
                    telemetry.record_error(format!(
                        "failed to initialize fullscreen shader renderer: {error}"
                    ));
                }
                let fallback_scene = RenderScene::default();
                match WgpuPreviewRenderer::new(Arc::clone(&window), &fallback_scene) {
                    Ok(renderer) => {
                        self.scene = fallback_scene;
                        self.renderer = Some(renderer);
                        self.window = Some(window);
                        self.request_redraw();
                    }
                    Err(fallback_error) => {
                        eprintln!(
                            "failed to initialize fallback preview renderer: {fallback_error}"
                        );
                        if let Some(telemetry) = &mut self.telemetry {
                            telemetry.record_error(format!(
                                "failed to initialize fallback preview renderer: {fallback_error}"
                            ));
                        }
                        event_loop.exit();
                    }
                }
            }
            Err(error) => {
                eprintln!("failed to initialize preview renderer: {error}");
                if let Some(telemetry) = &mut self.telemetry {
                    telemetry
                        .record_error(format!("failed to initialize preview renderer: {error}"));
                }
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = &self.window else {
            return;
        };
        if window.id() != window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                window.set_title(&self.title());
                let frame_started = Instant::now();
                if let Some(renderer) = &mut self.renderer
                    && let Err(error) = renderer.render(
                        &self.scene,
                        self.frame_index as u32,
                        self.started_at.elapsed().as_secs_f32(),
                    )
                {
                    eprintln!("failed to render preview frame: {error}");
                    if let Some(telemetry) = &mut self.telemetry {
                        telemetry.record_error(format!("failed to render preview frame: {error}"));
                    }
                } else if let Some(telemetry) = &mut self.telemetry {
                    telemetry.record_frame(frame_started.elapsed().as_secs_f64() * 1000.0);
                }
                self.frame_index += 1;
                if self.should_exit_after_redraw() {
                    event_loop.exit();
                } else {
                    self.last_redraw = Instant::now();
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self.last_redraw.elapsed() >= std::time::Duration::from_millis(16) {
            self.request_redraw();
        }
    }
}

struct WgpuPreviewRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    mode: WgpuPreviewMode,
}

enum WgpuPreviewMode {
    Clear,
    FullscreenShader {
        pipeline: wgpu::RenderPipeline,
        bind_group: wgpu::BindGroup,
        uniform_buffer: wgpu::Buffer,
    },
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
struct SkenionFrameUniform {
    resolution: [f32; 2],
    time: f32,
    frame: u32,
}

impl SkenionFrameUniform {
    fn new(width: u32, height: u32, time: f32, frame: u32) -> Self {
        Self {
            resolution: [width as f32, height as f32],
            time,
            frame,
        }
    }
}

impl WgpuPreviewRenderer {
    fn new(window: Arc<Window>, scene: &RenderScene) -> Result<Self, String> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window)
            .map_err(|error| error.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|error| error.to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("skenion-preview-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|error| error.to_string())?;
        let config = surface
            .get_default_config(&adapter, width, height)
            .ok_or_else(|| "preview surface is not supported by the selected adapter".to_owned())?;
        surface.configure(&device, &config);
        let mode = WgpuPreviewMode::new(&device, &config, scene)?;

        Ok(Self {
            surface,
            device,
            queue,
            config,
            mode,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.config.width == width && self.config.height == height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn render(&mut self, scene: &RenderScene, frame_index: u32, time: f32) -> Result<(), String> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(output)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
                    status => return Err(format!("surface acquisition retry failed: {status:?}")),
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface acquisition validation error".to_owned());
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("skenion-preview-render-encoder"),
            });
        match &self.mode {
            WgpuPreviewMode::Clear => {
                let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("skenion-preview-clear-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu_color(scene.fallback_clear_color())),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            WgpuPreviewMode::FullscreenShader {
                pipeline,
                bind_group,
                uniform_buffer,
            } => {
                let uniform = SkenionFrameUniform::new(
                    self.config.width,
                    self.config.height,
                    time,
                    frame_index,
                );
                self.queue
                    .write_buffer(uniform_buffer, 0, bytemuck::bytes_of(&uniform));
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("skenion-preview-fullscreen-shader-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu_color(scene.fallback_clear_color())),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                render_pass.set_pipeline(pipeline);
                render_pass.set_bind_group(0, bind_group, &[]);
                render_pass.draw(0..3, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        output.present();
        Ok(())
    }
}

impl WgpuPreviewMode {
    fn new(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        scene: &RenderScene,
    ) -> Result<Self, String> {
        match scene {
            RenderScene::ClearColor(_) => Ok(Self::Clear),
            RenderScene::FullscreenShader(shader_scene) => {
                Self::fullscreen_shader(device, config, &shader_scene.source)
            }
        }
    }

    fn fullscreen_shader(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        source: &str,
    ) -> Result<Self, String> {
        let uniform = SkenionFrameUniform::new(config.width, config.height, 0.0, 0);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("skenion-frame-uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skenion-frame-bind-group-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(
                        std::mem::size_of::<SkenionFrameUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("skenion-frame-bind-group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skenion-fullscreen-shader-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader_error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("skenion-fullscreen-shader-module"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(source)),
        });
        let targets = [Some(wgpu::ColorTargetState {
            format: config.format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("skenion-fullscreen-shader-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
        if let Some(error) = pollster::block_on(shader_error_scope.pop()) {
            return Err(format!("shader validation failed: {error}"));
        }

        Ok(Self::FullscreenShader {
            pipeline,
            bind_group,
            uniform_buffer,
        })
    }
}

fn wgpu_color(color: [f64; 4]) -> wgpu::Color {
    wgpu::Color {
        r: color[0],
        g: color[1],
        b: color[2],
        a: color[3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_scene_color_to_wgpu_color() {
        let color = wgpu_color([0.1, 0.2, 0.3, 1.0]);

        assert_eq!(color.r, 0.1);
        assert_eq!(color.g, 0.2);
        assert_eq!(color.b, 0.3);
        assert_eq!(color.a, 1.0);
    }

    #[test]
    fn frame_uniform_uses_resolution_time_and_frame() {
        let uniform = SkenionFrameUniform::new(960, 540, 1.25, 12);

        assert_eq!(uniform.resolution, [960.0, 540.0]);
        assert_eq!(uniform.time, 1.25);
        assert_eq!(uniform.frame, 12);
        assert_eq!(std::mem::size_of::<SkenionFrameUniform>(), 16);
    }

    #[test]
    fn render_scene_reports_renderer_labels() {
        assert_eq!(RenderScene::default().renderer_label(), "clear-color");
        assert_eq!(RenderScene::default().source_node_id(), None);
    }
}
