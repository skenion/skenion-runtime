use std::{borrow::Cow, error::Error, num::NonZeroU64, path::PathBuf, sync::Arc, time::Instant};

use serde::Serialize;
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use crate::{
    PreviewFrameLimit,
    render::{
        FullscreenShaderScene, PreviewDocument, RenderScene, ShaderUniformBinding,
        ShaderUniformValue, render_scene_from_preview_document,
    },
    telemetry::{
        PreviewTelemetryWriter, ShaderDiagnostic, ShaderDiagnosticPhase, ShaderDiagnosticSeverity,
        ShaderDiagnosticSource,
    },
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedShaderResponse {
    pub ok: bool,
    pub node_id: Option<String>,
    pub language: Option<String>,
    pub source: Option<String>,
    pub source_map: Option<GeneratedShaderSourceMap>,
    pub diagnostics: Vec<ShaderDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedShaderSourceMap {
    pub user_source_start_line: usize,
    pub generated_line_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedShaderSource {
    pub source: String,
    pub source_map: GeneratedShaderSourceMap,
}

pub fn generated_shader_response_from_preview_document(
    document: &PreviewDocument,
) -> GeneratedShaderResponse {
    match render_scene_from_preview_document(document) {
        Ok(RenderScene::FullscreenShader(scene)) => {
            let generated = generated_fullscreen_shader_module_source(&scene);
            GeneratedShaderResponse {
                ok: true,
                node_id: Some(scene.source_node_id),
                language: Some("wgsl".to_owned()),
                source: Some(generated.source),
                source_map: Some(generated.source_map),
                diagnostics: Vec::new(),
            }
        }
        Ok(RenderScene::ClearColor(_)) => GeneratedShaderResponse {
            ok: false,
            node_id: None,
            language: None,
            source: None,
            source_map: None,
            diagnostics: vec![ShaderDiagnostic::new(
                ShaderDiagnosticSeverity::Info,
                ShaderDiagnosticPhase::WgslGeneration,
                "no-generated-shader",
                "current render scene does not use a fullscreen shader",
                ShaderDiagnosticSource::Runtime,
            )],
        },
        Err(error) => GeneratedShaderResponse {
            ok: false,
            node_id: None,
            language: None,
            source: None,
            source_map: None,
            diagnostics: error.shader_diagnostics(),
        },
    }
}

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
                    let phase = if error.contains("shader validation failed") {
                        ShaderDiagnosticPhase::WgslCompile
                    } else {
                        ShaderDiagnosticPhase::RenderPipeline
                    };
                    telemetry.record_shader_diagnostic(ShaderDiagnostic::error(
                        phase,
                        "fullscreen-shader-initialization-failed",
                        format!("failed to initialize fullscreen shader renderer: {error}"),
                        ShaderDiagnosticSource::Generated,
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
                    shader_uniforms(scene),
                );
                self.queue.write_buffer(uniform_buffer, 0, &uniform.bytes);
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
                Self::fullscreen_shader(device, config, shader_scene)
            }
        }
    }

    fn fullscreen_shader(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        shader_scene: &FullscreenShaderScene,
    ) -> Result<Self, String> {
        let uniform =
            SkenionFrameUniform::new(config.width, config.height, 0.0, 0, &shader_scene.uniforms);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("skenion-frame-uniform"),
            contents: &uniform.bytes,
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
                    min_binding_size: NonZeroU64::new(uniform.bytes.len() as u64),
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
        let module_source = fullscreen_shader_module_source(shader_scene);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("skenion-fullscreen-shader-module"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(module_source)),
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

#[derive(Debug, Clone, PartialEq)]
struct SkenionFrameUniform {
    bytes: Vec<u8>,
}

impl SkenionFrameUniform {
    fn new(
        width: u32,
        height: u32,
        time: f32,
        frame: u32,
        uniforms: &[ShaderUniformBinding],
    ) -> Self {
        let mut bytes = Vec::new();
        write_f32(&mut bytes, 0, width as f32);
        write_f32(&mut bytes, 4, height as f32);
        write_f32(&mut bytes, 8, time);
        write_u32(&mut bytes, 12, frame);
        let mut offset = 16;

        for uniform in uniforms {
            let (alignment, size) = uniform_layout(&uniform.value);
            offset = align_to(offset, alignment);
            match &uniform.value {
                ShaderUniformValue::F32(value) => write_f32(&mut bytes, offset, *value),
                ShaderUniformValue::I32(value) => write_i32(&mut bytes, offset, *value),
                ShaderUniformValue::Bool(value) => {
                    write_u32(&mut bytes, offset, u32::from(*value));
                }
                ShaderUniformValue::ColorRgba(value) => {
                    for (index, component) in value.iter().enumerate() {
                        write_f32(&mut bytes, offset + index * 4, *component);
                    }
                }
            }
            offset += size;
        }

        bytes.resize(align_to(offset, 16), 0);
        Self { bytes }
    }
}

fn shader_uniforms(scene: &RenderScene) -> &[ShaderUniformBinding] {
    match scene {
        RenderScene::FullscreenShader(shader_scene) => &shader_scene.uniforms,
        RenderScene::ClearColor(_) => &[],
    }
}

fn generated_fullscreen_shader_module_source(
    shader_scene: &FullscreenShaderScene,
) -> GeneratedShaderSource {
    let mut source = String::from(
        "struct SkenionFrame {\n  resolution: vec2<f32>,\n  time: f32,\n  frame: u32,\n",
    );
    for uniform in &shader_scene.uniforms {
        source.push_str("  ");
        source.push_str(&uniform.id);
        source.push_str(": ");
        source.push_str(wgsl_type(&uniform.value));
        source.push_str(",\n");
    }
    source.push_str("}\n\n@group(0) @binding(0)\nvar<uniform> skenion: SkenionFrame;\n\nfn sk_bool(value: u32) -> bool {\n  return value != 0u;\n}\n\nstruct VertexOut {\n  @builtin(position) position: vec4<f32>,\n}\n\n@vertex\nfn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {\n  var positions = array<vec2<f32>, 3>(\n    vec2<f32>(-1.0, -3.0),\n    vec2<f32>(-1.0,  1.0),\n    vec2<f32>( 3.0,  1.0)\n  );\n\n  var out: VertexOut;\n  out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);\n  return out;\n}\n\n");
    let user_source_start_line = source.lines().count() + 1;
    source.push_str(&shader_scene.source);
    GeneratedShaderSource {
        source,
        source_map: GeneratedShaderSourceMap {
            user_source_start_line,
            generated_line_offset: user_source_start_line - 1,
        },
    }
}

fn fullscreen_shader_module_source(shader_scene: &FullscreenShaderScene) -> String {
    generated_fullscreen_shader_module_source(shader_scene).source
}

fn wgsl_type(value: &ShaderUniformValue) -> &'static str {
    match value {
        ShaderUniformValue::F32(_) => "f32",
        ShaderUniformValue::I32(_) => "i32",
        ShaderUniformValue::Bool(_) => "u32",
        ShaderUniformValue::ColorRgba(_) => "vec4<f32>",
    }
}

fn uniform_layout(value: &ShaderUniformValue) -> (usize, usize) {
    match value {
        ShaderUniformValue::ColorRgba(_) => (16, 16),
        ShaderUniformValue::F32(_) | ShaderUniformValue::I32(_) | ShaderUniformValue::Bool(_) => {
            (4, 4)
        }
    }
}

fn align_to(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

fn write_f32(bytes: &mut Vec<u8>, offset: usize, value: f32) {
    write_bytes(bytes, offset, &value.to_le_bytes());
}

fn write_i32(bytes: &mut Vec<u8>, offset: usize, value: i32) {
    write_bytes(bytes, offset, &value.to_le_bytes());
}

fn write_u32(bytes: &mut Vec<u8>, offset: usize, value: u32) {
    write_bytes(bytes, offset, &value.to_le_bytes());
}

fn write_bytes(bytes: &mut Vec<u8>, offset: usize, value: &[u8]) {
    let end = offset + value.len();
    if bytes.len() < end {
        bytes.resize(end, 0);
    }
    bytes[offset..end].copy_from_slice(value);
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
        let uniform = SkenionFrameUniform::new(
            960,
            540,
            1.25,
            12,
            &[
                ShaderUniformBinding {
                    id: "speed".to_owned(),
                    value: ShaderUniformValue::F32(0.75),
                },
                ShaderUniformBinding {
                    id: "enabled".to_owned(),
                    value: ShaderUniformValue::Bool(true),
                },
                ShaderUniformBinding {
                    id: "iterations".to_owned(),
                    value: ShaderUniformValue::I32(8),
                },
                ShaderUniformBinding {
                    id: "tint".to_owned(),
                    value: ShaderUniformValue::ColorRgba([1.0, 0.5, 0.25, 0.8]),
                },
            ],
        );

        assert_eq!(read_f32(&uniform.bytes, 0), 960.0);
        assert_eq!(read_f32(&uniform.bytes, 4), 540.0);
        assert_eq!(read_f32(&uniform.bytes, 8), 1.25);
        assert_eq!(read_u32(&uniform.bytes, 12), 12);
        assert_eq!(read_f32(&uniform.bytes, 16), 0.75);
        assert_eq!(read_u32(&uniform.bytes, 20), 1);
        assert_eq!(read_i32(&uniform.bytes, 24), 8);
        assert_eq!(read_f32(&uniform.bytes, 32), 1.0);
        assert_eq!(read_f32(&uniform.bytes, 36), 0.5);
        assert_eq!(read_f32(&uniform.bytes, 40), 0.25);
        assert_eq!(read_f32(&uniform.bytes, 44), 0.8);
        assert_eq!(uniform.bytes.len(), 48);
    }

    #[test]
    fn render_scene_reports_renderer_labels() {
        assert_eq!(RenderScene::default().renderer_label(), "clear-color");
        assert_eq!(RenderScene::default().source_node_id(), None);
    }

    #[test]
    fn generated_shader_module_declares_dynamic_uniforms() {
        let scene = FullscreenShaderScene {
            language: crate::render::ShaderLanguage::Wgsl,
            source: "@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"
                .to_owned(),
            source_node_id: "shader_1".to_owned(),
            uniforms: vec![
                ShaderUniformBinding {
                    id: "speed".to_owned(),
                    value: ShaderUniformValue::F32(0.5),
                },
                ShaderUniformBinding {
                    id: "enabled".to_owned(),
                    value: ShaderUniformValue::Bool(true),
                },
            ],
            fallback_clear_color: [0.0, 0.0, 0.0, 1.0],
        };

        let generated = generated_fullscreen_shader_module_source(&scene);
        let source = generated.source;

        assert!(source.contains("speed: f32"));
        assert!(source.contains("enabled: u32"));
        assert!(source.contains("fn sk_bool(value: u32) -> bool"));
        assert!(source.contains("fn vs_main"));
        assert!(source.contains("fn fs_main"));
        assert_eq!(
            generated.source_map.generated_line_offset + 1,
            generated.source_map.user_source_start_line
        );
        assert!(generated.source_map.user_source_start_line > 1);
    }

    #[test]
    fn fullscreen_shader_pipeline_reports_wgsl_validation_errors() {
        let Some((device, config)) = headless_test_device() else {
            eprintln!(
                "skipping WGPU validation diagnostic smoke because no headless adapter is available"
            );
            return;
        };
        let scene = FullscreenShaderScene {
            language: crate::render::ShaderLanguage::Wgsl,
            source: "@fragment\nfn fs_main() -> @location(0) vec4<f32> {\n  return vec4<f32>(skenion.missingField, 0.0, 0.0, 1.0);\n}"
                .to_owned(),
            source_node_id: "shader_1".to_owned(),
            uniforms: vec![ShaderUniformBinding {
                id: "speed".to_owned(),
                value: ShaderUniformValue::F32(0.5),
            }],
            fallback_clear_color: [0.0, 0.0, 0.0, 1.0],
        };

        let error = match WgpuPreviewMode::fullscreen_shader(&device, &config, &scene) {
            Ok(_) => panic!("invalid WGSL should fail pipeline validation"),
            Err(error) => error,
        };

        assert!(error.contains("shader validation failed"));
        assert!(error.contains("missingField"));
    }

    fn headless_test_device() -> Option<(wgpu::Device, wgpu::SurfaceConfiguration)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })) {
                Ok(adapter) => adapter,
                Err(error) => {
                    eprintln!("headless wgpu adapter unavailable: {error}");
                    return None;
                }
            };
        let (device, _queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("skenion-preview-test-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })) {
                Ok(device_and_queue) => device_and_queue,
                Err(error) => {
                    eprintln!("headless wgpu device unavailable: {error}");
                    return None;
                }
            };
        let rgba_supported = adapter
            .get_texture_format_features(wgpu::TextureFormat::Rgba8UnormSrgb)
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT);
        let format = if rgba_supported {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Bgra8UnormSrgb
        };

        Some((
            device,
            wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: 64,
                height: 64,
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: Vec::new(),
            },
        ))
    }

    fn read_f32(bytes: &[u8], offset: usize) -> f32 {
        f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn read_i32(bytes: &[u8], offset: usize) -> i32 {
        i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }
}
